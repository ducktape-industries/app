//! Program-owned views. Every screen the app draws inside its chrome is a
//! wasm view a PROGRAM on the connected network ships (`backend::views`:
//! the roster's code blob, its `ducktape.view` section) — nothing wasm
//! ships beside the binary, and with no node there is no view. A view
//! developer's `DUCKTAPE_VIEWS_DIR` ([`override_views_from`]) is the one
//! exception, and says so in app.log for every view it supplies. A view is
//! ticked inside a fuel and time budget, and presented through native
//! gpui-kit controls in its seat.
//!
//! The app knows no program by name. The roster's order is the rail's, a
//! view's manifest names its tab, and what a view asks of the app is the
//! kernel contract (`kernel`) alone; its props are the session basics every
//! view gets.

mod filesystem;
mod kernel;

pub(crate) use kernel::chord_of;

/// Shared HTTP connections need a continuously driven I/O runtime. Loader
/// threads can compile or join child loads between requests; their own parked
/// runtimes would strand the pooled sockets another loader reuses.
pub(crate) fn runtime() -> tokio::runtime::Handle {
    kernel::runtime()
}

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::editor::wire::EditorStore;
use gpui_kit::AppContext as _;
use pictures::Pictures;
use view_wire as wire;
use wasmtime::component::{Component, Linker, TypedFunc};
use wasmtime::{
    Cache, CacheConfig, Config, Engine, OptLevel, Store, StoreContextMut, StoreLimits,
    StoreLimitsBuilder,
};

/// What a module view asked the app to do: `kind` is the operation
/// (`vote`, `execute`), `detail` the guest's JSON for it.
#[derive(Clone, Debug, Hash, PartialEq, Default)]
pub struct ModuleViewEvent {
    pub kind: String,
    pub detail: String,
}

/// Instruction budget for one call into a view.
const FUEL_PER_TICK: u64 = 100_000_000;
const MEMORY_LIMIT: usize = 64 << 20;
const MAX_MODULE_BYTES: u64 = 64 << 20;
/// A frame the view sends past this ends it: nothing a screen needs is
/// megabytes, and the host would decode all of it on the window thread.
const MAX_FRAME_BYTES: usize = 8 << 20;
const MAX_REQUESTS_PER_TICK: usize = 256;
const MAX_PAYLOAD_BYTES: usize = 1 << 20;
/// The most one op may carry: the kernel bounds nothing, this host does.
const MAX_OP_BYTES: usize = 16 << 20;
/// The gap before the first re-attempt at a candidate whose load failed,
/// and the longest that gap widens to. A load fetches the artifact,
/// instantiates it, restores the snapshot and verifies the first tree —
/// and compiles the component too, whenever `compiled_view` does not
/// already hold it. A deployment that cannot load fails that way every
/// time, so trying it once a block buys nothing and never stops. Widening
/// the gap rather than giving up keeps a fetch that failed on the
/// transport recoverable: nothing is suppressed for good, only spaced out.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);

pub fn event_text(event: &ModuleViewEvent, field: &str) -> String {
    detail(event)
        .and_then(|detail| detail.get(field)?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn detail(event: &ModuleViewEvent) -> Option<serde_json::Value> {
    serde_json::from_str(&event.detail).ok()
}

pub fn event_int(event: &ModuleViewEvent, field: &str) -> i64 {
    detail(event)
        .and_then(|detail| detail.get(field).and_then(serde_json::Value::as_i64))
        .unwrap_or_default()
}

// ---------- the node seat ----------

/// Which module holds each claimed chord, and how many live subscriptions
/// hold it. Global, because the claim is: two seats answering to one chord
/// would make the key mean whichever guest the shell reached first that
/// frame.
///
/// The COUNT is what lets a chord move. A claim is made per subscription and
/// given back per subscription — including the ones a torn-down guest still
/// had — so a swap, in which the replacement claims before the instance it
/// replaces is dropped, leaves the holder standing rather than releasing the
/// key out from under the view that just took it.
type ChordClaims = std::collections::BTreeMap<String, (&'static str, usize)>;

fn chord_claims() -> &'static Mutex<ChordClaims> {
    static CLAIMS: OnceLock<Mutex<ChordClaims>> = OnceLock::new();
    CLAIMS.get_or_init(Mutex::default)
}

/// Claim `chord` for `module`, or name the module that already holds it.
///
/// FIRST COME HOLDS IT, and a module re-claiming its own is another claim on
/// the same key: a swap installs a new instance of the same id, and a view
/// must not lose its chord by being replaced with itself.
pub(crate) fn claim_chord(chord: &str, module: &'static str) -> Result<(), &'static str> {
    let mut claims = chord_claims().lock().expect("chord claims");
    match claims.get_mut(chord) {
        Some((holder, _)) if *holder != module => Err(holder),
        Some((_, held)) => {
            *held += 1;
            Ok(())
        }
        None => {
            claims.insert(chord.to_owned(), (module, 1));
            Ok(())
        }
    }
}

/// Give one claim on `chord` back. The key is free again once the last one
/// is given back — a chord nothing is listening for is a chord the next view
/// may have, and a claim that outlived its guest would make the key
/// unclaimable until the app restarted.
pub(crate) fn release_chord(chord: &str, module: &'static str) {
    let mut claims = chord_claims().lock().expect("chord claims");
    let Some((holder, held)) = claims.get_mut(chord) else {
        return;
    };
    if *holder != module {
        return;
    }
    *held -= 1;
    if *held == 0 {
        claims.remove(chord);
    }
}

/// The module that holds `chord`, if any — what the shell asks before it
/// carries a press to a seat.
pub(crate) fn chord_holder(chord: &str) -> Option<&'static str> {
    chord_claims()
        .lock()
        .expect("chord claims")
        .get(chord)
        .map(|(holder, _)| *holder)
}

pub(crate) fn intern(id: &str) -> &'static str {
    static INTERNED: OnceLock<Mutex<std::collections::BTreeSet<&'static str>>> = OnceLock::new();
    let mut interned = INTERNED
        .get_or_init(Mutex::default)
        .lock()
        .expect("interned ids");
    if let Some(known) = interned.get(id) {
        return known;
    }
    let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
    interned.insert(leaked);
    leaked
}

// ---------- mounting ----------

// ---------- the roster ----------

/// The four facts every view is handed as its props.
pub fn props(dark: bool, connected: bool, network: &str, account: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "connected": connected,
        "dark": dark,
        "chain": network,
        "account": account,
    }))
    .expect("props encode")
}

/// The node the views are asked of, and the network its frames name;
/// `rev` moves on every (re)connect so a load or a request from before it
/// is refused rather than landed on the wrong node.
#[derive(Clone, Default)]
struct Connection {
    client: Option<crate::backend::RpcClient>,
    network: String,
    rev: u64,
}

fn connection() -> &'static Mutex<Connection> {
    static CONNECTION: OnceLock<Mutex<Connection>> = OnceLock::new();
    CONNECTION.get_or_init(Mutex::default)
}

/// The roster as the node last listed it: every program and its code.
fn listed() -> &'static Mutex<Vec<crate::backend::views::Program>> {
    static LISTED: OnceLock<Mutex<Vec<crate::backend::views::Program>>> = OnceLock::new();
    LISTED.get_or_init(Mutex::default)
}

fn listed_code(module: &str) -> Option<abi::BlobId> {
    listed()
        .lock()
        .expect("roster")
        .iter()
        .find(|program| program.name == module)
        .map(|program| program.code)
}

/// A code id as the 32-byte hash a seat records: sha256 as is, sha1 padded.
fn code_digest(code: &abi::BlobId) -> [u8; 32] {
    let mut digest = [0; 32];
    let bytes = code.digest();
    digest[..bytes.len()].copy_from_slice(bytes);
    digest
}

/// The programs the rail lists, in roster order, with the name each one's
/// view gives itself (its program name until the manifest is read) and
/// whether it has a view at all.
pub struct RailRow {
    pub module: &'static str,
    pub label: String,
    /// `Some` while the seat is on its way or failed; `None` once drawn.
    pub note: Option<&'static str>,
    /// The program ships no view: the rail leaves it out.
    pub empty: bool,
}

pub fn rail() -> Vec<RailRow> {
    let programs: Vec<&'static str> = listed()
        .lock()
        .expect("roster")
        .iter()
        .map(|program| intern(&program.name))
        .collect();
    let registry = registry().lock().expect("module views");
    programs
        .into_iter()
        .map(|module| {
            let seat = registry
                .get(module)
                .map(|seat| seat.lock().expect("module view lock"));
            let (label, note, empty) = match seat.as_ref().map(|seat| &seat.slot) {
                Some(Slot::Ready(guest)) if !guest.name.is_empty() => {
                    (guest.name.clone(), None, false)
                }
                Some(Slot::Ready(_)) | None => (module.to_owned(), None, false),
                Some(Slot::Empty) => (module.to_owned(), None, true),
                Some(Slot::Failed(_)) => (module.to_owned(), Some("Failed"), false),
                Some(_) => (module.to_owned(), Some("Loading"), false),
            };
            RailRow {
                module,
                label,
                note,
                empty,
            }
        })
        .collect()
}

/// A node was connected: every seat is asked again of it, and the roster
/// is read so the rail lists what the network runs.
pub fn connected(client: &crate::backend::RpcClient, network: &str) -> Loads {
    let snapshot = {
        let mut connection = connection().lock().expect("views rpc");
        connection.rev += 1;
        connection.client = Some(client.clone());
        connection.network = network.to_owned();
        connection.clone()
    };
    let registry = registry().lock().expect("module views");
    for mounted in registry.values() {
        mounted
            .lock()
            .expect("module view lock")
            .changes
            .send_replace(());
    }
    drop(registry);
    Loads(vec![spawn_roster_read(snapshot)])
}

/// The node moved (a block landed): the roster is read again, and a
/// program whose code changed is loaded again. Cheap when nothing moved.
pub fn deployments_checked() -> Loads {
    static IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Loads(Vec::new());
    }
    let snapshot = connection().lock().expect("views rpc").clone();
    if snapshot.client.is_none() {
        IN_FLIGHT.store(false, Ordering::SeqCst);
        return Loads(Vec::new());
    }
    Loads(vec![std::thread::spawn(move || {
        let read = spawn_roster_read(snapshot);
        let _ = read.join();
        IN_FLIGHT.store(false, Ordering::SeqCst);
    })])
}

