use super::*;

pub(super) struct Mounted {
    pub(super) instance: u64,
    pub(super) rev: u64,
    pub(super) slot: Slot,
    pub(super) props: Option<Vec<u8>>,
    pub(super) generation: u64,
    pub(super) retry: Option<Retry>,
    pub(super) wake: tokio::sync::watch::Sender<()>,
    pub(super) swap: Option<Box<Parked>>,
    pub(super) to_start: Option<Box<Parked>>,
    pub(super) owned: bool,
    pub(super) replacing: Option<(u64, Prep)>,
    pub(super) rekeyed_at: Option<u64>,
}

pub(super) struct Parked {
    pub(super) code: Box<ViewCode>,
    pub(super) shown: String,
    pub(super) generation: u64,
    pub(super) asked_of: Connection,
    pub(super) timing: LoadTiming,
}

pub(super) struct Retry {
    pub(super) code: Option<[u8; 32]>,
}

impl Mounted {
    pub(super) fn seat() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            instance: 0,
            rev: 0,
            slot: Slot::Loading,
            props: None,
            generation: 0,
            retry: None,
            wake: tokio::sync::watch::Sender::new(()),
            swap: None,
            to_start: None,
            owned: false,
            replacing: None,
            rekeyed_at: None,
        }))
    }

    pub(super) fn show(&mut self, generation: u64, stage: Slot) {
        if self.generation != generation {
            return;
        }
        let far = Prep::of(&stage);
        if self.slot.loading() {
            if far < Prep::of(&self.slot) {
                return;
            }
            self.slot = stage;
            self.wake.send_replace(());
        } else if let Some((replacing, prep)) = &mut self.replacing
            && *replacing == generation
            && far > *prep
        {
            *prep = far;
        } else {
            return;
        }
        rail_moved();
    }

    pub(super) fn prep(&self) -> Prep {
        match self.replacing {
            _ if self.slot.loading() => Prep::of(&self.slot),
            Some((generation, prep)) if generation == self.generation => prep,
            _ => Prep::Settled,
        }
    }

    pub(super) fn reload_due(
        &self,
        same_code: bool,
        asked_of_this_node: bool,
        code: [u8; 32],
    ) -> bool {
        match &self.retry {
            None => !(same_code && asked_of_this_node),
            Some(held_off) => held_off.code != Some(code),
        }
    }

    pub(super) fn load_failed(
        &mut self,
        program: &str,
        asked_for: Option<[u8; 32]>,
        Unloaded {
            hash: failed_on,
            failure,
        }: Unloaded,
    ) {
        tracing::warn!(
            target: "ducktape::app",
            program,
            reason = "program_view_unloadable",
            error = %failure,
            "program view not loaded"
        );
        self.retry = Some(Retry {
            code: failed_on.and(asked_for),
        });
        if !matches!(self.slot, Slot::Ready(_)) {
            self.slot = Slot::Failed(failure);
        }
    }

    pub(super) fn start(&mut self) -> u64 {
        if self.slot.loading() {
            self.slot = Slot::Loading;
        }
        self.generation += 1;
        self.generation
    }

    fn step_back(&mut self) -> bool {
        if self.owned {
            return false;
        }
        match &mut self.slot {
            Slot::Ready(view) => match view.code.take() {
                Some(code) => self.slot = Slot::Compiled(code),
                None => return false,
            },
            Slot::Compiled(_) => {}
            _ => return false,
        }
        self.instance = 0;
        self.generation += 1;
        self.swap = None;
        self.to_start = None;
        self.wake.send_replace(());
        true
    }

    pub(super) fn retire(&mut self) {
        self.generation += 1;
        self.slot = Slot::Empty;
        self.wake.send_replace(());
    }

    pub(super) fn rekey(&mut self, props: &[u8]) -> bool {
        if !rekeyed(self.props.as_deref(), Some(props)) {
            return false;
        }
        self.props = Some(props.to_vec());
        let Slot::Ready(view) = &mut self.slot else {
            return false;
        };
        let running = view.connection.clone();
        let restart = match self.swap.take() {
            Some(pending) => Some((pending.code, pending.shown, pending.asked_of)),
            None => view
                .restart_code()
                .map(|(code, shown)| (code, shown, running)),
        };
        self.generation += 1;
        self.rekeyed_at = Some(self.generation);
        self.to_start = None;
        self.wake.send_replace(());
        let Some((code, shown, asked_of)) = restart else {
            self.slot = Slot::Failed(Failure::Refused(
                "the seated key changed, and this view has no code to start again".into(),
            ));
            return true;
        };
        self.slot = Slot::Compiled(code.clone());
        self.to_start = Some(Box::new(Parked {
            code,
            shown,
            generation: self.generation,
            asked_of,
            timing: LoadTiming::default(),
        }));
        true
    }
}

