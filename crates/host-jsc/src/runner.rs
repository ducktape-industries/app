//! The runner and its instances (iOS). The page, and everything that
//! touches it, lives on the main thread ([`PAGE`]); what the loader thread
//! and the kernel's pool reach (the prepared code, the page's one fact) is
//! behind locks ([`SHARED`]). One runner per process: one page.
//!
//! Threads: `prepare` meters on the loader thread and never touches the
//! page; `instantiate` and every `begin_*` post to the main thread (the
//! seat's thread, where they already are: the post is then the call
//! itself); `describe` posts too, and blocks its pool thread for the
//! answer (never called on the main thread, which it would block). Every
//! completion runs on the main thread and lands on the seat's `Landing`.
//! A drop posts the page's part of it and never waits (contract
//! `ViewInstance`: a drop may happen under a lock the main thread waits on).

use crate::answer::{self, Export};
use crate::calls::{Answered, Begun, Calls, DEAD, Seating};
use crate::code::{Code, Codes};
use crate::ffi::Web;
use crate::lock;
use base64::Engine as _;
use ducktape_host::contract::{
    Failure, Fault, Landed, Landing, Prepared, RunnerConfig, SeatId, ViewInstance, ViewRunner,
};
use objc2::MainThreadMarker;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// A call's answer callback, and a seating's: on the main thread.
type Done = Box<dyn FnOnce(Answered)>;
type Work = Box<dyn FnOnce(Result<u64, String>)>;

/// The page: its web view and what is in flight on it.
struct Page {
    web: Web,
    calls: Calls<Done, Work>,
    /// The life a failed page was loaded again for ([`page_failed`]): a
    /// failure of that load stands, so a page that keeps failing is loaded
    /// again once per failure, not in a loop.
    reloaded: Option<u64>,
}

thread_local! {
    /// The one page, on the main thread.
    static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
}

/// What the loader thread and the kernel's pool share with the page.
struct Shared {
    cache: Option<PathBuf>,
    /// Prepared code, while anything holds it.
    code: Codes,
    /// Describe modules by sha256, held for the process; `None` for bytes
    /// the runners do not take.
    described: Mutex<HashMap<[u8; 32], Option<Arc<Code>>>>,
    /// The page's one fact (SPEC §6.6): whether it has WebAssembly, once
    /// the first page loaded; or why no page did.
    wasm: Mutex<Option<Result<(), String>>>,
    probed: Condvar,
    /// Page deaths so far, for the gate's killer to wait on.
    deaths: Mutex<u64>,
    died: Condvar,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// Runs `work` on the main thread: here, when this is it; else posted,
/// never waited for.
fn on_main(work: impl FnOnce(MainThreadMarker) + Send + 'static) {
    match MainThreadMarker::new() {
        Some(mtm) => work(mtm),
        None => dispatch2::DispatchQueue::main().exec_async(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                work(mtm);
            }
        }),
    }
}

/// `work` on the page, its borrow let go before anything it hands back is
/// run; `None` before the runner made the page.
fn with_page<T>(work: impl FnOnce(&mut Page) -> T) -> Option<T> {
    PAGE.with(|page| page.borrow_mut().as_mut().map(work))
}

/// Loads the page (again), outside its borrow: WebKit may call a delegate
/// back as a load begins (the load it cancels failing), and that delegate
/// borrows the page.
fn load_page() {
    if let Some(web) = with_page(|page| page.web.clone()) {
        web.load();
    }
}

/// The glue's words for code it could not fetch or compile (`glue.js`,
/// `seatView`).
const NOT_LOADED: &str = "the view's code did not load";

impl Drop for Code {
    /// The page forgets its compiled module; posted to the main thread,
    /// never waited for, even there.
    fn drop(&mut self) {
        let name = std::mem::take(&mut self.name);
        dispatch2::DispatchQueue::main().exec_async(move || {
            let life = with_page(|page| page.calls.life());
            if let Some(life) = life {
                call(life, "forget", 0, &name, "", Box::new(|_| {}));
            }
        });
    }
}