fn spawn_roster_read(asked_of: Connection) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(client) = asked_of.client.as_ref() else {
            return;
        };
        let programs =
            match runtime().block_on(crate::backend::views::programs(client, &asked_of.network)) {
                Ok(programs) => programs,
                Err(error) => {
                    tracing::warn!(
                        target: "ducktape::app",
                        reason = "roster_unreadable",
                        error = %error,
                        "the node's programs were not listed"
                    );
                    return;
                }
            };
        let loads = {
            let node_since_left = connection().lock().expect("views rpc").rev != asked_of.rev;
            if node_since_left {
                return;
            }
            let mut registry = registry().lock().expect("module views");
            let names: Vec<&'static str> = programs.iter().map(|p| intern(&p.name)).collect();
            // a program the roster no longer lists gives up its seat
            for gone in registry
                .keys()
                .copied()
                .filter(|seated| !names.contains(seated))
                .collect::<Vec<_>>()
            {
                if let Some(retired) = registry.remove(gone) {
                    let mut retired = retired.lock().expect("module view lock");
                    retired.slot = Slot::Empty;
                    retired.changes.send_replace(());
                }
            }
            let previous =
                std::mem::replace(&mut *listed().lock().expect("roster"), programs.clone());
            let mut loads = Vec::new();
            for (program, module) in programs.iter().zip(names) {
                let seat = registry.entry(module).or_insert_with(Mounted::seat);
                let mut locked = seat.lock().expect("module view lock");
                let same_code = previous
                    .iter()
                    .any(|old| old.name == program.name && old.code == program.code);
                let asked_of_this_node = locked.generation > 0 && locked.rev == asked_of.rev;
                if same_code && asked_of_this_node && !locked.held_off_now() {
                    continue;
                }
                if locked.held_off(Some(code_digest(&program.code))) {
                    continue;
                }
                locked.rev = asked_of.rev;
                let generation = locked.start();
                drop(locked);
                loads.push(spawn_load(module, seat, generation, asked_of.clone()));
            }
            loads
        };
        for load in loads {
            let _ = load.join();
        }
    })
}

/// The loads an event started; nobody waits on them, the seats swap in place.
pub struct Loads(#[allow(dead_code)] Vec<std::thread::JoinHandle<()>>);

/// The node's deployments moved (a new block): every module-owned view
/// whose module's active code is not the one it was drawn from is loaded
/// again, under a new generation, and swapped in place when it is ready.
/// One check in flight at a time; a block that lands during one is
/// covered by the next. The loads it starts swap in place, so nobody waits
/// on them but a test; the block stream drops them.
struct Mounted {
    changes: tokio::sync::watch::Sender<()>,
    /// The connection this seat was last asked of.
    rev: u64,
    slot: Slot,
    props: Option<Vec<u8>>,
    generation: u64,
    /// The deployment the slot answers for — the view drawn, or the empty
    /// slot of a deployment without one — so a block moves it only when the
    /// active code moved. A load that failed never seated anything, so it
    /// leaves this alone and is held off by `retry` instead.
    hash: Option<[u8; 32]>,
    /// A load is on its way for `generation`, and the deployment it is
    /// after when a block named one: a block that names it again waits
    /// for it instead of starting over.
    in_flight: bool,
    /// The proposed frame this device tastes in place of the active one:
    /// the seat's wanted hash is `tasting.or(active)`. Cleared, with a
    /// notice, when the hash leaves the taste set — withdrawn, or
    /// activated into the very hash the seat already draws.
    /// The candidate a load last failed on, and when the next block may try
    /// it again. Cleared by any load that comes back, so only a repeated
    /// failure on the same candidate widens the gap.
    retry: Option<Retry>,
    /// When a tab last drew this seat: a load for a seat on screen does not
    /// queue for the link.
    shown: Option<Instant>,
}

/// A failed load's hold-off: the candidate it failed on, when the next
/// attempt at that same candidate is due, and the gap that produced it.
struct Retry {
    hash: Option<[u8; 32]>,
    next: Instant,
    gap: Duration,
}

impl Retry {
    /// The hold-off after a load for `hash` failed: the gap doubles up to
    /// `RETRY_MAX` while the same candidate keeps failing, and starts over
    /// at `RETRY_FIRST` for a different one.
    fn after(previous: Option<&Retry>, hash: Option<[u8; 32]>) -> Retry {
        let gap = match previous {
            Some(previous) if previous.hash == hash => (previous.gap * 2).min(RETRY_MAX),
            _ => RETRY_FIRST,
        };
        Retry {
            hash,
            next: Instant::now() + gap,
            gap,
        }
    }
}

impl Mounted {
    /// A seat with nothing asked for yet: `Loading` until its source is
    /// asked, under generation 0, which no load answers for.
    fn seat() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            changes: tokio::sync::watch::channel(()).0,
            rev: 0,
            slot: Slot::Loading,
            props: None,
            generation: 0,
            hash: None,
            in_flight: false,
            retry: None,
            shown: None,
        }))
    }

    /// Load `generation`'s stage, shown only while the seat shows a load and
    /// still waits for this one: a view drawn, the network's "none" and a
    /// failure stay up until the load lands.
    fn show(&mut self, generation: u64, stage: Slot) {
        if self.generation == generation && self.slot.loading() {
            self.slot = stage;
        }
    }

    /// A retry is scheduled and not yet due.
    fn held_off_now(&self) -> bool {
        self.retry
            .as_ref()
            .is_some_and(|retry| Instant::now() < retry.next)
    }

    /// Whether a block naming `active` is still inside the hold-off a
    /// failed load for that same candidate left behind.
    fn held_off(&self, active: Option<[u8; 32]>) -> bool {
        self.retry
            .as_ref()
            .is_some_and(|retry| retry.hash == active && Instant::now() < retry.next)
    }

    /// Opens the next generation for a load after `wanted` (None: whatever
    /// the node holds active), and names it.
    fn start(&mut self) -> u64 {
        self.changes.send_replace(());
        self.generation += 1;
        self.in_flight = true;
        self.generation
    }
}

/// What a seat holds, and so what its tab draws: the view on its way —
/// asked, its bytes coming in, verified, compiled — the view itself, the
/// network's word that there is none, or why there is none, named.
enum Slot {
    /// Asked for; nothing back yet.
    Loading,
    /// The artifact's bytes coming in: how many, of how many when the node
    /// said.
    Fetching {
        received: u64,
        total: Option<u64>,
    },
    /// Verified; its code compiles.
    Compiling,
    Ready(Box<Guest>),
    /// The active deployment verified, and it ships no view.
    Empty,
    Failed(Failure),
}

impl Slot {
    /// On its way: one of the stages a load shows before it lands.
    fn loading(&self) -> bool {
        matches!(
            self,
            Slot::Loading | Slot::Fetching { .. } | Slot::Compiling
        )
    }
}

/// Why a seat has no view, by name — the tab says it above the reason and
/// offers Retry. The reason is the load's own sentence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Failure {
    /// The node could not be asked, or does not hold the bytes (yet).
    Unreachable(String),
    /// The bytes do not hash to the code the registry names.
    HashMismatch(String),
    /// This network's registry does not list the view.
    NotListed(String),
    /// The view ran out of fuel or trapped.
    Trapped(String),
    /// Bytes this build does not run as a view, or a load overtaken.
    Refused(String),
    /// The view speaks a wire epoch this app does not.
    WireEpoch(String),
}

/// What a view and the screen hear when the node did not answer — a load that
/// could not ask, or a request that ran out of retries. The transport's own
/// text stays in the log.
pub(crate) const NODE_UNREACHABLE: &str = "The node could not be reached";

impl Failure {
    fn title(&self) -> &'static str {
        match self {
            Failure::Unreachable(_) => NODE_UNREACHABLE,
            Failure::HashMismatch(_) => "The bytes do not match the network's code hash",
            Failure::NotListed(_) => "This network does not list this view",
            Failure::Trapped(_) => "This view stopped",
            Failure::Refused(_) => "This view could not be loaded",
            Failure::WireEpoch(_) => "This view speaks a wire this app does not",
        }
    }
}

impl Failure {
    fn of(error: crate::backend::views::Fetch) -> Failure {
        use crate::backend::views::Fetch;
        let reason = error.to_string();
        match error {
            Fetch::Unreachable(_) | Fetch::NotHeld => Failure::Unreachable(reason),
            Fetch::Corrupt => Failure::HashMismatch(reason),
            Fetch::Refused(_) => Failure::Refused(reason),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Failure::Unreachable(reason)
        | Failure::HashMismatch(reason)
        | Failure::NotListed(reason)
        | Failure::Trapped(reason)
        | Failure::Refused(reason)
        | Failure::WireEpoch(reason)) = self;
        formatter.write_str(reason)
    }
}

type Registry = Mutex<HashMap<&'static str, Arc<Mutex<Mounted>>>>;

fn registry() -> &'static Registry {
    static MOUNTED: OnceLock<Registry> = OnceLock::new();
    MOUNTED.get_or_init(Mutex::default)
}

/// The seat of `module`'s view, made on its first ask. Making it asks for
/// nothing: a load starts at the view's source event, never at a draw, so
/// a seat a tab is first to ask for waits for the next [`connected`] or
/// [`deployments_checked`] like every other.
fn mounted(module: &'static str) -> Arc<Mutex<Mounted>> {
    registry()
        .lock()
        .expect("module views")
        .entry(module)
        .or_insert_with(Mounted::seat)
        .clone()
}

/// Retry, as a failed or stopped tab offers it: the seat's view is asked for
/// again now, under a new generation, past any hold-off a block's retries
/// left. A failure goes back to loading, and a stopped view gives up its
/// seat, so what lands is a fresh instance rather than a swap against it.
pub(crate) fn retry(module: &'static str) -> Loads {
    let registry = registry().lock().expect("module views");
    let Some(seat) = registry.get(module) else {
        return Loads(Vec::new());
    };
    let snapshot = connection().lock().expect("views rpc").clone();
    let mut locked = seat.lock().expect("module view lock");
    let stopped = matches!(&locked.slot, Slot::Ready(guest) if guest.fault.is_some());
    if stopped || matches!(locked.slot, Slot::Failed(_)) {
        locked.slot = Slot::Loading;
        locked.hash = None;
    }
    locked.retry = None;
    let generation = locked.start();
    drop(locked);
    Loads(vec![spawn_load(module, seat, generation, snapshot)])
}