fn signer(props: Option<&[u8]>) -> Option<String> {
    props
        .and_then(|props| wire::methods::decode::<wire::methods::Session>(props).ok())
        .map(|session| session.signer)
        .filter(|signer| !signer.is_empty())
}

pub(super) fn rekeyed(was: Option<&[u8]>, now: Option<&[u8]>) -> bool {
    signer(was).is_some_and(|was| signer(now).as_deref() != Some(was.as_str()))
}

#[cfg(test)]
mod rekey_tests {
    use super::*;

    fn session(signer: &str) -> Vec<u8> {
        wire::methods::encode(&wire::methods::Session {
            connected: true,
            chain_id: "dev".into(),
            signer: signer.into(),
            account: None,
            endpoint: String::new(),
        })
    }

    #[test]
    fn only_a_move_off_a_seated_key_restarts() {
        let (a, b, none) = (session("aa"), session("bb"), session(""));
        assert!(!rekeyed(None, Some(&a)));
        assert!(!rekeyed(Some(&none), Some(&a)));
        assert!(!rekeyed(Some(&a), Some(&a)));
        assert!(rekeyed(Some(&a), Some(&b)));
        assert!(rekeyed(Some(&a), Some(&none)));
        assert!(rekeyed(Some(&a), None));
    }

    #[test]
    fn a_key_change_restarts_from_the_update_waiting_to_swap_in() {
        let mut view = crate::runtime::kernel::tests::view();
        view.code = Some(ViewCode::for_test("Telegram", "", 480));
        let asked_of = view.connection.clone();
        let mut update = ViewCode::for_test("Telegram v2", "", 480);
        update.hash = [2; 32];
        let seat = Mounted::seat();
        let mut locked = lock(&seat);
        locked.props = Some(session("aa"));
        locked.slot = Slot::Ready(Box::new(view));
        locked.swap = Some(Box::new(Parked {
            code: update,
            shown: "telegram view @ 020202020202".into(),
            generation: locked.generation,
            asked_of,
            timing: LoadTiming::default(),
        }));
        assert!(locked.rekey(&session("bb")));
        assert!(locked.swap.is_none());
        assert!(matches!(&locked.slot, Slot::Compiled(code) if code.hash == [2; 32]));
        let parked = locked.to_start.as_ref().expect("its start is parked");
        assert_eq!(parked.code.hash, [2; 32], "the old code started again");
        assert_eq!(parked.generation, locked.generation);
    }
}

pub(super) enum Slot {
    Loading,
    Fetching { received: u64, total: Option<u64> },
    Listed(Box<Listing>),
    Compiling(Box<Listing>),
    Compiled(Box<ViewCode>),
    Ready(Box<View>),
    Empty,
    Failed(Failure),
}

#[derive(Clone)]
pub(super) struct ViewCode {
    pub(super) runner: Arc<dyn ViewRunner>,
    pub(super) blob: Option<abi::BlobId>,
    pub(super) hash: [u8; 32],
    pub(super) _hold: Arc<dyn std::any::Any + Send + Sync>,
    pub(super) name: String,
    pub(super) icon: String,
    pub(super) tint: Tint,
    pub(super) description: String,
    pub(super) min_width: u32,
    pub(super) width: Option<u32>,
    pub(super) capabilities: Vec<Capability>,
    pub(super) hosts: Vec<String>,
    pub(super) wire: String,
}

#[cfg(any(test, feature = "test-support"))]
impl ViewCode {
    pub(super) fn for_test(name: &str, icon: &str, min_width: u32) -> Box<Self> {
        Box::new(ViewCode {
            runner: crate::contract::runner().clone(),
            blob: None,
            hash: [0; 32],
            _hold: Arc::new(()),
            name: name.into(),
            icon: icon.into(),
            tint: Tint::None,
            description: String::new(),
            min_width,
            width: None,
            capabilities: Vec::new(),
            hosts: Vec::new(),
            wire: view_wire::WIRE_LEVEL.to_string(),
        })
    }
}

impl Slot {
    pub(super) fn startable(&self, code: Option<(abi::BlobId, bool)>) -> bool {
        matches!(self, Slot::Compiled(view) if view.blob.is_some() && view.blob == code.map(|(blob, _)| blob))
    }

    pub(super) fn loading(&self) -> bool {
        matches!(
            self,
            Slot::Loading | Slot::Fetching { .. } | Slot::Listed(_) | Slot::Compiling(_)
        )
    }
}

/// Why a seat has no view, by name — the tab says it above the reason and
/// offers Retry. The reason is the load's own sentence.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The node could not be asked, or does not hold the bytes (yet).
    Unreachable(String),
    /// The bytes do not hash to the code the programs role names.
    HashMismatch(String),
    /// This network's programs role does not list the view.
    NotListed(String),
    /// The view trapped.
    Trapped(String),
    /// Bytes this build does not run as a view, a load overtaken, or a
    /// loader that stopped.
    Refused(String),
    /// A view built against another wire than this app's stopped: the
    /// reason says what failed, where, and both wires.
    Wire(String),
}