/// One call into the page on `life`, `done` given its answer: sent, or,
/// on a page that died since (or never loaded), answered as dead at once.
fn call(life: u64, op: &str, id: u64, name: &str, arg: &str, done: Done) {
    let text = format!("{op} {id} {name} {arg}");
    let sent = with_page(|page| match page.calls.begin(life, done) {
        Begun::Send(token) => {
            page.web
                .call(&text, move |answered| answer_call(token, answered));
            None
        }
        Begun::Dead(done) => Some(done),
    });
    if let Some(Some(done)) = sent {
        done(Err(DEAD.into()));
    }
}

/// The call under `token` answered: its `done`, once (none for a call the
/// page's death answered already). Taken up on the next turn of the main
/// queue if it came while the page is borrowed (WebKit answering a call
/// as it is made).
fn answer_call(token: u64, answered: Answered) {
    if page_busy() {
        return dispatch2::DispatchQueue::main().exec_async(move || answer_call(token, answered));
    }
    if let Some(Some(done)) = with_page(|page| page.calls.answered(token)) {
        done(answered);
    }
}

/// Whether the page is borrowed right now (a callback inside a page call).
fn page_busy() -> bool {
    PAGE.with(|page| page.try_borrow_mut().is_err())
}

/// `work` once the page is ready, on its life; at once when it is.
fn seating(work: Work) {
    let decided = with_page(|page| page.calls.when_ready(work));
    match decided {
        Some(Seating::Run(work, life)) => work(Ok(life)),
        Some(Seating::Refused(work, why)) => work(Err(why)),
        Some(Seating::Held) | None => {}
    }
}

/// The page loaded (`webView:didFinishNavigation:`): it is asked its one
/// fact and given the budget, then the seatings held for it run.
pub(crate) fn page_loaded(_: MainThreadMarker) {
    let booted = with_page(|page| {
        let life = page.calls.life();
        page.web.call(
            &format!("boot 0 {} {}", crate::VIEW_FUEL, crate::DESCRIBE_FUEL),
            move |answered| booted(life, answered),
        );
    });
    if booted.is_none() {
        tracing::warn!(target: "ducktape::jsc", "a page loaded with no runner");
    }
}

/// The page's answer to `boot`: its fact kept, and the page ready (or
/// failed).
fn booted(life: u64, answered: Answered) {
    if page_busy() {
        return dispatch2::DispatchQueue::main().exec_async(move || booted(life, answered));
    }
    // a page that died while it booted boots again once its successor loads
    if with_page(|page| page.calls.life()) != Some(life) {
        return;
    }
    let fact = match answered.map(|text| answer::read(&text).outcome) {
        Ok(Ok(bytes)) if bytes == b"object" => Ok(()),
        // Lockdown Mode: the page has no WebAssembly (SPEC §6.6)
        Ok(Ok(_)) => Err("WebAssembly".to_owned()),
        Ok(Err(fault)) => Err(fault.to_string()),
        Err(why) => Err(why),
    };
    if let Some(shared) = SHARED.get() {
        let mut wasm = lock(&shared.wasm);
        if wasm.is_none() {
            *wasm = Some(fact.clone());
            shared.probed.notify_all();
        }
    }
    match fact {
        Err(why) if why != "WebAssembly" => page_failed(why),
        // no WebAssembly is a fact, not a failure: nothing is prepared, so
        // nothing is seated
        _ => {
            if let Some((held, life)) = with_page(|page| page.calls.ready()) {
                for work in held {
                    work(Ok(life));
                }
            }
        }
    }
}

/// The page did not load, or did not boot. A page that booted before (a
/// load after a death, its boot) is loaded once more on a new life, every
/// call on it answered as dead; otherwise, or when that load fails too,
/// every seating held is refused with `why`, and every later one, until
/// the next death loads the page again.
pub(crate) fn page_failed(why: String) {
    tracing::error!(target: "ducktape::jsc", %why, "the views' page did not load");
    let booted_before = SHARED.get().is_some_and(|shared| {
        let mut wasm = lock(&shared.wasm);
        match &*wasm {
            Some(Ok(())) => true,
            Some(Err(_)) => false,
            None => {
                *wasm = Some(Err(why.clone()));
                shared.probed.notify_all();
                false
            }
        }
    });
    let again = booted_before
        .then(|| {
            with_page(|page| {
                if page.reloaded == Some(page.calls.life()) {
                    return None;
                }
                let flying = page.calls.died();
                page.reloaded = Some(page.calls.life());
                Some(flying)
            })
            .flatten()
        })
        .flatten();
    if let Some(flying) = again {
        load_page();
        for done in flying {
            done(Err(DEAD.into()));
        }
        return;
    }
    if let Some(held) = with_page(|page| page.calls.failed(why.clone())) {
        for work in held {
            work(Err(why.clone()));
        }
    }
}