/// Loads the view on its own thread — a cold cranelift compile is a second
/// or more; the window thread shows "Loading" instead of freezing for it —
/// and installs it only if `mounted` still waits for this very load AND,
/// for a network's view, the app is still on the node it was asked of. A
/// view the developer's override supplies is the same on every node: its
/// load lands wherever the app has moved to meanwhile.
fn spawn_load(
    module: &'static str,
    mounted: &Arc<Mutex<Mounted>>,
    generation: u64,
    asked_of: Connection,
) -> std::thread::JoinHandle<()> {
    let loading = mounted.clone();
    std::thread::spawn(move || {
        let loaded = Guest::load(module, &asked_of, generation, &loading);
        let mut locked = loading.lock().expect("module view lock");
        if locked.generation != generation {
            return;
        }
        locked.in_flight = false;
        // the connection lock is a leaf: read under the seat's lock, never
        // across it. A connect bumps the revision before it reaches this
        // seat, so the revision read here is the one the seat installs
        // against — and `connected` reconnects a seat it passed already
        let current_rev = connection().lock().expect("views rpc").rev;
        let from_the_node = view_override(module).is_none();
        let node_since_left = current_rev != asked_of.rev;
        if from_the_node && node_since_left {
            return;
        }
        let Mounted {
            slot, hash, retry, ..
        } = &mut *locked;
        // a load that came back at all clears the hold-off; only the error
        // arm below puts one back, widened against the one taken here
        let held_off = retry.take();
        match loaded {
            Ok(Loaded::Fresh(mut guest)) => {
                guest.installed_generation = Some(generation);
                guest.report_display_truncation();
                *hash = guest.hash;
                *slot = Slot::Ready(guest);
            }
            Ok(Loaded::Unchanged) => {
                if let Slot::Ready(guest) = slot {
                    guest.reconnect(current_rev);
                }
            }
            Ok(Loaded::Empty(removed)) => {
                *hash = Some(removed);
                *slot = Slot::Empty;
            }
            // the same tab, the same surface handle, the same host-side
            // input text and pictures: only the instance behind them moves
            Ok(Loaded::Swap {
                mut fresh,
                alive,
                ticks,
            }) => {
                let Slot::Ready(old) = slot else {
                    log_source(
                        module,
                        fresh.hash.as_ref(),
                        "Failed",
                        generation,
                        "the view left while its replacement was prepared",
                    );
                    return;
                };
                let still_eligible = old.ticks == ticks && old.settled();
                if !Arc::ptr_eq(&old.alive, &alive) || !still_eligible {
                    log_source(
                        module,
                        fresh.hash.as_ref(),
                        "Failed",
                        generation,
                        "the view moved while its replacement was prepared",
                    );
                    return;
                }
                fresh.frame_rev = old.frame_rev + 1;
                if let Some(root) = &fresh.frame.root
                    && let Err(reason) = fresh.inputs.retain_restored_projections(&old.inputs, root)
                {
                    log_source(module, fresh.hash.as_ref(), "Failed", generation, &reason);
                    return;
                }
                if let Some(root) = &mut fresh.frame.root {
                    fresh.pictures.hydrate(root);
                    old.pictures.adopt(root);
                    root.for_each_mut(&mut |node| match node {
                        wire::Node::Svg { bytes, .. } => *bytes = None,
                        wire::Node::Image { data, .. } | wire::Node::ImageViewer { data, .. } => {
                            *data = None
                        }
                        _ => {}
                    });
                }
                fresh.pictures = std::mem::take(&mut old.pictures);
                fresh.installed_generation = Some(generation);
                fresh.report_display_truncation();
                log_source(module, fresh.hash.as_ref(), "Swapped", generation, "");
                *hash = fresh.hash;
                *slot = Slot::Ready(fresh);
            }
            Err(Unloaded {
                hash: failed_on,
                failure,
            }) => {
                tracing::warn!(
                    target: "ducktape::app",
                    module,
                    reason = "module_view_unloadable",
                    error = %failure,
                    "module view not loaded"
                );
                // the next block leaves the candidate this failed on alone
                // until the gap is up; a failure before any candidate (a
                // status or transport error) holds nothing off, and any
                // other deployment is unaffected
                *retry = Some(Retry::after(held_off.as_ref(), failed_on));
                // a view that is there stays, with its hash: the failure is
                // the replacement's, not its own
                if !matches!(slot, Slot::Ready(_)) {
                    *slot = Slot::Failed(failure);
                }
            }
        }
        locked.changes.send_replace(());
    })
}

/// What a load came back with.
enum Loaded {
    /// A view where there was none.
    Fresh(Box<Guest>),
    /// The view already drawn is the active deployment's.
    Unchanged,
    /// A prepared replacement for the view drawn: restored from its
    /// snapshot, its first tree verified; `alive` and `ticks` name the
    /// instance it was prepared against.
    Swap {
        fresh: Box<Guest>,
        alive: Arc<()>,
        ticks: u64,
    },
    /// The deployment (this hash) ships no view.
    Empty([u8; 32]),
}

/// A load that came back with no view, and the candidate it failed on —
/// none when it never got as far as one — so the block check's hold-off
/// keys on the bytes that failed, never on what the load was asked after.
struct Unloaded {
    hash: Option<[u8; 32]>,
    failure: Failure,
}

/// One `view_source` line per outcome, with the same fields every time —
/// the canary greps them.
fn log_source(module: &str, hash: Option<&[u8; 32]>, state: &str, generation: u64, reason: &str) {
    tracing::info!(
        target: "ducktape::app",
        module,
        hash = %hash.map_or_else(|| "-".to_owned(), |hash| crate::backend::hex_encode(hash)),
        state,
        gen = generation,
        reason = %if reason.is_empty() { "-" } else { reason },
        "view_source"
    );
}

/// How long each stage of one module-owned load took: `status` and
/// `fetch` are the node's answers, `compile` is cranelift, `init` the
/// instance and its `on mount` or restore, `first_frame` the tree a
/// replacement proves (a fresh view draws its first on the window thread),
/// `check` the second look at the registry before the seat. `path` is
/// `first` for a load over an empty slot, `swap` over a view drawn.
#[derive(Default)]
struct LoadTiming {
    path: &'static str,
    status: Duration,
    fetch: Duration,
    compile: Duration,
    init: Duration,
    first_frame: Option<Duration>,
    check: Duration,
}

impl LoadTiming {
    /// One `view_load` line per load, every field every time, through the
    /// same logger and test tap as `view_source`.
    fn log(&self, module: &str, hash: Option<&[u8; 32]>, started: Instant, state: &str) {
        let ms = |duration: Duration| duration.as_millis();
        let hash = hash.map_or_else(|| "-".to_owned(), |hash| crate::backend::hex_encode(hash));
        let first_frame = self
            .first_frame
            .map_or_else(|| "-".to_owned(), |frame| ms(frame).to_string());
        let total = ms(started.elapsed());
        tracing::info!(
            target: "ducktape::app",
            module,
            hash = %hash,
            path = self.path,
            outcome = state,
            status_ms = ms(self.status),
            fetch_ms = ms(self.fetch),
            compile_ms = ms(self.compile),
            init_ms = ms(self.init),
            first_frame_ms = %first_frame,
            check_ms = ms(self.check),
            total_ms = total,
            "view_load"
        );
    }
}

/// A VIEW DEVELOPER'S OVERRIDE, and nothing else: the directory `main`
/// reads out of `DUCKTAPE_VIEWS_DIR`. While it is set, `<id>_view.wasm` in
/// it is loaded for any view the app seats, in place of the network's
/// bytes — unverified, and said so in app.log each time it is loaded.
static VIEW_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets (or, with `None`, clears) the developer's override directory.
pub fn override_views_from(dir: Option<PathBuf>) {
    *VIEW_OVERRIDE.lock().expect("view override") = dir;
}

/// The file the developer's override supplies for `module`, if any.
fn view_override(module: &str) -> Option<PathBuf> {
    let dir = VIEW_OVERRIDE.lock().expect("view override").clone()?;
    let path = dir.join(format!("{module}_view.wasm"));
    path.is_file().then_some(path)
}

// ---------- the guest ----------

/// What a view's store holds: its limits, and the message its panic hook
/// handed over before the trap that follows.
struct HostState {
    limits: StoreLimits,
    panic: Option<String>,
}

/// The guest's `restore(state, macos)` export.
type Restore = TypedFunc<(Vec<u8>, bool), (Result<(), String>,)>;

/// What a fresh instance did with the state the drawn view left it. A trap
/// is not one of these — it takes the instance with it and is the load's
/// error. A refusal is the guest's own word, reported before it builds
/// anything, so the instance is untouched and can start clean instead.
enum Restored {
    Carried,
    Refused(String),
}

struct Guest {
    /// Node requests belong to the network selected when this instance starts.
    connection_rev: u64,
    user_activation: Option<()>,
    module: &'static str,
    /// The manifest's name: what a registered view's tab is called.
    name: String,
    store: Store<HostState>,
    tick: TypedFunc<(Vec<u8>,), (Vec<u8>,)>,
    /// The guest's events for its next tick.
    pending: Vec<wire::Event>,
    /// Requests wait for the native layout of a frame that still mounts
    /// their target, inside this instance only.
    widget_commands: Vec<(u64, wire::WidgetCommand)>,
    /// The last frame, its `root` kept across `unchanged` ticks and patched
    /// in place by a frame that carries patches instead of a tree.
    frame: wire::Frame,
    frame_reports: display_diagnostics::FrameReports,
    display_diagnostics: display_diagnostics::DisplayDiagnostics,
    installed_generation: Option<u64>,
    /// Bumped when `frame.root` changes: the widget rebuilds when it sees a
    /// number it has not rendered.
    frame_rev: u64,
    ticks: u64,
    /// The live text of every input in the tree — the host's, not the guest's.
    inputs: EditorStore,
    /// Every picture the guest has sent, by hash: the bytes cross once.
    pictures: Pictures,
    /// The guest's `<module>.props` subscription, once it asked, and the
    /// props it was last given on it.
    props_subscription: Option<u64>,
    props_sent: Option<Vec<u8>>,
    visible: bool,
    visibility_change: Option<bool>,
    visibility_subscriptions: Vec<u64>,
    /// What the guest asked the app to do this redraw.
    intents: Vec<ModuleViewEvent>,
    /// The kernel's answers to this guest's node calls, on their way in.
    replies: Arc<kernel::Replies>,
    /// The guest's `rpc.live` subscriptions, each with the plane it named:
    /// told on every block that moves that plane.
    live_subscriptions: Vec<(u64, String)>,
    /// Pending host requests and subscriptions, each owned by this guest
    /// the kernel opened for it: retired with the cancel, and with the guest.
    tasks: Vec<(u64, kernel::NodeTask)>,
    filesystem: filesystem::Filesystem,
    /// The guest's `clock.ticks` subscriptions: the period it asked for and
    /// the instant its next item is due. A module has no clock of its own,
    /// so periodic guest subscriptions use this list — driven from the window
    /// thread's own redraw, never from a thread that would have to wake it.
    clocks: Vec<kernel::Clock>,
    /// The chords this guest claimed, each with the subscription its presses
    /// arrive on. The claim itself is global ([`claim_chord`]): two seats
    /// cannot answer to one chord.
    chords: Vec<(u64, String)>,
    /// The trap that ended the view, if one did. A faulted guest never ticks again.
    fault: Option<String>,
    /// The assets the deployment shipped beside this view, for the host
    /// surfaces that paint them by canonical relative path; swapped with the
    /// instance as one unit. Empty for a view the developer's override supplies.
    /// The deployment this instance came from; none for a file the
    /// developer's override supplies.
    hash: Option<[u8; 32]>,
    /// This instance's identity: a replacement prepared against it is
    /// installed only over it.
    alive: Arc<()>,
    /// A replacement's first tree is in `frame` with its requests still
    /// to dispatch — the first redraw does that, without another tick.
    staged: bool,
    snapshot: TypedFunc<(), (Result<Vec<u8>, String>,)>,
    restore: Restore,
    init: TypedFunc<(bool,), ()>,
}