/// What a view and the screen hear when the node did not answer — a load that
/// could not ask, or a request that ran out of retries. The transport's own
/// text stays in the log.
pub const NODE_UNREACHABLE: &str = "The node could not be reached";

impl Failure {
    pub(super) fn title(&self) -> &'static str {
        match self {
            Failure::Unreachable(_) => NODE_UNREACHABLE,
            Failure::HashMismatch(_) => "The bytes do not match the network's code hash",
            Failure::NotListed(_) => "This network does not list this view",
            Failure::Trapped(_) => "This view stopped",
            Failure::Refused(_) => "This view could not be loaded",
            Failure::Wire(_) => "This view, built against another wire, stopped",
        }
    }
}

impl Failure {
    pub(super) fn of(error: crate::backend::views::Fetch) -> Failure {
        use crate::backend::views::Fetch;
        let reason = error.to_string();
        match error {
            Fetch::Unreachable(_) | Fetch::NotHeld => Failure::Unreachable(reason),
            Fetch::Corrupt | Fetch::Collision(_) => Failure::HashMismatch(reason),
            Fetch::Refused(_) | Fetch::Unreadable(_) => Failure::Refused(reason),
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
        | Failure::Wire(reason)) = self;
        formatter.write_str(reason)
    }
}

pub(super) type Registry = Mutex<HashMap<(&'static str, u64), Arc<Mutex<Mounted>>>>;

pub(super) fn registry() -> &'static Registry {
    &in_hand().seats
}

pub(super) fn let_go(network: &Network, program: &'static str, instance: u64) {
    let mut registry = lock(&network.seats);
    let Some(seat) = registry.remove(&(program, instance)) else {
        return;
    };
    if !registry.contains_key(&(program, 0)) && lock(&seat).step_back() {
        registry.insert((program, 0), seat);
    }
}

pub(super) fn mounted(
    network: &Network,
    program: &'static str,
    instance: u64,
) -> Arc<Mutex<Mounted>> {
    let code = network.roster.code(program);
    let mut registry = lock(&network.seats);
    let seat = registry.remove(&(program, 0)).unwrap_or_else(Mounted::seat);
    registry.insert((program, instance), seat.clone());
    let snapshot = lock(&network.connection).clone();
    let mut locked = lock(&seat);
    locked.instance = instance;
    if let Slot::Ready(view) = &mut locked.slot {
        view.instance = instance;
    }
    let first = locked.generation == 0;
    let listed = !network.roster.lock().is_empty();
    let asked = asks_now(snapshot.client.is_some(), code.is_some(), listed)
        || view_override(program).is_some();
    let compiled = matches!(locked.slot, Slot::Compiled(_));
    let here = locked.slot.startable(code);
    let reading = !first && matches!(locked.slot, Slot::Loading | Slot::Fetching { .. });
    let load = (first && asked || compiled || reading).then(|| {
        locked.rev = snapshot.rev;
        Load {
            program,
            seat: seat.clone(),
            generation: locked.start(),
            asked_of: snapshot,
            code,
        }
    });
    drop(locked);
    drop(registry);
    match load {
        Some(load) if here => {
            load.land(|load, timing| View::start(load.program, load.code, &load.seat, timing))
        }
        Some(load) => queue(load),
        None => {}
    }
    rail_moved();
    seat
}

fn asks_now(connected: bool, listed_here: bool, roster_read: bool) -> bool {
    connected && (listed_here || roster_read)
}

/// Retry, as a failed or stopped tab offers it: the seat's view is asked for
/// again now, under a new generation, past any hold-off a block's retries
/// left. A failure goes back to loading, and a stopped view gives up its
/// seat, so what lands is a fresh instance rather than a swap against it.
pub fn retry(program: &'static str, instance: u64) {
    let code = roster().code(program);
    let registry = lock(registry());
    let Some(seat) = registry.get(&(program, instance)).cloned() else {
        return;
    };
    let snapshot = connection().lock().expect("views rpc").clone();
    let mut locked = lock(&seat);
    let stopped = matches!(&locked.slot, Slot::Ready(view) if view.fault.is_some());
    if stopped || matches!(locked.slot, Slot::Failed(_)) {
        locked.slot = Slot::Loading;
    }
    locked.retry = None;
    let generation = locked.start();
    locked.wake.send_replace(());
    drop(locked);
    drop(registry);
    rail_moved();
    queue(Load {
        program,
        seat,
        generation,
        asked_of: snapshot,
        code,
    });
}