/// The content process died (`webViewWebContentProcessDidTerminate:`):
/// every call in flight lands the runner's fault, every instance of the
/// dead page is dead, and the page loads again in a new process. A first
/// boot the death cut short says nothing about the page: the next boot's
/// answer is its fact.
pub(crate) fn page_died(_: MainThreadMarker) {
    tracing::warn!(target: "ducktape::jsc", reason = "page_died", "the views' content process died");
    let flying = with_page(|page| page.calls.died());
    if let Some(shared) = SHARED.get() {
        let mut wasm = lock(&shared.wasm);
        if matches!(&*wasm, Some(Err(why)) if why != "WebAssembly") {
            *wasm = None;
        }
        drop(wasm);
        *lock(&shared.deaths) += 1;
        shared.died.notify_all();
    }
    load_page();
    for done in flying.into_iter().flatten() {
        done(Err(DEAD.into()));
    }
}

/// What the scheme handler answers `dt://app/<name>` with: the page, or
/// metered code something holds.
pub(crate) fn served(name: &str) -> Option<(Arc<[u8]>, &'static str)> {
    match name {
        "index.html" => return Some((Arc::from(&include_bytes!("index.html")[..]), "text/html")),
        "glue.js" => return Some((Arc::from(&include_bytes!("glue.js")[..]), "text/javascript")),
        _ => {}
    }
    let shared = SHARED.get()?;
    let found = shared.code.named(name).or_else(|| {
        lock(&shared.described)
            .values()
            .flatten()
            .find(|code| code.name == name)
            .cloned()
    })?;
    Some((found.bytes(), "application/wasm"))
}

/// The iOS view runner (crate doc). One per process.
pub struct JscRunner {
    shared: &'static Shared,
}