/// The asset at `path` in a deployment's map: the canonical relative path,
/// exactly — no normalisation, no file, no network.
fn hex_short(hash: &[u8; 32]) -> String {
    hash[..6].iter().map(|byte| format!("{byte:02x}")).collect()
}

const COMPILED_VIEW_LIMIT: usize = 16;
const COMPILED_VIEW_SOURCE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct ViewCodeCache {
    entries: VecDeque<CompiledView>,
}

struct CompiledView {
    hash: [u8; 32],
    source_bytes: usize,
    component: Arc<Component>,
}

impl ViewCodeCache {
    fn get(&mut self, hash: &[u8; 32]) -> Option<Arc<Component>> {
        let index = self.entries.iter().position(|entry| &entry.hash == hash)?;
        let entry = self.entries.remove(index)?;
        let component = entry.component.clone();
        self.entries.push_back(entry);
        Some(component)
    }

    fn insert(&mut self, entry: CompiledView) {
        if entry.source_bytes > COMPILED_VIEW_SOURCE_BYTES {
            return;
        }
        let mut bytes: usize = self.entries.iter().map(|entry| entry.source_bytes).sum();
        loop {
            let fits = self.entries.len() < COMPILED_VIEW_LIMIT
                && bytes + entry.source_bytes <= COMPILED_VIEW_SOURCE_BYTES;
            if fits {
                break;
            }
            let Some(old) = self.entries.pop_front() else {
                break;
            };
            bytes -= old.source_bytes;
        }
        self.entries.push_back(entry);
    }
}

fn compiled_view(bytes: &[u8]) -> Result<Arc<Component>, String> {
    static CODE: OnceLock<Mutex<ViewCodeCache>> = OnceLock::new();
    compile_view(engine(), CODE.get_or_init(Mutex::default), bytes)
}

// Cache code only: every load still creates its own Store, instance and assets.
// The cache belongs to this one Engine. Compile outside the lock so unrelated
// views can prepare concurrently; a competing result adopts the existing entry.
fn compile_view(
    engine: &Engine,
    cache: &Mutex<ViewCodeCache>,
    bytes: &[u8],
) -> Result<Arc<Component>, String> {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes).into();
    if let Some(component) = cache.lock().expect("view code cache").get(&hash) {
        return Ok(component);
    }
    let component = Arc::new(Component::new(engine, bytes).map_err(|error| error.to_string())?);
    let mut cache = cache.lock().expect("view code cache");
    if let Some(existing) = cache.get(&hash) {
        return Ok(existing);
    }
    cache.insert(CompiledView {
        hash,
        source_bytes: bytes.len(),
        component: component.clone(),
    });
    Ok(component)
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.cranelift_opt_level(OptLevel::Speed);
        config.consume_fuel(true);
        match crate::backend::cache_dir() {
            Ok(directory) => {
                let mut cache = CacheConfig::new();
                cache.with_directory(directory.join("view-code"));
                match Cache::new(cache) {
                    Ok(cache) => {
                        config.cache(Some(cache));
                    }
                    Err(error) => tracing::warn!(reason = "view_cache_unavailable", %error),
                }
            }
            Err(error) => tracing::warn!(reason = "view_cache_directory_unavailable", %error),
        }
        Engine::new(&config).expect("wasmtime engine")
    })
}

/// Reset the instruction allowance before entering the guest.
fn arm(store: &mut Store<HostState>) {
    let _ = store.set_fuel(FUEL_PER_TICK);
}

/// EVERY CHORD LEAVES WITH THE GUEST. The claim table is global and outlives
/// any one instance, so an instance that is retired, replaced, or ended by a
/// trap and kept its chords would make them unclaimable until the app
/// restarted — including by the view that takes its seat. This is the one
/// place that covers all three, because all three end with the box dropped.
impl Drop for Guest {
    fn drop(&mut self) {
        for (_, chord) in &self.chords {
            release_chord(chord, self.module);
        }
    }
}

impl Guest {
    /// Reusing identical code still retires work started on the old connection.
    fn reconnect(&mut self, revision: u64) {
        if self.connection_rev == revision {
            return;
        }
        let ids: Vec<_> = self.tasks.iter().map(|(id, _)| *id).collect();
        self.tasks.clear();
        self.filesystem = Default::default();
        for id in ids {
            self.refuse(id, "stale_connection", "network connection changed");
        }
        self.connection_rev = revision;
    }