pub(super) struct Load {
    pub(super) program: &'static str,
    pub(super) seat: Arc<Mutex<Mounted>>,
    pub(super) generation: u64,
    pub(super) asked_of: Connection,
    pub(super) code: Option<(abi::BlobId, bool)>,
}

struct Loaders {
    waiting: VecDeque<Load>,
    running: usize,
}

static LOADING: Mutex<Loaders> = Mutex::new(Loaders {
    waiting: VecDeque::new(),
    running: 0,
});

pub(super) fn queue(load: Load) {
    let claimed = lock(&load.seat).instance != 0;
    let mut loaders = lock(&LOADING);
    match claimed {
        true => loaders.waiting.push_front(load),
        false => loaders.waiting.push_back(load),
    }
    if loaders.running >= super::loaders() {
        return;
    }
    loaders.running += 1;
    drop(loaders);
    let spawned = std::thread::Builder::new()
        .name("view-loader".into())
        .spawn(|| {
            while let Some(load) = next_load() {
                load.run();
            }
        });
    let Err(error) = spawned else {
        return;
    };
    let mut loaders = lock(&LOADING);
    loaders.running -= 1;
    let stranded = match loaders.running {
        0 => std::mem::take(&mut loaders.waiting),
        _ => VecDeque::new(),
    };
    drop(loaders);
    tracing::warn!(
        target: "ducktape::app",
        reason = "loader_unavailable",
        error = %error,
        "no thread to load a view on"
    );
    for load in stranded {
        let failure = Failure::Refused(format!("no thread to load the view on: {error}"));
        load.land(|_, _| {
            Err(Unloaded {
                hash: None,
                failure,
            })
        });
    }
}

fn next_load() -> Option<Load> {
    let mut loaders = lock(&LOADING);
    let next = first_claimed_or_first_waiting_load(&loaders.waiting);
    let load = loaders.waiting.remove(next);
    if load.is_none() {
        loaders.running -= 1;
    }
    load
}

fn first_claimed_or_first_waiting_load(waiting: &VecDeque<Load>) -> usize {
    (waiting.iter())
        .position(|load| lock(&load.seat).instance != 0)
        .unwrap_or(0)
}

enum Read {
    View(Box<Listing>),
    None,
    Failed(Failure),
    Loader,
}

pub(super) fn read_ahead(loads: Vec<Load>) {
    if loads.is_empty() {
        return;
    }
    let network = loads[0].asked_of.on.get();
    handle().spawn(async move {
        let began = Instant::now();
        let read = read_every(loads).await;
        let (programs, from_node) = (
            read.len(),
            (read.iter())
                .filter(|(_, _, timing)| timing.blob == "node")
                .count(),
        );
        let mut prepare = Vec::new();
        {
            let _rail = network.as_ref().map(|network| lock(&network.seats));
            for (load, read, timing) in read {
                if lock(&load.seat).generation != load.generation {
                    continue;
                }
                match read {
                    Read::View(listing) => {
                        lock(&load.seat).show(load.generation, Slot::Listed(listing));
                        prepare.push(load);
                    }
                    Read::Loader => prepare.push(load),
                    Read::None => load.land(move |_, landed| {
                        *landed = timing;
                        Ok(Loaded::Empty)
                    }),
                    Read::Failed(failure) => load.land(move |_, landed| {
                        *landed = timing;
                        Err(Unloaded {
                            hash: None,
                            failure,
                        })
                    }),
                }
            }
        }
        tracing::info!(
            target: "ducktape::perf",
            programs,
            from_node,
            views = prepare.len(),
            read_ms = began.elapsed().as_millis(),
            "view_read"
        );
        for load in prepare {
            queue(load);
        }
    });
}

enum Source {
    Read(Read),
    Node {
        code: abi::BlobId,
        bare: bool,
        client: crate::backend::RpcClient,
    },
}