impl JscRunner {
    /// The runner and its page, made on the main thread, which is the only
    /// thread that may: off it, or a second time in a process, the runner's
    /// own failure. `config.cache_dir` keeps the metered code across
    /// launches.
    pub fn new(config: RunnerConfig) -> Result<Self, Failure> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(Failure::Runner(
                "the view runner is made on the main thread".into(),
            ));
        };
        let fresh = Shared {
            cache: config.cache_dir,
            code: Codes::default(),
            described: Mutex::default(),
            wasm: Mutex::new(None),
            probed: Condvar::new(),
            deaths: Mutex::new(0),
            died: Condvar::new(),
        };
        if SHARED.set(fresh).is_err() {
            return Err(Failure::Runner("one view runner per process".into()));
        }
        let shared = SHARED
            .get()
            .ok_or_else(|| Failure::Runner("no runner state".into()))?;
        PAGE.with(|page| {
            *page.borrow_mut() = Some(Page {
                web: Web::new(mtm),
                calls: Calls::new(),
                reloaded: None,
            });
        });
        load_page();
        Ok(Self { shared })
    }

    /// Whether the page has WebAssembly, once its first load says: waited
    /// for here (a loader thread's wait; on the main thread, which loads the
    /// page, it would never end, so there it is the runner's failure).
    fn wasm(&self) -> Result<(), Failure> {
        let mut wasm = lock(&self.shared.wasm);
        while wasm.is_none() {
            if MainThreadMarker::new().is_some() {
                return Err(Failure::Runner(
                    "the page has not loaded yet, and the main thread loads it".into(),
                ));
            }
            wasm = self
                .shared
                .probed
                .wait(wasm)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match wasm.clone().unwrap_or(Ok(())) {
            Ok(()) => Ok(()),
            Err(why) if why == "WebAssembly" => Err(Failure::Unsupported("WebAssembly".into())),
            Err(why) => Err(Failure::Runner(why)),
        }
    }

    /// The content process's pid, for the simulator gate's killer.
    #[cfg(feature = "gate")]
    pub fn content_process(&self) -> Option<i32> {
        dispatch2::run_on_main(|_| with_page(|page| page.web.content_process()).flatten())
    }

    /// Waits until the page has died more than `seen` times; the deaths so
    /// far. For the simulator gate, which kills the page from outside.
    #[cfg(feature = "gate")]
    pub fn wait_for_death(&self, seen: u64) -> u64 {
        let mut deaths = lock(&self.shared.deaths);
        while *deaths <= seen {
            deaths = self
                .shared
                .died
                .wait(deaths)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *deaths
    }

    /// Page deaths so far.
    #[cfg(feature = "gate")]
    pub fn deaths(&self) -> u64 {
        *lock(&self.shared.deaths)
    }

    /// Keeps the page busy until its process is killed, for the simulator
    /// gate: a call begun after this is still in flight in the page when it
    /// dies (where one the page already answered would land as answered).
    /// On the main thread, as every call is begun.
    #[cfg(feature = "gate")]
    pub fn stall(&self) {
        on_main(|_| {
            if let Some(life) = with_page(|page| page.calls.life()) {
                call(life, "stall", 0, "-", "", Box::new(|_| {}));
            }
        });
    }
}

impl ViewRunner for JscRunner {
    fn prepare(
        &self,
        hash: [u8; 32],
        code: ducktape_host::contract::Code<'_>,
        view_bytes: usize,
    ) -> Result<Prepared, Failure> {
        self.wasm()?;
        let view_bytes = u64::try_from(view_bytes).unwrap_or(u64::MAX);
        let (hold, source) = self.shared.code.prepare(
            self.shared.cache.as_deref(),
            &hash,
            code.bytes,
            view_bytes,
        )?;
        Ok(Prepared { source, hold })
    }

    fn instantiate(&self, hash: [u8; 32], seat: SeatId, land: Landing) {
        let Some(code) = self.shared.code.held(&hash) else {
            land.send(Landed::Fault(Fault::Runner(
                "the view's code is not prepared, or nothing holds it any more".into(),
            )));
            return;
        };
        let cache = self.shared.cache.clone();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        on_main(move |mtm| {
            with_page(|page| page.web.attach(mtm));
            seating(Box::new(move |ready| {
                let life = match ready {
                    Ok(life) => life,
                    Err(why) => return land.send(Landed::Fault(Fault::Runner(why))),
                };
                let name = code.name.clone();
                call(
                    life,
                    "seat",
                    id,
                    &name,
                    "",
                    Box::new(move |answered| {
                        let answer = match answered {
                            Ok(text) => answer::read(&text),
                            Err(why) => return land.send(Landed::Fault(Fault::Runner(why))),
                        };
                        let landed = match answer.outcome {
                            Ok(_) => Landed::Seated(Box::new(JscInstance {
                                id,
                                life,
                                hash,
                                seat,
                                land: land.clone(),
                                stats: Arc::new(Stats {
                                    fuel: AtomicU64::new(0),
                                    memory: AtomicUsize::new(answer.memory.unwrap_or(0)),
                                }),
                                _code: code,
                            })),
                            Err(fault) => {
                                if matches!(&fault, Fault::Runner(words) if words.starts_with(NOT_LOADED))
                                {
                                    code.not_loaded(cache.as_deref());
                                }
                                Landed::Fault(fault)
                            }
                        };
                        land.send(landed);
                    }),
                );
            }));
        });
    }

    fn describe(
        &self,
        hash: [u8; 32],
        module: ducktape_host::contract::Code<'_>,
        op: &[u8],
    ) -> Option<view_wire::methods::Description> {
        let module = module.bytes;
        // its answer comes on the main thread: waiting there never ends
        if MainThreadMarker::new().is_some() || self.wasm().is_err() {
            return None;
        }
        let code = lock(&self.shared.described).get(&hash).cloned();
        let code = match code {
            Some(code) => code,
            None => {
                let metered = crate::metered(None, &hash, module, crate::DESCRIBE_MEMORY).ok();
                let code = metered.map(|(bytes, _)| {
                    let name = crate::file_name(&hash, crate::DESCRIBE_MEMORY);
                    Code::new(format!("describe-{name}"), crate::DESCRIBE_MEMORY, bytes)
                });
                lock(&self.shared.described)
                    .entry(hash)
                    .or_insert(code)
                    .clone()
            }
        }?;
        let arg = base64::engine::general_purpose::STANDARD.encode(op);
        let ask = || {
            let (send, answered) = std::sync::mpsc::channel();
            let (code, arg) = (code.clone(), arg.clone());
            on_main(move |_| {
                seating(Box::new(move |ready| match ready {
                    Ok(life) => call(
                        life,
                        "describe",
                        0,
                        &code.name,
                        &arg,
                        Box::new(move |answered| {
                            let _sent = send.send(answered);
                        }),
                    ),
                    Err(why) => {
                        let _sent = send.send(Err(why));
                    }
                }));
            });
            answered.recv().ok()
        };
        let mut answered = ask()?;
        // the page died under the call: asked once more, of the new page
        // (the kernel keeps a `None` for the op for good)
        if matches!(&answered, Err(why) if why == DEAD) {
            answered = ask()?;
        }
        let text = answered.ok()?;
        let bytes = answer::read(&text).outcome.ok()?;
        view_wire::methods::Description::decode(&bytes)
    }
}

/// What the last landed call of an instance used, as its completion left it.
struct Stats {
    fuel: AtomicU64,
    memory: AtomicUsize,
}

/// One seated view: a `WebAssembly.Instance` in the page's life `life`,
/// under `id`.
struct JscInstance {
    id: u64,
    life: u64,
    hash: [u8; 32],
    seat: SeatId,
    land: Landing,
    stats: Arc<Stats>,
    /// The code, held while the instance is, dead or alive (contract:
    /// a revive seats it again by hash).
    _code: Arc<Code>,
}

impl JscInstance {
    fn begin(&self, export: Export, arg: Vec<u8>) {
        let (life, id, land, stats) = (self.life, self.id, self.land.clone(), self.stats.clone());
        let arg = base64::engine::general_purpose::STANDARD.encode(arg);
        on_main(move |_| {
            call(
                life,
                export.op(),
                id,
                "-",
                &arg,
                Box::new(move |answered| {
                    let landed = match answered {
                        Ok(text) => {
                            let answer = answer::read(&text);
                            stats.fuel.store(answer.fuel_used, Ordering::Relaxed);
                            if let Some(memory) = answer.memory {
                                stats.memory.store(memory, Ordering::Relaxed);
                            }
                            export.landed(answer.outcome)
                        }
                        Err(why) => {
                            stats.fuel.store(0, Ordering::Relaxed);
                            Landed::Fault(Fault::Runner(why))
                        }
                    };
                    land.send(landed);
                }),
            );
        });
    }
}

impl ViewInstance for JscInstance {
    fn code_hash(&self) -> [u8; 32] {
        self.hash
    }

    fn begin_init(&mut self) {
        self.begin(Export::Init, Vec::new());
    }

    fn begin_tick(&mut self, events: Vec<u8>) {
        self.begin(Export::Tick, events);
    }

    fn begin_snapshot(&mut self) {
        self.begin(Export::Snapshot, Vec::new());
    }

    fn begin_restore(&mut self, snapshot: Vec<u8>) {
        self.begin(Export::Restore, snapshot);
    }

    fn fuel_used(&self) -> u64 {
        self.stats.fuel.load(Ordering::Relaxed)
    }

    fn memory_bytes(&self) -> usize {
        self.stats.memory.load(Ordering::Relaxed)
    }

    fn stop(self: Box<Self>) {}
}

impl Drop for JscInstance {
    /// The page lets the instance go; posted to the main thread, never
    /// waited for, even there (a drop may come from inside a page callback).
    fn drop(&mut self) {
        let (life, id) = (self.life, self.id);
        tracing::debug!(target: "ducktape::jsc", program = self.seat.0, instance = self.seat.1, id, "view_dropped");
        dispatch2::DispatchQueue::main().exec_async(move || {
            call(life, "drop", id, "-", "", Box::new(|_| {}));
        });
    }
}