    /// A view comes from its registry entry's active deployment on the
    /// connected node — nothing else, so with no node there is nothing to
    /// load yet, and no file is ever opened for it unless the developer's
    /// override ([`override_views_from`]) supplies one. With a view of the module already
    /// drawn, the deployment's view is prepared as its replacement:
    /// instantiated without `init`, restored from the drawn view's snapshot,
    /// and its first tree verified — and the active code is read again
    /// right before it is handed over, so a deployment that moved meanwhile
    /// is not installed. Every outcome for a module-owned view is one
    /// `view_source` log line with stable fields.
    fn load(
        module: &'static str,
        asked_of: &Connection,
        generation: u64,
        mounted: &Arc<Mutex<Mounted>>,
    ) -> Result<Loaded, Unloaded> {
        let before_any_candidate = |failure: Failure| Unloaded {
            hash: None,
            failure,
        };
        let logged = |hash: Option<&[u8; 32]>, state: &str, reason: &str| {
            log_source(module, hash, state, generation, reason);
        };
        if let Some(path) = view_override(module) {
            let reason = format!("DUCKTAPE_VIEWS_DIR developer override: {}", path.display());
            logged(None, "Overridden", &reason);
            return Self::load_from(module, &path)
                .map(|guest| Loaded::Fresh(Box::new(guest)))
                .map_err(|reason| before_any_candidate(Failure::Refused(reason)));
        }
        let Some(client) = asked_of.client.as_ref() else {
            let reason = "not connected to a node yet";
            logged(None, "Failed", reason);
            return Err(before_any_candidate(Failure::Unreachable(
                reason.to_owned(),
            )));
        };
        let Some(code) = listed_code(module) else {
            let reason = format!("this network's roster does not list {module}");
            logged(None, "Failed", &reason);
            return Err(before_any_candidate(Failure::NotListed(reason)));
        };
        let runtime = runtime();
        let started = Instant::now();
        let mut timing = LoadTiming {
            path: "first",
            ..LoadTiming::default()
        };
        let show = |stage: Slot| {
            mounted
                .lock()
                .expect("module view lock")
                .show(generation, stage)
        };
        show(Slot::Fetching {
            received: 0,
            total: None,
        });
        let fetched = Instant::now();
        let bytes = runtime.block_on(crate::backend::views::view_of(client, &code));
        timing.fetch = fetched.elapsed();
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                let failure = Failure::of(error);
                logged(None, "Failed", &failure.to_string());
                timing.log(module, None, started, "Failed");
                return Err(before_any_candidate(failure));
            }
        };
        let Some(component_bytes) = bytes else {
            let hash = code_digest(&code);
            logged(Some(&hash), "Missing", "");
            timing.log(module, Some(&hash), started, "Missing");
            return Ok(Loaded::Empty(hash));
        };
        let hash: [u8; 32] = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&component_bytes).into()
        };
        let shown = format!("{module} view @ {}", hex_short(&hash));
        let outcome = (|| -> Result<Loaded, Failure> {
            // the instance in the slot, if the deployment is a new one for
            // it: the replacement is seated only against that very
            // instance at that very tick count
            let mut against = {
                let locked = mounted.lock().expect("module view lock");
                match &locked.slot {
                    Slot::Ready(old) if old.hash == Some(hash) => return Ok(Loaded::Unchanged),
                    Slot::Ready(old) => Some((old.alive.clone(), old.ticks)),
                    _ => None,
                }
            };
            if against.is_some() {
                timing.path = "swap";
            }
            show(Slot::Compiling);
            let compiled = Instant::now();
            let component = Self::compile(&component_bytes, &shown);
            timing.compile = compiled.elapsed();
            let component = component?;
            let seated = Instant::now();
            let name = manifest_name(&component_bytes);
            let prepared = (|| -> Result<Self, Failure> {
                let mut fresh =
                    Self::instantiate(module, &component, &shown).map_err(Failure::Refused)?;
                fresh.name = name.clone();
                fresh.deployed(hash);
                match &mut against {
                    // A once-valid view carries its state over. Only an
                    // explicitly admitted never-valid recovery may initialize.
                    Some((alive, ticks)) => {
                        let snapshot = {
                            let mut locked = mounted.lock().expect("module view lock");
                            let Slot::Ready(old) = &mut locked.slot else {
                                return Err(Failure::Refused(
                                    "the view left while its replacement was prepared".into(),
                                ));
                            };
                            if !Arc::ptr_eq(alive, &old.alive) {
                                return Err(Failure::Refused(
                                    "the view changed while its replacement was prepared".into(),
                                ));
                            }
                            *ticks = old.ticks;
                            let preserve = *ticks > 0;
                            if preserve && !old.settled() {
                                return Err(Failure::Refused(
                                    "the view has pending work; its replacement waits".into(),
                                ));
                            }
                            if preserve {
                                Some(old.snapshot().map_err(Failure::Trapped)?)
                            } else {
                                None
                            }
                        };
                        match snapshot {
                            Some(snapshot) => {
                                wire::Snapshot::decode(&snapshot).map_err(Failure::Refused)?;
                                match fresh.restore(&snapshot, &shown).map_err(Failure::Trapped)? {
                                    Restored::Carried => {}
                                    Restored::Refused(refusal) => {
                                        tracing::warn!(
                                            target: "ducktape::app",
                                            module,
                                            hash = %crate::backend::hex_encode(&hash),
                                            reason = "snapshot_refused",
                                            refusal = %refusal,
                                            "view_state_dropped"
                                        );
                                        fresh.init(&shown).map_err(Failure::Trapped)?;
                                    }
                                }
                            }
                            None => fresh.init(&shown).map_err(Failure::Trapped)?,
                        }
                        let framed = Instant::now();
                        let frame = fresh.first_frame(&shown);
                        timing.first_frame = Some(framed.elapsed());
                        frame.map_err(Failure::Trapped)?;
                    }
                    None => {
                        fresh.init(&shown).map_err(Failure::Trapped)?;
                    }
                }
                Ok(fresh)
            })();
            timing.init = seated
                .elapsed()
                .saturating_sub(timing.first_frame.unwrap_or_default());
            let fresh = prepared?;
            Ok(match against {
                Some((alive, ticks)) => Loaded::Swap {
                    fresh: Box::new(fresh),
                    alive,
                    ticks,
                },
                None => Loaded::Fresh(Box::new(fresh)),
            })
        })();
        let state = match &outcome {
            Ok(Loaded::Fresh(_)) => {
                logged(Some(&hash), "Ready", "");
                "Ready"
            }
            Ok(Loaded::Unchanged) => "Unchanged",
            Ok(_) => "Swap",
            Err(failure) => {
                logged(Some(&hash), "Failed", &failure.to_string());
                "Failed"
            }
        };
        timing.log(module, Some(&hash), started, state);
        outcome.map_err(|failure| Unloaded {
            hash: Some(hash),
            failure,
        })
    }

    fn deployed(&mut self, hash: [u8; 32]) {
        self.hash = Some(hash);
    }

    /// Candidate attempts do not retire a seated view or its input routes.
    /// Direct test fixtures have no loader-assigned generation and use zero.
    fn seated_generation(&self) -> u64 {
        self.installed_generation.unwrap_or_default()
    }

    /// Everything this instance was asked to do is done: nothing pending,
    /// no request the host has yet to route, no trap. A replacement not
    /// yet redrawn is settled too: the only requests its first tree
    /// carries are the subscriptions its restore rebuilt, which its own
    /// replacement rebuilds again — a tab not shown between two
    /// deployments is not stuck on the first.
    fn settled(&self) -> bool {
        self.fault.is_none()
            && self.replies.fault().is_none()
            && self.pending.is_empty()
            && self.widget_commands.is_empty()
            && self.inputs.ready() == Ok(true)
            && !self.inputs.pending()
            && !self.frame.busy
            && (self.staged || self.frame.requests.is_empty())
    }

    fn snapshot(&mut self) -> Result<Vec<u8>, String> {
        arm(&mut self.store);
        self.snapshot
            .call(&mut self.store, ())
            .map_err(|error| first_line(&error))?
            .0
    }

    fn restore(&mut self, snapshot: &[u8], shown: &str) -> Result<Restored, String> {
        arm(&mut self.store);
        let answered = self
            .restore
            .call(
                &mut self.store,
                (snapshot.to_vec(), cfg!(target_os = "macos")),
            )
            .map_err(|error| format!("{shown}: restore trapped: {}", first_line(&error)))?
            .0;
        Ok(match answered {
            Ok(()) => Restored::Carried,
            Err(refusal) => Restored::Refused(refusal),
        })
    }

    /// `on mount` runs in here, told which platform it keys for.
    fn init(&mut self, shown: &str) -> Result<(), String> {
        arm(&mut self.store);
        if let Err(error) = self
            .init
            .call(&mut self.store, (cfg!(target_os = "macos"),))
        {
            let trap = format!("{shown}: init trapped: {}", first_line(&error));
            return Err(panic_message(&mut self.store).unwrap_or(trap));
        }
        Ok(())
    }

    /// A restored instance's first tick: its whole tree, or it is no
    /// replacement. Its requests wait for the first redraw.
    fn first_frame(&mut self, shown: &str) -> Result<(), String> {
        let mut requests = Vec::new();
        let mut cancels = Vec::new();
        let limit = wire::editor_document::MAX_EDITOR_DOCUMENTS
            * (wire::editor_document::MAX_EDITOR_CHUNKS + 3)
            + 1;
        for _ in 0..limit {
            self.tick();
            #[cfg(test)]
            if let Some(fault) = &self.fault {
                return Err(format!("{shown}: {fault}"));
            }
            if self.frame.root.is_none() {
                return Err(format!(
                    "{shown}: the replacement did not publish a complete tree"
                ));
            }
            requests.append(&mut self.frame.requests);
            cancels.append(&mut self.frame.cancels);
            if requests.len() > MAX_REQUESTS_PER_TICK || cancels.len() > 2 * MAX_REQUESTS_PER_TICK {
                return Err(format!(
                    "{shown}: replacement requests exceed the first-frame budget"
                ));
            }
            if self.inputs.ready()? && self.pending.is_empty() {
                break;
            }
        }
        if !self.inputs.ready()? || !self.pending.is_empty() {
            return Err(format!(
                "{shown}: replacement document transfer did not complete"
            ));
        }
        requests.retain(|request| !cancels.contains(&request.id));
        self.frame.requests = requests;
        self.frame.cancels = cancels;
        self.ticks += 1;
        self.staged = true;
        Ok(())
    }

    fn load_from(module: &'static str, path: &std::path::Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        let metadata = std::fs::metadata(path).map_err(|error| format!("{shown}: {error}"))?;
        if metadata.len() > MAX_MODULE_BYTES {
            return Err(format!(
                "{shown}: past the {MAX_MODULE_BYTES} byte module limit"
            ));
        }
        let bytes = std::fs::read(path).map_err(|error| format!("{shown}: {error}"))?;
        Self::from_bytes(module, &bytes, &shown)
    }

    /// The component instantiated and mounted; `shown` names it in errors.
    fn from_bytes(module: &'static str, bytes: &[u8], shown: &str) -> Result<Self, String> {
        let component = Self::compile(bytes, shown).map_err(|failure| failure.to_string())?;
        let mut guest = Self::instantiate(module, &component, shown)?;
        guest.name = manifest_name(bytes);
        guest.init(shown)?;
        Ok(guest)
    }

    /// The component's bytes checked and compiled — the cranelift stage of
    /// a load, measured on its own.
    fn compile(bytes: &[u8], shown: &str) -> Result<Arc<Component>, Failure> {
        if bytes.len() as u64 > MAX_MODULE_BYTES {
            return Err(Failure::Refused(format!(
                "{shown}: past the {MAX_MODULE_BYTES} byte module limit"
            )));
        }
        // its preferred size is for placing a new window; the tab embeds
        let manifest = view_wire::manifest::read_manifest(bytes).ok_or_else(|| {
            Failure::Refused(format!("{shown}: the component's manifest cannot be read"))
        })?;
        wire_epoch(manifest.wire_epoch)
            .map_err(|error| Failure::WireEpoch(format!("{shown}: {error}")))?;
        let component =
            compiled_view(bytes).map_err(|error| Failure::Refused(format!("{shown}: {error}")))?;
        Ok(component)
    }

    /// The component instantiated, its exports bound, nothing run yet: a
    /// fresh view is `init`ed, a replacement `restore`d.
    fn instantiate(
        module: &'static str,
        component: &Component,
        shown: &str,
    ) -> Result<Self, String> {
        let engine = engine();
        // Tables are allocated eagerly at their declared minimum, before any
        // fuel or memory limit is consulted; a component is several core
        // instances — the guest and its component bindings — and one memory.
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .memories(1)
            .instances(8)
            .tables(4)
            .table_elements(1 << 20)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(
            engine,
            HostState {
                limits,
                panic: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        // The `ducktape:view` world's one import is the panic hook's; anything
        // else the component asks for traps if it is ever called.
        let mut linker = Linker::<HostState>::new(engine);
        linker
            .root()
            .func_wrap(
                "panicked",
                |mut store: StoreContextMut<'_, HostState>, (message,): (String,)| {
                    let line = message.lines().next().unwrap_or_default();
                    store.data_mut().panic = Some(line.chars().take(1024).collect());
                    Ok(())
                },
            )
            .map_err(|error| error.to_string())?;
        linker
            .define_unknown_imports_as_traps(component)
            .map_err(|error| error.to_string())?;
        arm(&mut store);
        let instance = linker
            .instantiate(&mut store, component)
            .map_err(|error| format!("{shown}: {}", first_line(&error)))?;
        let init = instance
            .get_typed_func::<(bool,), ()>(&mut store, "init")
            .map_err(|error| format!("{shown}: {error}"))?;
        let tick = instance
            .get_typed_func::<(Vec<u8>,), (Vec<u8>,)>(&mut store, "tick")
            .map_err(|error| format!("{shown}: {error}"))?;
        let snapshot = instance
            .get_typed_func::<(), (Result<Vec<u8>, String>,)>(&mut store, "snapshot")
            .map_err(|error| format!("{shown}: {error}"))?;
        let restore = instance
            .get_typed_func::<(Vec<u8>, bool), (Result<(), String>,)>(&mut store, "restore")
            .map_err(|error| format!("{shown}: {error}"))?;
        Ok(Self {
            connection_rev: connection().lock().expect("views rpc").rev,
            user_activation: None,
            module,
            name: String::new(),
            store,
            tick,
            pending: Vec::new(),
            widget_commands: Vec::new(),
            frame: wire::Frame::default(),
            frame_reports: Default::default(),
            display_diagnostics: Default::default(),
            installed_generation: None,
            frame_rev: 0,
            ticks: 0,
            inputs: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                EditorStore::new(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            },
            pictures: Pictures::default(),
            props_subscription: None,
            props_sent: None,
            visible: false,
            visibility_change: None,
            visibility_subscriptions: Vec::new(),
            intents: Vec::new(),
            replies: Arc::default(),
            live_subscriptions: Vec::new(),
            tasks: Vec::new(),
            filesystem: Default::default(),
            clocks: Vec::new(),
            chords: Vec::new(),
            fault: None,
            hash: None,
            alive: Arc::new(()),
            staged: false,
            snapshot,
            restore,
            init,
        })
    }

    /// Hands the guest the props the app holds, if they moved since the
    /// guest last saw them. Before the guest has subscribed they wait here.
    fn sync_props(&mut self, props: &Option<Vec<u8>>) {
        let Some(id) = self.props_subscription else {
            return;
        };
        if props.is_none() || *props == self.props_sent {
            return;
        }
        self.props_sent = props.clone();
        let props = props.clone().unwrap_or_default();
        self.pending.push(wire::Event::Response {
            id,
            result: Ok(props),
            done: false,
        });
    }

    fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.visibility_change = Some(visible);
    }

    /// A chord this guest claimed was pressed: every subscription that named
    /// it gets one item. Says whether any did, which is how the shell knows
    /// the press was spent and must not also be a native key.
    fn chord_pressed(&mut self, chord: &str) -> bool {
        let claimed: Vec<u64> = self
            .chords
            .iter()
            .filter(|(_, named)| named == chord)
            .map(|(id, _)| *id)
            .collect();
        for id in &claimed {
            self.pending.push(wire::Event::Response {
                id: *id,
                result: Ok(Vec::new()),
                done: false,
            });
        }
        !claimed.is_empty()
    }

    fn sync_visibility(&mut self) {
        let Some(visible) = self.visibility_change.take() else {
            return;
        };
        for id in &self.visibility_subscriptions {
            self.pending.push(wire::Event::Response {
                id: *id,
                result: Ok(visible.to_string().into_bytes()),
                done: false,
            });
        }
    }

    /// One redraw: tick if there is anything to deliver — or never was a
    /// first frame — answer the requests, and say whether the guest is due
    /// again at once. A guest with nothing to deliver is left alone: the
    /// tree the host has is the tree it would send.
    fn redraw(&mut self, props: &Option<Vec<u8>>) -> bool {
        if self.fault.is_some() {
            return false;
        }
        self.sync_props(props);
        self.pending.extend(self.inputs.drain());
        if let Err(error) = self.inputs.ready() {
            self.fault = Some(error);
            return false;
        }
        if let Err(error) = self.replies.drain_into(&mut self.pending) {
            self.fault = Some(error);
            return false;
        }
        self.pending
            .extend(kernel::ticked(&mut self.clocks, std::time::Instant::now()));
        // Queued results precede becoming visible, so a view can distinguish
        // arrivals while hidden from data received after it returns to screen.
        self.sync_visibility();
        if self.staged {
            // a replacement's first tree is already here; only its
            // requests and cancels are still to route
            self.staged = false;
        } else {
            let quiet = self.ticks > 0
                && !self.frame.busy
                && self.pending.is_empty()
                && !self.inputs.pending()
                && self.inputs.ready() != Ok(false);
            if quiet {
                return false;
            }
            self.tick();
            self.ticks += 1;
        }
        for (nth, request) in std::mem::take(&mut self.frame.requests)
            .into_iter()
            .enumerate()
        {
            match nth < MAX_REQUESTS_PER_TICK {
                true => self.answer(request, props),
                false => self.refuse(request.id, "tick_limit", "too many requests this tick"),
            }
        }
        self.user_activation = None;
        for id in std::mem::take(&mut self.frame.cancels) {
            self.filesystem.cancel(id);
            self.widget_commands.retain(|(request, _)| *request != id);
            if self.props_subscription == Some(id) {
                self.props_subscription = None;
            }
            self.live_subscriptions.retain(|(live, _)| *live != id);
            self.visibility_subscriptions
                .retain(|subscription| *subscription != id);
            // dropping the stream aborts it: the node socket goes with the
            // subscription the view abandoned
            self.tasks.retain(|(task, _)| *task != id);
            self.clocks.retain(|clock| clock.id != id);
            // a chord is given back with the subscription its presses were
            // arriving on, or the next view could never claim it
            let dropped: Vec<String> = self
                .chords
                .iter()
                .filter(|(subscription, _)| *subscription == id)
                .map(|(_, chord)| chord.clone())
                .collect();
            self.chords.retain(|(subscription, _)| *subscription != id);
            for chord in dropped {
                release_chord(&chord, self.module);
            }
        }
        self.fault.is_none()
            && (self.frame.busy
                || self.inputs.pending()
                || !self.pending.is_empty()
                || self.inputs.ready() == Ok(false))
    }

    /// Routes one request: the props subscription is answered from what the
    /// app holds, an intent the module declares goes to the app, the log
    /// goes to the log, and anything else is refused.
    fn answer(&mut self, request: wire::Request, props: &Option<Vec<u8>>) {
        let wire::Request { id, kind, payload } = request;
        let payload_limit = match kind.as_str() {
            "op.submit_bytes" => MAX_OP_BYTES.div_ceil(3) * 4 + 256,
            _ => MAX_PAYLOAD_BYTES,
        };
        if payload.len() > payload_limit {
            self.refuse(
                id,
                "too_large",
                format!("`{kind}` carries more than {payload_limit} bytes"),
            );
            return;
        }
        let (capability, operation) = kind.split_once('.').unwrap_or((kind.as_str(), ""));
        // the kernel contract first: what every view may ask, module-free
        if kernel::answer(self, capability, operation, id, &payload) {
            return;
        }
        let own = capability == self.module;
        match (capability, operation) {
            ("host", "widget") => self.widget_request(id, &payload),
            _ if own && operation == "props" => {
                self.props_subscription = Some(id);
                self.props_sent = None;
                self.sync_props(props);
            }
            ("host", "log") => {
                tracing::debug!(
                    target: "ducktape::app",
                    module = self.module,
                    line = %String::from_utf8_lossy(&payload),
                    "module view log"
                );
                self.reply(id, Ok(Vec::new()));
            }
            _ => self.refuse(id, "unknown_request", format!("unknown request `{kind}`")),
        }
    }

    /// The host's own refusal, in the shape a guest gets a node's: a stable
    /// snake_case token it may branch on, and the sentence it may show.
    fn refuse(&mut self, id: u64, reason: &'static str, message: impl Into<String>) {
        self.reply(id, Err(wire::Refusal::new(reason, message)));
    }

    /// The key a command acts on, or `None` for the two that act on focus
    /// order rather than a node. Exhaustive by design: adding a command
    /// requires reviewing its scope.
    fn command_target(command: &wire::WidgetCommand) -> Option<&str> {
        use wire::WidgetCommand as C;
        match command {
            C::FocusPrevious | C::FocusNext => None,
            C::EditorAction { target, .. }
            | C::Focus { target }
            | C::Focused { target }
            | C::CursorFront { target }
            | C::CursorEnd { target }
            | C::Cursor { target, .. }
            | C::SelectAll { target }
            | C::Select { target, .. }
            | C::Snap { target, .. }
            | C::SnapEnd { target }
            | C::ScrollTo { target, .. }
            | C::ScrollToKey { target, .. }
            | C::ScrollBy { target, .. } => Some(target),
        }
    }

    /// Whether the key this command names is in the tree the guest is
    /// showing RIGHT NOW. A press is answered a frame or more after it was
    /// made, and a live view replaces its frame between the two — so the
    /// question a queued command has to pass is whether its target is still
    /// there, never whether the frame it was made against is still the
    /// current one.
    fn target_is_mounted(&self, command: &wire::WidgetCommand) -> bool {
        fn contains(node: &wire::Node, target: &str) -> bool {
            node.key() == Some(target)
                || node.children().iter().any(|child| contains(child, target))
        }
        let Some(target) = Self::command_target(command) else {
            return true;
        };
        self.frame
            .root
            .as_ref()
            .is_some_and(|root| contains(root, target))
    }

    fn widget_request(&mut self, id: u64, payload: &[u8]) {
        let admitted = (|| -> Result<wire::WidgetCommand, String> {
            let mut command: wire::WidgetCommand = wire::decode(payload)?;
            let exact_payload = wire::encoded_size(&command) == payload.len() as u64;
            if !exact_payload {
                return Err("widget request has trailing bytes".into());
            }
            command.validate()?;
            let queue_full = self.widget_commands.len() >= MAX_REQUESTS_PER_TICK;
            if queue_full {
                return Err("too many pending widget requests".into());
            }
            if !self.target_is_mounted(&command) {
                return Err("widget target is outside this guest tree".into());
            }
            Ok(command)
        })();
        match admitted {
            Ok(command) => self.widget_commands.push((id, command)),
            Err(error) => self.refuse(id, "invalid_widget_command", error),
        }
    }

    /// How many of the queued widget commands can run now. They wait for
    /// native editor work to drain, since a caret or a selection is placed in
    /// the document as it stands. A Focus does not wait: a field the view
    /// just opened has to hold the keys typed into it before its document
    /// arrives. A rich editor's Focus still waits — it puts the caret in a
    /// block of its document, and has none until the document is here.
    fn runnable_widget_commands(&self) -> usize {
        if !self.inputs.pending() {
            return self.widget_commands.len();
        }
        fn rich(node: &wire::Node, target: &str) -> bool {
            match node {
                wire::Node::Editor { key, options, .. } if key == target => options.rich.is_some(),
                node => node.children().iter().any(|child| rich(child, target)),
            }
        }
        self.widget_commands
            .iter()
            .take_while(|(_, command)| {
                matches!(command, wire::WidgetCommand::Focus { target }
                    if !self.frame.root.as_ref().is_some_and(|root| rich(root, target)))
            })
            .count()
    }

    /// Called on this guest's mounted tree with the commands that can run
    /// now. A command whose target left the tree in the meantime is answered
    /// with that, never performed.
    fn execute_widget_commands(
        &mut self,
        mut execute: impl FnMut(wire::WidgetCommand) -> Result<Vec<u8>, String>,
    ) {
        let runnable = self.runnable_widget_commands();
        let commands: Vec<_> = self.widget_commands.drain(..runnable).collect();
        for (id, command) in commands {
            let result = match self.target_is_mounted(&command) {
                true => execute(command)
                    .map_err(|error| wire::Refusal::new("widget_command_failed", error)),
                false => Err(wire::Refusal::new(
                    "widget_unmounted",
                    "widget target left the tree",
                )),
            };
            self.reply(id, result);
        }
    }

    fn reply(&mut self, id: u64, result: kernel::Answer) {
        self.pending.push(wire::Event::Response {
            id,
            result,
            done: true,
        });
    }

    fn report_display_truncation(&mut self) {
        let Some(generation) = self.installed_generation else {
            return;
        };
        for origin in self
            .display_diagnostics
            .observe(self.frame_reports)
            .into_iter()
            .flatten()
        {
            tracing::warn!(target: "ducktape::app", module = self.module, generation,
                reason = "display_text_truncated", origin, "module view display text truncated");
        }
    }

    /// One call into the module with the pending events, inside the budget.
    /// A trap ends the view; the widget shows the message in its place.
    fn tick(&mut self) {
        let events = std::mem::take(&mut self.pending);
        let bytes = wire::encode(&events);
        arm(&mut self.store);
        let outcome = self
            .tick
            .call(&mut self.store, (bytes,))
            .map(|(frame,)| frame)
            .map_err(|error| first_line(&error))
            .and_then(|frame| shape(&frame));
        match outcome {
            Ok((mut frame, mut reports)) => {
                let inherits = frame.root.is_none();
                let mut previous = self.frame.root.take();
                let mut accepted = true;
                let merged = merge(&mut previous, &mut frame)
                    .map_err(str::to_owned)
                    .and_then(|changed| {
                        if changed.0
                            && let Some(root) = &frame.root
                        {
                            self.inputs.validate(root)?;
                        }
                        Ok(changed)
                    });
                match merged {
                    Ok((false, _)) => {}
                    Ok((true, report)) => {
                        reports.local.merge(report);
                        self.frame_rev += 1;
                        if let Some(root) = &mut frame.root {
                            if let Err(error) = self.inputs.replace(root) {
                                self.fault = Some(error);
                            }
                            self.pictures.adopt(root);
                            // The guest remembers its tree without the
                            // picture bytes; the tree its patches build on
                            // has to be that one.
                            root.for_each_mut(&mut |node| match node {
                                wire::Node::Svg { bytes, .. } => *bytes = None,
                                wire::Node::Image { data, .. }
                                | wire::Node::ImageViewer { data, .. } => *data = None,
                                _ => {}
                            });
                        }
                    }
                    // Preserve accepted document state while requesting a full tree.
                    Err(refused) => {
                        accepted = false;
                        frame.root = previous;
                        tracing::warn!(
                            target: "ducktape::app",
                            module = self.module,
                            reason = "module_view_patch_refused",
                            error = refused,
                            "module view patch refused"
                        );
                        self.frame_rev += 1;
                        self.pending.push(wire::Event::Resync);
                    }
                }
                if accepted {
                    if inherits {
                        reports.inherit(self.frame_reports);
                    }
                    self.frame_reports = reports;
                    self.report_display_truncation();
                    if let Err(error) = self.inputs.frame(&frame) {
                        self.fault = Some(error);
                    }
                    self.pending.extend(self.inputs.drain());
                }
                self.frame = frame;
            }
            Err(trap) => {
                let reason = panic_message(&mut self.store).unwrap_or(trap);
                tracing::warn!(
                    target: "ducktape::app",
                    module = self.module,
                    reason = "module_view_trapped",
                    error = %reason,
                    "module view ended"
                );
                self.fault = Some(reason);
                self.frame_rev += 1;
            }
        }
    }
}