type ViewBytes =
    Result<Option<(crate::backend::views::Blob, &'static str)>, crate::backend::views::Fetch>;

async fn read_every(loads: Vec<Load>) -> Vec<(Load, Read, LoadTiming)> {
    use crate::backend::views::{programs_bytes, view_in, view_of};
    let mut read = Vec::with_capacity(loads.len());
    let mut asked = Vec::new();
    for load in loads {
        let mut timing = LoadTiming::default();
        match caught(&load, &mut timing, read_source) {
            Ok(Source::Read(done)) => read.push((load, done, timing)),
            Ok(Source::Node { code, bare, client }) => {
                asked.push((load, timing, code, bare, client))
            }
            Err(failure) => read.push((load, Read::Failed(failure), timing)),
        }
    }
    let Some(client) = asked.first().map(|asked| asked.4.clone()) else {
        return read;
    };
    let mut codes = Vec::new();
    for asked in &asked {
        if !codes.contains(&asked.2) {
            codes.push(asked.2);
        }
    }
    let mut again = Vec::new();
    programs_bytes(&client, &codes, |code, bytes| {
        let mut bytes = Some(bytes);
        let theirs: Vec<_> = asked.extract_if(.., |asked| asked.2 == code).collect();
        for (load, mut timing, _, bare, _) in theirs {
            let Some(bytes) = bytes.take() else {
                again.push((load, timing, code, bare));
                continue;
            };
            let view = bytes.map(|(blob, from)| view_in(blob, bare).map(|view| (view, from)));
            let done = caught(&load, &mut timing, |load, timing| {
                read_listing(load, timing, view)
            });
            read.push((load, done.unwrap_or_else(Read::Failed), timing));
        }
    })
    .await;
    for (load, mut timing, code, bare) in again {
        let view = view_of(&client, &code, bare).await;
        let done = caught(&load, &mut timing, |load, timing| {
            read_listing(load, timing, view)
        });
        read.push((load, done.unwrap_or_else(Read::Failed), timing));
    }
    for (load, timing, ..) in asked {
        let failure = Failure::Refused("the node's answer left this program's code out".into());
        read.push((load, Read::Failed(failure), timing));
    }
    read
}

fn caught<T>(
    load: &Load,
    timing: &mut LoadTiming,
    reading: impl FnOnce(&Load, &mut LoadTiming) -> T,
) -> Result<T, Failure> {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    catch_unwind(AssertUnwindSafe(|| reading(load, timing))).map_err(|panic| {
        let failure = loader_panic_with_stage("reading", &*panic);
        let reason = failure.to_string();
        log_source(load.program, None, "Failed", load.generation, &reason);
        timing.outcome = "Failed";
        failure
    })
}

#[cfg(test)]
const PANICS_READING: &str = "seat-tests-panics-reading";

fn read_source(load: &Load, timing: &mut LoadTiming) -> Source {
    #[cfg(test)]
    assert_ne!(load.program, PANICS_READING, "the read panicked");
    let (Some((blob, bare)), Some(client), None) = (
        load.code,
        load.asked_of.client.clone(),
        view_override(load.program),
    ) else {
        return Source::Read(Read::Loader);
    };
    timing.started = Some(Instant::now());
    timing.path = "read";
    if let Some(view) = crate::contract::os().bundled_view(&blob) {
        return Source::Read(read_listing(
            load,
            timing,
            Ok(Some((view.into(), "bundle"))),
        ));
    }
    let fetching = Slot::Fetching {
        received: 0,
        total: None,
    };
    lock(&load.seat).show(load.generation, fetching);
    Source::Node {
        code: blob,
        bare,
        client,
    }
}

fn read_listing(load: &Load, timing: &mut LoadTiming, bytes: ViewBytes) -> Read {
    let show = |stage: Slot| lock(&load.seat).show(load.generation, stage);
    timing.fetch = timing
        .started
        .map_or_else(Duration::default, |started| started.elapsed());
    match bytes {
        Ok(Some((view, from))) => {
            timing.blob = from;
            let listing = manifest_of(&view);
            let size = view.len() as u64;
            drop(view);
            show(Slot::Fetching {
                received: size,
                total: Some(size),
            });
            Read::View(Box::new(listing))
        }
        Ok(None) => {
            let hash = load.code.map(|(code, _)| code_digest(&code));
            log_source(load.program, hash.as_ref(), "Missing", load.generation, "");
            (timing.hash, timing.outcome) = (hash, "Missing");
            show(Slot::Fetching {
                received: 0,
                total: Some(0),
            });
            Read::None
        }
        Err(error) => {
            let failure = Failure::of(error);
            let reason = failure.to_string();
            log_source(load.program, None, "Failed", load.generation, &reason);
            timing.outcome = "Failed";
            Read::Failed(failure)
        }
    }
}

impl Load {
    fn run(self) {
        self.land(|load, timing| {
            View::load(
                load.program,
                load.code,
                &load.asked_of,
                load.generation,
                &load.seat,
                timing,
            )
        });
    }

    pub(super) fn land(
        self,
        loading: impl FnOnce(&Load, &mut LoadTiming) -> Result<Loaded, Unloaded>,
    ) {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let Load {
            program,
            generation,
            ..
        } = self;
        let asked_for = self.code.map(|(code, _)| code_digest(&code));
        let mut timing = LoadTiming::default();
        let loaded = catch_unwind(AssertUnwindSafe(|| loading(&self, &mut timing))).unwrap_or_else(
            |panic| {
                Err(Unloaded {
                    hash: asked_for,
                    failure: loader_panic_with_stage("loading", &*panic),
                })
            },
        );
        let waited = Instant::now();
        let mut locked = lock(&self.seat);
        timing.lock_wait = waited.elapsed();
        let key = crate::perf::Key::View {
            program,
            instance: locked.instance,
        };
        let installed = Instant::now();
        let landed = catch_unwind(AssertUnwindSafe(|| {
            install(
                program,
                &mut locked,
                generation,
                &self.asked_of,
                asked_for,
                loaded,
                &mut timing,
            )
        }));
        if let Err(panic) = &landed
            && locked.generation == generation
        {
            let failure = loader_panic_with_stage("installing", &**panic);
            locked.load_failed(
                program,
                asked_for,
                Unloaded {
                    hash: asked_for,
                    failure,
                },
            );
        }
        let claimed = matches!(landed, Ok(true)) && locked.instance != 0;
        let start = claimed.then(|| locked.start());
        let parked =
            (locked.to_start.as_ref()).is_some_and(|parked| parked.generation == generation);
        rail_moved();
        locked.wake.send_replace(());
        timing.install = installed.elapsed();
        if timing.started.is_some() {
            crate::perf::record(
                key,
                "install.lock_wait",
                timing.lock_wait.as_micros() as u64,
            );
            crate::perf::record(key, "install", timing.install.as_micros() as u64);
            timing.log(program);
        }
        drop(locked);
        #[cfg(feature = "ui")]
        if parked {
            entity::begin_start(self.seat.clone(), program);
        }
        #[cfg(not(feature = "ui"))]
        let _ = (parked, &self.seat);
        if let Some(generation) = start {
            queue(Load { generation, ..self });
        }
    }
}

fn loader_panic_with_stage(doing: &str, panic: &(dyn std::any::Any + Send)) -> Failure {
    let words = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message");
    Failure::Refused(format!("the loader stopped {doing} the view: {words}"))
}

fn install(
    program: &'static str,
    locked: &mut Mounted,
    generation: u64,
    asked_of: &Connection,
    asked_for: Option<[u8; 32]>,
    loaded: Result<Loaded, Unloaded>,
    timing: &mut LoadTiming,
) -> bool {
    if locked.generation != generation {
        return false;
    }
    let current = asked_of.current();
    let from_the_node = view_override(program).is_none();
    let node_since_left = asked_of.stale();
    if from_the_node && node_since_left {
        return false;
    }
    locked.retry = None;
    locked.replacing = None;
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(unloaded) => {
            locked.load_failed(program, asked_for, unloaded);
            return false;
        }
    };
    let Mounted {
        slot,
        instance,
        swap,
        to_start,
        ..
    } = &mut *locked;
    match loaded {
        Loaded::Fresh(mut view) => {
            view.instance = *instance;
            view.installed_generation = Some(generation);
            view.report_display_truncation();
            *slot = Slot::Ready(view);
        }
        Loaded::Unchanged => {
            if let Slot::Ready(view) = slot {
                view.reconnect(&current);
            }
        }
        Loaded::Empty => {
            *slot = Slot::Empty;
        }
        Loaded::Compiled(view) => {
            *slot = Slot::Compiled(view);
            return true;
        }
        Loaded::Seat { code, shown } => {
            if !matches!(slot, Slot::Ready(_)) {
                *slot = Slot::Compiled(code.clone());
            }
            *to_start = Some(Box::new(Parked {
                code,
                shown,
                generation,
                asked_of: asked_of.clone(),
                timing: std::mem::take(timing),
            }));
        }
        Loaded::Swap { code, shown } => {
            *swap = Some(Box::new(Parked {
                code,
                shown,
                generation,
                asked_of: asked_of.clone(),
                timing: std::mem::take(timing),
            }));
        }
    }
    false
}

pub(super) async fn swap_in(mounted: Arc<Mutex<Mounted>>, program: &'static str) {
    let started = {
        let mut locked = lock(&mounted);
        let Some(pending) = locked.swap.take() else {
            return;
        };
        if pending.generation != locked.generation {
            return;
        }
        let Slot::Ready(old) = &locked.slot else {
            return;
        };
        if old.ticking() || locked.owned || old.parked.is_some() {
            locked.swap = Some(pending);
            return;
        }
        let alive = old.alive.clone();
        let instance = locked.instance;
        locked.owned = true;
        (pending, alive, instance)
    };
    let (pending, alive, instance) = started;
    let Parked {
        code,
        shown,
        generation,
        asked_of,
        mut timing,
    } = *pending;
    let prepared = View::replacement(
        program,
        instance,
        &code,
        &alive,
        &mounted,
        &shown,
        &mut timing,
    )
    .await;
    let waited = Instant::now();
    let mut locked = lock(&mounted);
    timing.lock_wait = waited.elapsed();
    let installed = Instant::now();
    locked.owned = false;
    let left = view_override(program).is_none() && asked_of.stale();
    let stale = locked.generation != generation || left;
    if stale && !left && locked.rekeyed_at == Some(locked.generation) {
        let current = locked.generation;
        let mounted: &mut Mounted = &mut locked;
        match mounted.to_start.as_mut() {
            Some(start) => {
                start.code = code.clone();
                start.shown = shown.clone();
                mounted.slot = Slot::Compiled(code.clone());
            }
            None => {
                mounted.swap = Some(Box::new(Parked {
                    code: code.clone(),
                    shown: shown.clone(),
                    generation: current,
                    asked_of: asked_of.clone(),
                    timing: LoadTiming::default(),
                }));
            }
        }
    }
    if !stale {
        let Mounted {
            slot,
            instance,
            retry,
            ..
        } = &mut *locked;
        match prepared
            .map_err(|failure| failure.to_string())
            .and_then(|fresh| swapped(slot, Box::new(fresh), &alive))
        {
            Ok(mut fresh) => {
                fresh.instance = *instance;
                fresh.installed_generation = Some(generation);
                fresh.report_display_truncation();
                log_source(program, fresh.hash.as_ref(), "Swapped", generation, "");
                *slot = Slot::Ready(fresh);
            }
            Err(reason) => {
                log_source(program, Some(&code.hash), "Failed", generation, &reason);
                *retry = Some(Retry { code: None });
            }
        }
    }
    rail_moved();
    locked.wake.send_replace(());
    timing.install = installed.elapsed();
    if timing.started.is_some() {
        let key = crate::perf::Key::View {
            program,
            instance: locked.instance,
        };
        crate::perf::record(
            key,
            "install.lock_wait",
            timing.lock_wait.as_micros() as u64,
        );
        crate::perf::record(key, "install", timing.install.as_micros() as u64);
        timing.log(program);
    }
}