/// Brings the tree the host holds into `frame`: an `unchanged` frame takes
/// it as is, a frame without a tree patches it, a frame with one replaces
/// it. `Ok(true)` is a tree the widget has to rebuild for.
fn merge(
    held: &mut Option<wire::Node>,
    frame: &mut wire::Frame,
) -> Result<(bool, wire::SanitizeReport), &'static str> {
    if frame.unchanged {
        frame.root = held.take();
        return Ok((false, Default::default()));
    }
    if frame.root.is_some() {
        return Ok((true, Default::default()));
    }
    let patches = std::mem::take(&mut frame.patches);
    let mut root = held.as_ref().ok_or("no tree to patch")?.clone();
    let report = wire::apply(&mut root, patches)?;
    frame.root = Some(root);
    Ok((true, report))
}

/// What the host is willing to take from one tick's bytes: nothing in here
/// is trusted — the length, the counts, the tree.
fn shape(bytes: &[u8]) -> Result<(wire::Frame, display_diagnostics::FrameReports), String> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("frame too large".to_string());
    }
    let mut frame: wire::Frame = wire::decode(bytes)?;
    let requests_exceed_budget = frame.requests.len() > MAX_REQUESTS_PER_TICK;
    let cancels_exceed_budget = frame.cancels.len() > 2 * MAX_REQUESTS_PER_TICK;
    if requests_exceed_budget || cancels_exceed_budget {
        return Err("frame request or cancellation budget exceeded".into());
    }
    if frame.unchanged {
        frame.root = None;
    }
    if frame.unchanged || frame.root.is_some() {
        frame.patches = Vec::new();
    }
    let upstream = frame.upstream_sanitization;
    let local = wire::sanitize(&mut frame).map_err(str::to_owned)?;
    frame.upstream_sanitization = upstream;
    Ok((frame, display_diagnostics::FrameReports { local, upstream }))
}

fn panic_message(store: &mut Store<HostState>) -> Option<String> {
    let text = store.data_mut().panic.take()?;
    (!text.is_empty()).then_some(text)
}

/// Why a call failed: the trap itself, not the wrapper and backtrace
/// wasmtime prints around it.
fn first_line(error: &wasmtime::Error) -> String {
    error
        .root_cause()
        .to_string()
        .lines()
        .next()
        .unwrap_or("trap")
        .to_string()
}

/// The manifest epoch a view must speak, or the plain refusal sentence for
/// every unsupported epoch.
fn wire_epoch(epoch: u32) -> Result<(), String> {
    (epoch == wire::WIRE_EPOCH).then_some(()).ok_or_else(|| {
        format!(
            "this view speaks wire epoch {epoch}; this app speaks {}",
            wire::WIRE_EPOCH
        )
    })
}

#[path = "module_view/display_diagnostics.rs"]
mod display_diagnostics;

#[path = "module_view/input.rs"]
pub(crate) mod input;
#[path = "module_view/pictures.rs"]
mod pictures;

/// The name a component's manifest gives it; "" for one whose manifest
/// cannot be read (`compile` refuses those before a seat).
fn manifest_name(bytes: &[u8]) -> String {
    view_wire::manifest::read_manifest(bytes)
        .map(|manifest| manifest.name)
        .unwrap_or_default()
}

// ---------- the widget ----------

/// The native window retains this entity while a tab is open. A deployment
/// replacement gets a fresh native tree, so no focus or event route survives
/// across guest instances; ordinary guest frames retain keyed control state.
pub(crate) struct NativeModuleView {
    module: &'static str,
    content: Option<gpui_kit::Entity<crate::view_tree::ViewTree>>,
    subscription: Option<gpui_kit::Subscription>,
    generation: u64,
    revision: u64,
    alive: Option<Arc<()>>,
    replies_changed: Option<gpui_kit::Task<()>>,
    deadline: Option<(Instant, gpui_kit::Task<()>)>,
    observers: Vec<gpui_kit::Subscription>,
    hovered_files: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    pointer_inside: std::rc::Rc<std::cell::Cell<bool>>,
    /// Drawn since the props were last set: a layer mounted this frame.
    drawn: bool,
}

impl gpui_kit::EventEmitter<ModuleViewEvent> for NativeModuleView {}

/// The guest's root, sized to the seat: a wire root that names no size
/// would otherwise take its content's, and a Fill-sized pane inside it
/// (a room, a document) collapses to zero height.
fn native_root(root: wire::Node) -> wire::Node {
    wire::Node::Container {
        shadow: Default::default(),
        max_width: None,
        max_height: None,
        clip: false,
        key: "NativeModuleView/root".into(),
        width: Some(wire::Length::Fill),
        height: Some(wire::Length::Fill),
        padding: None,
        align_x: None,
        align_y: None,
        background: None,
        border: None,
        snap: None,
        content: Box::new(root),
    }
}

/// What a tab draws where its view is not: the load's stage over a skeleton
/// of a view, or why there is none — named, with Retry where a retry helps.
struct Standin {
    title: Option<&'static str>,
    words: String,
    loading: bool,
    retry: bool,
}

impl From<String> for Standin {
    fn from(words: String) -> Self {
        Standin {
            title: None,
            words,
            loading: false,
            retry: false,
        }
    }
}

impl From<&Failure> for Standin {
    fn from(failure: &Failure) -> Self {
        Standin {
            title: Some(failure.title()),
            words: failure.to_string(),
            loading: false,
            retry: true,
        }
    }
}

/// The words a loading tab says for its stage.
fn stage_words(slot: &Slot) -> String {
    let size = |bytes: u64| match bytes < 1_000_000 {
        true => format!("{} KB", bytes.div_ceil(1000)),
        false => format!("{:.1} MB", bytes as f64 / 1e6),
    };
    match slot {
        Slot::Fetching {
            received,
            total: Some(total),
        } => format!(
            "Fetching the view — {} of {}",
            size(*received),
            size(*total)
        ),
        Slot::Fetching { received, .. } => format!("Fetching the view — {}", size(*received)),
        Slot::Compiling => "Compiling the view…".into(),
        _ => "Loading the view…".into(),
    }
}

impl NativeModuleView {
    /// The id around the view's tree: the test door reads the module off it.
    fn ax_mark(&self) -> gpui_kit::ElementId {
        gpui_kit::ElementId::Name(format!("{}{}", crate::ax_door::VIEW_MARK, self.module).into())
    }

    pub(crate) fn new(module: &'static str) -> Self {
        Self {
            module,
            content: None,
            subscription: None,
            generation: 0,
            revision: 0,
            alive: None,
            replies_changed: None,
            deadline: None,
            observers: Vec::new(),
            hovered_files: Default::default(),
            pointer_inside: Default::default(),
            drawn: false,
        }
    }