pub(super) async fn start_in(mounted: Arc<Mutex<Mounted>>, program: &'static str) {
    let (pending, instance) = {
        let mut locked = lock(&mounted);
        let Some(pending) = locked.to_start.take() else {
            return;
        };
        if pending.generation != locked.generation {
            return;
        }
        (pending, locked.instance)
    };
    let Parked {
        code,
        shown,
        generation,
        asked_of,
        mut timing,
    } = *pending;
    let started = async {
        let mut fresh = View::seated_from(program, instance, &code, &shown, &mut timing).await?;
        fresh.timed_inited(&shown, &mut timing).await?;
        Ok::<View, Failure>(fresh)
    }
    .await;
    let (state, reason) = match &started {
        Ok(_) => ("Ready", String::new()),
        Err(failure) => ("Failed", failure.to_string()),
    };
    log_source(program, Some(&code.hash), state, generation, &reason);
    timing.outcome = state;
    let loaded = started
        .map(|fresh| Loaded::Fresh(Box::new(fresh)))
        .map_err(|failure| Unloaded {
            hash: Some(code.hash),
            failure,
        });
    let asked_for = code.blob.map(|blob| code_digest(&blob));
    let waited = Instant::now();
    let mut locked = lock(&mounted);
    timing.lock_wait = waited.elapsed();
    let installed = Instant::now();
    install(
        program,
        &mut locked,
        generation,
        &asked_of,
        asked_for,
        loaded,
        &mut timing,
    );
    rail_moved();
    locked.wake.send_replace(());
    timing.install = installed.elapsed();
    if timing.started.is_some() {
        let key = crate::perf::Key::View {
            program,
            instance: locked.instance,
        };
        crate::perf::record(
            key,
            "install.lock_wait",
            timing.lock_wait.as_micros() as u64,
        );
        crate::perf::record(key, "install", timing.install.as_micros() as u64);
        timing.log(program);
    }
}

pub(super) async fn keep_in(mounted: Arc<Mutex<Mounted>>) {
    let (landing, alive) = {
        let mut locked = lock(&mounted);
        if locked.owned {
            return;
        }
        let Slot::Ready(view) = &mut locked.slot else {
            return;
        };
        if !view.keep_due() {
            return;
        }
        let asked = (view.begin_keep(), view.alive.clone());
        locked.owned = true;
        asked
    };
    let landed = super::view::next(&landing).await;
    let mut locked = lock(&mounted);
    if let Slot::Ready(view) = &mut locked.slot
        && Arc::ptr_eq(&view.alive, &alive)
    {
        match landed {
            death @ Landed::Fault(Fault::Runner(_)) => view.parked = Some(death),
            landed => view.kept_landed(landed),
        }
    }
    locked.owned = false;
    locked.wake.send_replace(());
}

pub(super) async fn take_up(mounted: Arc<Mutex<Mounted>>) {
    let (alive, death) = {
        let mut locked = lock(&mounted);
        if locked.owned {
            return;
        }
        let Slot::Ready(view) = &mut locked.slot else {
            return;
        };
        if view.ticking() {
            return;
        }
        let Some(Landed::Fault(Fault::Runner(death))) = view.parked.take() else {
            return;
        };
        let alive = view.alive.clone();
        locked.owned = true;
        (alive, death)
    };
    View::died(&mounted, &alive, &death).await;
    let mut locked = lock(&mounted);
    locked.owned = false;
    locked.wake.send_replace(());
}

fn swapped(slot: &mut Slot, mut fresh: Box<View>, alive: &Arc<()>) -> Result<Box<View>, String> {
    let Slot::Ready(old) = slot else {
        return Err("the view left while its replacement was prepared".into());
    };
    if !Arc::ptr_eq(&old.alive, alive) {
        return Err("the view changed while its replacement was prepared".into());
    }
    if !old.settled() {
        return Err("the view took work while its replacement was prepared".into());
    }
    fresh.frame_rev = old.frame_rev + 1;
    let mut pictures = std::mem::take(&mut old.pictures);
    let own = std::mem::take(&mut fresh.pictures);
    let mut none = wire::Node::empty();
    let root = fresh.frame.root.as_mut().unwrap_or(&mut none);
    if pictures.carry(own, root, old.program) {
        fresh.pending.push(wire::Event::Resync);
    }
    fresh.pictures = pictures;
    Ok(fresh)
}

pub(super) enum Loaded {
    Fresh(Box<View>),
    Unchanged,
    Swap { code: Box<ViewCode>, shown: String },
    Empty,
    Compiled(Box<ViewCode>),
    Seat { code: Box<ViewCode>, shown: String },
}

pub(super) struct Unloaded {
    pub(super) hash: Option<[u8; 32]>,
    pub(super) failure: Failure,
}

pub(super) fn log_source(
    program: &str,
    hash: Option<&[u8; 32]>,
    state: &str,
    generation: u64,
    reason: &str,
) {
    tracing::info!(
        target: "ducktape::app",
        program,
        hash = %hash.map_or_else(|| "-".to_owned(), |hash| noded_client::hex::hex_encode(hash)),
        state,
        gen = generation,
        reason = %if reason.is_empty() { "-" } else { reason },
        "view_source"
    );
}

#[derive(Default)]
pub(super) struct LoadTiming {
    pub(super) path: &'static str,
    pub(super) started: Option<Instant>,
    pub(super) hash: Option<[u8; 32]>,
    pub(super) outcome: &'static str,
    pub(super) blob: &'static str,
    pub(super) code: &'static str,
    pub(super) fetch: Duration,
    pub(super) compile: Duration,
    pub(super) instantiate: Duration,
    pub(super) snapshot: Duration,
    pub(super) snapshot_bytes: usize,
    pub(super) restore: Duration,
    pub(super) init: Duration,
    pub(super) first_frame: Option<Duration>,
    pub(super) lock_wait: Duration,
    pub(super) install: Duration,
}

impl LoadTiming {
    pub(super) fn log(&self, program: &str) {
        let ms = |duration: Duration| duration.as_millis();
        let word = |word: &'static str| if word.is_empty() { "-" } else { word };
        let hash = self.hash.map_or_else(
            || "-".to_owned(),
            |hash| noded_client::hex::hex_encode(&hash),
        );
        let first_frame = self
            .first_frame
            .map_or_else(|| "-".to_owned(), |frame| ms(frame).to_string());
        let total = self.started.map_or(0, |started| ms(started.elapsed()));
        tracing::info!(
            target: "ducktape::perf",
            program,
            hash = %hash,
            path = self.path,
            outcome = self.outcome,
            blob = word(self.blob),
            code = word(self.code),
            fetch_ms = ms(self.fetch),
            compile_ms = ms(self.compile),
            instantiate_ms = ms(self.instantiate),
            snapshot_ms = ms(self.snapshot),
            snapshot_bytes = self.snapshot_bytes,
            restore_ms = ms(self.restore),
            init_ms = ms(self.init),
            first_frame_ms = %first_frame,
            lock_wait_ms = ms(self.lock_wait),
            install_ms = ms(self.install),
            total_ms = total,
            "view_load"
        );
    }
}

pub(super) fn view_override(program: &str) -> Option<PathBuf> {
    let dir = crate::backend::dirs().views_override.as_ref()?;
    let path = dir.join(format!("{program}_view.wasm"));
    path.is_file().then_some(path)
}

#[cfg(feature = "ui")]
mod entity;
#[cfg(feature = "ui")]
mod standin;

#[cfg(feature = "ui")]
pub use entity::{Backgrounded, Placed, Seat, serve_starts};

/// What a Back the shell took came to at the seat in front
/// ([`Seat::back`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Back {
    /// The view heard it: it goes back inside itself.
    Heard,
    /// The view has nowhere to go back to: the shell leaves the program.
    Leave,
}

#[cfg(test)]
mod entity_tests;
#[cfg(test)]
mod first_frame_tests;
#[cfg(test)]
pub(super) mod tests;