    pub(crate) fn set_props(&mut self, props: Vec<u8>, cx: &mut gpui_kit::Context<Self>) {
        let seat = mounted(self.module);
        let mut seat = seat.lock().expect("module view lock");
        let changed = seat.props.as_ref() != Some(&props);
        if changed {
            seat.props = Some(props);
            cx.notify();
        }
        // a seat no layer draws this frame is still turned once, after it:
        // an overlay mounts only once it draws, and it draws only once it
        // has run (#110)
        self.drawn = false;
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = view.update(cx, |view, cx| {
                if !view.drawn {
                    view.turn(cx);
                }
            });
        });
    }

    /// One turn of a seated guest no layer draws: the props, replies and
    /// presses it is owed go in, and the requests its tick makes are
    /// answered — a chord claim among them.
    fn turn(&mut self, cx: &mut gpui_kit::Context<Self>) {
        let seat = mounted(self.module);
        let mut locked = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *locked;
        let Slot::Ready(guest) = slot else {
            return;
        };
        let again = guest.redraw(props);
        filesystem::mount(guest, cx);
        let intents = std::mem::take(&mut guest.intents);
        drop(locked);
        for intent in intents {
            cx.emit(intent);
        }
        if again {
            cx.notify();
        }
    }

    /// A chord was pressed: if this seat's module claimed it, its guest is
    /// told and redrawn, and the press is spent. Says whether it landed, so
    /// the shell stops at the seat that took it.
    pub(crate) fn chord(&mut self, chord: &str, cx: &mut gpui_kit::Context<Self>) -> bool {
        if chord_holder(chord) != Some(self.module) {
            return false;
        }
        let seat = mounted(self.module);
        let mut mounted = seat.lock().expect("module view lock");
        let Slot::Ready(guest) = &mut mounted.slot else {
            return false;
        };
        let taken = guest.chord_pressed(chord);
        drop(mounted);
        if taken && !self.drawn {
            self.turn(cx);
        }
        if taken {
            cx.notify();
        }
        taken
    }

    /// A hidden tab gets one bounded update before its native presenter leaves.
    pub(crate) fn hide(&mut self) -> Vec<ModuleViewEvent> {
        let Some(alive) = &self.alive else {
            return Vec::new();
        };
        let seat = mounted(self.module);
        let mut mounted = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *mounted;
        let Slot::Ready(guest) = slot else {
            return Vec::new();
        };
        let owns_instance =
            guest.seated_generation() == self.generation && Arc::ptr_eq(alive, &guest.alive);
        if !owns_instance || !guest.visible {
            return Vec::new();
        }
        guest.set_visible(false);
        guest.redraw(props);
        std::mem::take(&mut guest.intents)
    }

    /// Closing has no next paint. Deliver the final semantic observation through
    /// one bounded guest redraw and return its intents to the surviving shell.
    pub(crate) fn observe_final_window_event(
        &mut self,
        event: wire::events::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Vec<ModuleViewEvent> {
        let Some(alive) = &self.alive else {
            return Vec::new();
        };
        let seat = mounted(self.module);
        let mut mounted = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *mounted;
        let Slot::Ready(guest) = slot else {
            return Vec::new();
        };
        if guest.seated_generation() != self.generation || !Arc::ptr_eq(alive, &guest.alive) {
            return Vec::new();
        }
        let accepted = input::deliver(
            guest,
            wire::Event::Observation {
                event: wire::events::Event::Window(event),
                captured: false,
            },
        );
        if !accepted {
            return Vec::new();
        }
        guest.redraw(props);
        cx.notify();
        std::mem::take(&mut guest.intents)
    }

    fn frame(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Result<(), Standin> {
        let mounted = mounted(self.module);
        let mut locked = mounted.lock().expect("module view lock");
        locked.shown = Some(Instant::now());
        let Mounted { slot, props, .. } = &mut *locked;
        let guest = match slot {
            Slot::Ready(guest) => guest,
            Slot::Empty => {
                return Err(format!(
                    "This network has no {} view. An admin can activate a deployment that ships one.",
                    self.module
                )
                .into());
            }
            Slot::Failed(failure) => return Err((&*failure).into()),
            loading => {
                window.request_animation_frame();
                return Err(Standin {
                    title: None,
                    words: stage_words(loading),
                    loading: true,
                    retry: false,
                });
            }
        };
        let generation = guest.seated_generation();
        let ticks = guest.ticks;
        guest.set_visible(true);
        let again = guest.redraw(props);
        filesystem::mount(guest, cx);
        if again {
            window.request_animation_frame();
        }
        let next = kernel::next_tick(&guest.clocks);
        let deadline_changed = self.deadline.as_ref().map(|(due, _)| *due) != next;
        if deadline_changed {
            self.deadline = next.map(|due| {
                let timer = cx
                    .background_executor()
                    .timer(due.saturating_duration_since(Instant::now()));
                let task = cx.spawn(async move |view, cx| {
                    timer.await;
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
                (due, task)
            });
        }
        if let Some(fault) = &guest.fault {
            return Err((&Failure::Trapped(fault.clone())).into());
        }
        let same_instance = self.generation == generation
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &guest.alive));
        let changed = !same_instance || self.revision != guest.frame_rev;
        if changed {
            let mut root = guest.frame.root.clone().unwrap_or_else(wire::Node::empty);
            guest.pictures.hydrate(&mut root);
            let root = native_root(root);
            self.revision = guest.frame_rev;
            match (&self.content, same_instance) {
                (Some(content), true) => content.update(cx, |tree, cx| tree.replace(root, cx)),
                _ => {
                    self.generation = generation;
                    self.alive = Some(guest.alive.clone());
                    let mut changes = guest.replies.changes();
                    self.replies_changed = Some(cx.spawn(async move |view, cx| {
                        while changes.changed().await.is_ok() {
                            if view.update(cx, |_, cx| cx.notify()).is_err() {
                                break;
                            }
                        }
                    }));
                    // An answer that landed before subscription still needs
                    // delivery; later answers wake the entity directly.
                    if guest.replies.answer_owed() {
                        window.request_animation_frame();
                    }
                    let presentation = self
                        .content
                        .as_ref()
                        .map(|content| content.read(cx).presentation(window, cx))
                        .unwrap_or_default();
                    let content = cx.new(|_| {
                        crate::view_tree::ViewTree::new(root).with_presentation(presentation)
                    });
                    content.update(cx, |tree, cx| {
                        tree.set_editor_store(guest.inputs.clone(), cx)
                    });
                    let seat = mounted.clone();
                    let alive = guest.alive.clone();
                    self.subscription =
                        Some(cx.subscribe(&content, move |this, source, event, cx| {
                            let activation = source.read(cx).take_user_activation(event);
                            let mut locked = seat.lock().expect("module view lock");
                            let Slot::Ready(guest) = &mut locked.slot else {
                                return;
                            };
                            let current_instance = guest.seated_generation() == generation
                                && Arc::ptr_eq(&alive, &guest.alive);
                            if !current_instance || guest.frame_rev != this.revision {
                                cx.notify();
                                return;
                            }
                            guest.user_activation = activation;
                            input::deliver(guest, event.clone());
                            cx.notify();
                        }));
                    self.content = Some(content);
                }
            }
        }
        if let Some(content) = &self.content {
            if ticks != guest.ticks {
                content.update(cx, |_, cx| cx.notify());
            }
            let commands_ready = guest.runnable_widget_commands() > 0;
            if commands_ready {
                let view = cx.entity().downgrade();
                let seat = mounted.clone();
                let alive = guest.alive.clone();
                // The child tree mounts during this frame. A newly opened
                // menu or input cannot receive focus before that render.
                window.defer(cx, move |window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        let mut locked = seat.lock().expect("module view lock");
                        let Slot::Ready(guest) = &mut locked.slot else {
                            return;
                        };
                        // The GUEST has to be the one that asked — a
                        // replacement or a reseat takes its queue with it.
                        // The FRAME does not: a live view replaces its frame
                        // between the press and this deferred run, and each
                        // command is judged against the tree standing now.
                        let same_guest = guest.seated_generation() == generation
                            && Arc::ptr_eq(&alive, &guest.alive);
                        if !same_guest {
                            return;
                        }
                        if guest.runnable_widget_commands() == 0 {
                            cx.notify();
                            return;
                        }
                        let Some(content) = &this.content else {
                            return;
                        };
                        guest.execute_widget_commands(|command| {
                            content.update(cx, |tree, cx| {
                                tree.execute_widget_command(command, window, cx)
                            })
                        });
                        cx.notify();
                    });
                });
            }
        }
        for intent in std::mem::take(&mut guest.intents) {
            cx.emit(intent);
        }
        Ok(())
    }
}

impl NativeModuleView {
    /// THE ONE STAND-IN every tab draws where its view is not, native, so it
    /// draws before any wasm exists: a loading view says its stage over a
    /// skeleton laid out as a view lays itself out — a heading, then rows,
    /// from the top of the full pane the view will take, so nothing moves
    /// when it seats — and a failed one says what failed, why, and offers
    /// Retry.
    fn standin(&self, standin: Standin, cx: &mut gpui_kit::Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::Button;
        use gpui_kit::{
            FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
            div, px, relative,
        };
        let Standin {
            title,
            words,
            loading,
            retry: offers_retry,
        } = standin;
        let module = self.module;
        // a stable id per kind, so the tree tells a load on its way from a
        // view that is not there
        let (id, words_id) = match loading {
            true => ("view-loading", "view-loading-stage"),
            false => ("view-unavailable", "view-unavailable-reason"),
        };
        // the whole sentence, wrapped inside the pane: one line wider than
        // the pane is centred off both edges, and a reader loses its start
        // and its end — what failed, and what to do about it
        let reason = div()
            .id(id)
            .max_w_full()
            .flex()
            .flex_col()
            .gap_2()
            .children(title.map(|title| {
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(gpui_kit::Text::new(
                        "view-unavailable-title".into(),
                        title.into(),
                    ))
            }))
            .child(gpui_kit::Text::new(words_id.into(), words.into()))
            .children(offers_retry.then(|| {
                Button::new("view-retry")
                    .label("Retry")
                    .outline()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        drop(retry(module));
                        cx.notify();
                    }))
            }));
        #[cfg(test)]
        let reason = {
            use gpui_kit::test::TestSupportExt as _;
            reason.test_support()
        };
        let pane = div().id(self.ax_mark()).size_full().flex().flex_col().p_4();
        if !loading {
            return pane
                .items_center()
                .justify_center()
                .child(reason)
                .into_any_element();
        }
        let faint = gpui_kit::hsla(0., 0., 0.5, 0.14);
        let bar = |width: f32, height: f32| {
            div()
                .w(relative(width))
                .h(px(height))
                .rounded(px(4.))
                .bg(faint)
        };
        pane.gap_3()
            .child(bar(0.3, 22.))
            .child(div().text_size(px(12.)).opacity(0.7).child(reason))
            .children([0.9, 0.7, 0.8, 0.55, 0.85, 0.65].map(|width| bar(width, 14.)))
            .into_any_element()
    }
}

impl gpui_kit::Render for NativeModuleView {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{
            InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
        };
        self.bind_observers(window, cx);
        self.drawn = true;
        match self.frame(window, cx) {
            Ok(()) => match &self.content {
                Some(content) => {
                    let content = content.clone();
                    // GPUI rebuilds the accessibility tree from prepaint on
                    // every frame and a cached view replays paint without
                    // it, so a cached guest tree has no nodes and no
                    // focus for a screen reader. While one is listening,
                    // render the tree in full each frame as before.
                    let guest = if window.is_a11y_active() {
                        content.clone().into_any_element()
                    } else {
                        content
                            .clone()
                            .cached(gpui_kit::StyleRefinement::default().size_full())
                            .into_any_element()
                    };
                    let mut context = gpui_kit::KeyContext::default();
                    context.set(
                        "ducktape_guest",
                        format!("view{}", cx.entity().entity_id().as_u64()),
                    );
                    // A view owns its own inset: a split pane runs to the edges.
                    gpui_kit::div()
                        .id(self.ax_mark())
                        .key_context(context)
                        .size_full()
                        .child(guest)
                        .child(input::Observe::new(
                            gpui_kit::div().absolute().inset_0().into_any_element(),
                            self,
                            cx,
                        ))
                        .into_any_element()
                }
                None => gpui_kit::div().size_full().into_any_element(),
            },
            Err(standin) => self.standin(standin, cx),
        }
    }
}
