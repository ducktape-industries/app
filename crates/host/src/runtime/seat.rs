use super::*;

/// One seat: the load state, props and retry hold-off of one view instance
/// of one program (instance 0 is the preloaded seat no tab has claimed yet:
/// compiled, never started).
/// Shared by the tab's widget on the window thread and the loader thread,
/// which swaps a finished load in place; the tab polls it while loading.
pub(super) struct Mounted {
    /// The widget instance that claimed this seat; 0 while preloaded. The
    /// loader reads it for the perf key a load's stages land under.
    pub(super) instance: u64,
    /// The connection this seat was last asked of.
    pub(super) rev: u64,
    pub(super) slot: Slot,
    pub(super) props: Option<Vec<u8>>,
    pub(super) generation: u64,
    /// The code a load last failed on. Cleared by any load that comes back
    /// and by the person's Retry.
    pub(super) retry: Option<Retry>,
    /// The seat's load wake: signalled by an install, a stage shown and a
    /// retry, so the `Seat` turns once for each instead of asking for a
    /// frame per frame while a load is on its way.
    pub(super) wake: tokio::sync::watch::Sender<()>,
    /// A replacement for the drawn view, its code prepared on a loader
    /// thread, waiting for the seat's thread to run it (`swap_in`).
    pub(super) swap: Option<Box<Parked>>,
    /// A view to start where none is drawn, its code prepared on a loader
    /// thread (or compiled before, for a pane claiming the seat), waiting
    /// for the seat's thread to seat and init it (`start_in`): no instance
    /// is seated and no call begun anywhere else.
    pub(super) to_start: Option<Box<Parked>>,
    /// A sequence of calls the seat's thread runs owns the drawn view (a
    /// swap, `swap_in`; a kept snapshot, `keep_in`): the seat does not turn
    /// it until the sequence is done, which wakes the seat. One owner at a
    /// time: a sequence starts only on a view nothing owns and no tick is in
    /// flight on (one call in flight per instance, `ViewInstance`), sets
    /// this under the same lock, and is the only one to clear it.
    pub(super) owned: bool,
    /// New code the roster named over what the seat holds already (a view
    /// drawn or compiled, none, a failure), which stays up until the load
    /// asked for it (its generation) lands: how far that load has come, as
    /// the rail counts it ([`Self::prep`]).
    pub(super) replacing: Option<(u64, Prep)>,
}

/// Code a load prepared and parked on the seat for the seat's thread to
/// run (a replacement, `Mounted::swap`; a start, `Mounted::to_start`): its
/// code, the name it is shown by, the load that prepared it (installed on
/// the same conditions as any load, `install`) and the load's timing so far.
pub(super) struct Parked {
    pub(super) code: Box<ViewCode>,
    pub(super) shown: String,
    pub(super) generation: u64,
    pub(super) asked_of: Connection,
    pub(super) timing: LoadTiming,
}

/// A failed load's hold-off: the code (the roster's blob id, as
/// `code_digest` spells it) the load was asked for when it failed. That
/// code is not asked for again until the roster names another or the
/// person presses Retry; `None` is asked for again at the next roster read.
pub(super) struct Retry {
    pub(super) code: Option<[u8; 32]>,
}

impl Mounted {
    /// A seat with nothing asked for yet: `Loading` until its source is
    /// asked, under generation 0, which no load answers for.
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
        }))
    }

    /// Load `generation`'s stage, shown only while the seat shows a load and
    /// still waits for this one: a view drawn, the network's "none" and a
    /// failure stay up until the load lands, and the rail hears only how far
    /// their replacement has come. A stage never steps back: the loader
    /// reading again the bytes the roster's read ahead read
    /// ([`read_ahead`]) leaves them read. Either way the rail is told.
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

    /// How far the seat's code has come on this device ([`Prep`]), settled
    /// once it is drawn, compiled, none or failed, unless new code the roster
    /// named is on its way over what it holds.
    pub(super) fn prep(&self) -> Prep {
        match self.replacing {
            _ if self.slot.loading() => Prep::of(&self.slot),
            Some((generation, prep)) if generation == self.generation => prep,
            _ => Prep::Settled,
        }
    }

    /// Whether a roster read at `now` starts a load for this seat, given
    /// that the program's `code` is the `same_code` the last roster listed
    /// and the seat was `asked_of_this_node` already:
    ///
    /// | hold-off from a failed load | load when                              |
    /// |-----------------------------|----------------------------------------|
    /// | none                        | the code moved, or this node not asked |
    /// | for some code               | it is not for this very `code`         |
    ///
    /// The hold-off and `code` both name the blob the roster lists
    /// (`Retry.code`, [`code_digest`]), so a failed view is left alone until
    /// the roster names other code or the person presses Retry.
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

    /// A load for `asked_for` came back with no view: later blocks leave
    /// that code alone. A failure before any candidate (a status or
    /// transport error, `hash` none) holds nothing off, and any other code
    /// is unaffected. A view that is there stays: the failure is the
    /// replacement's, not its own.
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

    /// Opens the next load generation, so an older load still on its way
    /// lands nowhere, and names it. A load already on its way starts over
    /// at `Loading`: none of its stages is this one's.
    pub(super) fn start(&mut self) -> u64 {
        if self.slot.loading() {
            self.slot = Slot::Loading;
        }
        self.generation += 1;
        self.generation
    }

    /// The pane that claimed the seat let it go: a drawn view steps back to
    /// the code it was seated from (its instance and memory go, the code
    /// stays), a compiled one stays as it is, and a load on its way for the
    /// pane lands nowhere. False when there is nothing compiled to keep, or
    /// a sequence owns the drawn view: the seat goes.
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

    /// The program left the roster: the seat is emptied, and the `Seat`
    /// woken to draw that, since nothing else about it moves.
    pub(super) fn retire(&mut self) {
        self.generation += 1;
        self.slot = Slot::Empty;
        self.wake.send_replace(());
    }
}

/// What a seat holds, and so what its tab draws: the view on its way —
/// asked, its bytes coming in, verified, compiled — the view itself, the
/// network's word that there is none, or why there is none, named.
pub(super) enum Slot {
    /// Asked for; nothing back yet.
    Loading,
    /// The artifact's bytes coming in: how many, of how many when the node
    /// said.
    Fetching {
        received: u64,
        total: Option<u64>,
    },
    /// The bytes here and their manifest read, no code compiled: the rail
    /// and Home list the program by it (SPEC §4.7).
    Listed(Box<Listing>),
    /// Verified; its code compiles. The listing stays what the rail reads.
    Compiling(Box<Listing>),
    /// Compiled, and not started: a seat no pane has claimed keeps its
    /// view's code and what its manifest says, which the rail and the
    /// layout read. The pane that claims it starts that code.
    Compiled(Box<ViewCode>),
    Ready(Box<View>),
    /// The active deployment verified, and it ships no view.
    Empty,
    Failed(Failure),
}

/// A view compiled and not started: its code, prepared on `runner` under
/// the hash of its bytes and kept prepared by `_hold`, out of the `blob` the
/// roster lists (none for the developer's override, which a pane reads
/// again), and what its manifest says: the name
/// the rail lists it by, the icon its tab draws folded (empty: it declares
/// none), the tint of its tile on a phone's Home, the narrowest it is laid out, its capabilities and its targets.
/// All a pane needs to start it, with nothing fetched or compiled again.
#[derive(Clone)]
pub(super) struct ViewCode {
    pub(super) runner: Arc<dyn ViewRunner>,
    pub(super) blob: Option<abi::BlobId>,
    pub(super) hash: [u8; 32],
    /// Held only to keep the code prepared (`contract::Prepared`).
    pub(super) _hold: Arc<dyn std::any::Any + Send + Sync>,
    pub(super) name: String,
    pub(super) icon: String,
    pub(super) tint: Tint,
    pub(super) description: String,
    pub(super) min_width: u32,
    pub(super) capabilities: Vec<Capability>,
    pub(super) targets: Vec<String>,
    pub(super) wire: String,
}

#[cfg(any(test, feature = "test-support"))]
impl ViewCode {
    /// An empty program no blob lists, its manifest saying `name`, `icon`
    /// and `min_width`: a seat's compiled view no pane starts as it is.
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
            capabilities: Vec::new(),
            targets: Vec::new(),
            wire: view_wire::WIRE_ID.into(),
        })
    }
}

impl Slot {
    /// Compiled out of the blob `code` names: a pane starts it as it is.
    pub(super) fn startable(&self, code: Option<(abi::BlobId, bool)>) -> bool {
        matches!(self, Slot::Compiled(view) if view.blob.is_some() && view.blob == code.map(|(blob, _)| blob))
    }

    /// On its way: one of the stages a load shows before it lands.
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
    /// The view ran out of fuel or trapped.
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
    static MOUNTED: OnceLock<Registry> = OnceLock::new();
    MOUNTED.get_or_init(Mutex::default)
}

/// The pane `instance` let go of `program`'s seat: it is put back as the
/// program's preloaded seat, `(program, 0)`, which the next pane's
/// [`mounted`] claims, stepped back to its compiled code
/// (`Mounted::step_back`), so neither the next open nor the roster's next
/// read loads it again and the rail keeps listing it. A seat with nothing
/// compiled to keep, or a program that has a preloaded seat already, goes.
pub(super) fn let_go(program: &'static str, instance: u64) {
    let mut registry = lock(registry());
    let Some(seat) = registry.remove(&(program, instance)) else {
        return;
    };
    if !registry.contains_key(&(program, 0)) && lock(&seat).step_back() {
        registry.insert((program, 0), seat);
    }
}

/// Claim a preloaded seat or create a distinct view for this view instance.
pub(super) fn mounted(program: &'static str, instance: u64) -> Arc<Mutex<Mounted>> {
    let code = roster().code(program);
    let mut registry = lock(registry());
    let seat = registry.remove(&(program, 0)).unwrap_or_else(Mounted::seat);
    registry.insert((program, instance), seat.clone());
    let snapshot = connection().lock().expect("views rpc").clone();
    let mut locked = lock(&seat);
    locked.instance = instance;
    if let Slot::Ready(view) = &mut locked.slot {
        view.instance = instance;
    }
    let first = locked.generation == 0;
    let asked = snapshot.client.is_some() || view_override(program).is_some();
    // a seat preloaded only as far as compiled: this pane's load starts
    // it, right here when it holds the code the roster lists: an instance
    // and its `init`, on the one tick's fuel every tick here runs on
    let compiled = matches!(locked.slot, Slot::Compiled(_));
    let here = locked.slot.startable(code);
    // its preload still reading its bytes, in a roster read's read ahead
    // that waits for every program's (`read_ahead`): this pane's own load
    // reads them now, ahead of every other
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
    // the program's row may now be read off this seat
    rail_moved();
    seat
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
    // a failure's row says Loading again
    rail_moved();
    queue(Load {
        program,
        seat,
        generation,
        asked_of: snapshot,
        code,
    });
}

/// One load a seat waits for: `program`'s view out of `code` (the roster's
/// blob id when the load was asked for, and whether that blob is the view
/// itself), asked of the node in `asked_of`, under the seat's `generation`.
pub(super) struct Load {
    pub(super) program: &'static str,
    pub(super) seat: Arc<Mutex<Mounted>>,
    pub(super) generation: u64,
    pub(super) asked_of: Connection,
    pub(super) code: Option<(abi::BlobId, bool)>,
}

/// The loads waiting for a loader, and how many loaders run.
struct Loaders {
    waiting: VecDeque<Load>,
    running: usize,
}

static LOADING: Mutex<Loaders> = Mutex::new(Loaders {
    waiting: VecDeque::new(),
    running: 0,
});

/// Hands `load` to a loader thread — a cold cranelift compile is a second
/// or more; the window thread shows "Loading" instead of freezing for it.
/// A loader starts while fewer than [`loaders`] run, and each takes the next
/// waiting load until none is left, so a long roster queues rather than
/// forking a thread per program. A load a pane waits for goes ahead of the
/// preloads. With no thread to be had and no loader left to take it, what
/// waits fails as any load does, with a Retry. Never called holding the
/// registry or a seat's lock: that failure installs under the seat's.
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

/// The next waiting load, or none, and the loader asking stops. A seat a
/// pane claimed while its preload waited (a tap on a tile still waiting)
/// goes first: the pane waits for it.
fn next_load() -> Option<Load> {
    let mut loaders = lock(&LOADING);
    let next = next_of(&loaders.waiting);
    let load = loaders.waiting.remove(next);
    if load.is_none() {
        loaders.running -= 1;
    }
    load
}

/// Where in `waiting` the next load is: the first a pane claimed, else the
/// first.
fn next_of(waiting: &VecDeque<Load>) -> usize {
    (waiting.iter())
        .position(|load| lock(&load.seat).instance != 0)
        .unwrap_or(0)
}

/// How many preloads' bytes a read ahead reads at once ([`read_ahead`]):
/// each is its program's blob in memory (a few MB, framed and not) until its
/// manifest is read and the bytes let go.
const READ_AHEAD: usize = 4;

/// What a read ahead learnt of one preload's code ([`read_ahead`]).
enum Read {
    /// Its view's manifest: a loader prepares it, its bytes read again off
    /// the disk.
    View(Box<Listing>),
    /// It ships no view.
    None,
    /// Its bytes did not come.
    Failed(Failure),
    /// Not the network's bytes to read (the developer's file, no node, not
    /// listed): its loader asks, as any load does.
    Loader,
}

/// The first half of a roster read's preloads, every one's before any is
/// prepared: its bytes read, a few at a time (the cache's copy, else the
/// node's, which the cache keeps from then on), whether it ships a view and
/// its view's manifest kept and the bytes let go. Once every one is read,
/// all are shown at once, under the registry's lock the rail reads under:
/// each listed by its manifest, none, or failed, so a failure holds none of
/// the others. Then the loaders prepare the ones with a view. On a task of
/// its own: the roster read that asked is over.
// ponytail: a blob the node never answers (no request timeout) holds the
// second half of every preload, as it held the phone's one loader before;
// a timeout on the read when it does
pub(super) fn read_ahead(loads: Vec<Load>) {
    if loads.is_empty() {
        return;
    }
    handle().spawn(async move {
        use futures::StreamExt as _;
        let began = Instant::now();
        let read: Vec<(Load, Read, LoadTiming)> = futures::stream::iter(loads)
            .map(read_one)
            .buffer_unordered(READ_AHEAD)
            .collect()
            .await;
        let (programs, from_node) = (
            read.len(),
            (read.iter())
                .filter(|(_, _, timing)| timing.blob == "node")
                .count(),
        );
        let mut prepare = Vec::new();
        {
            let _rail = lock(registry());
            for (load, read, timing) in read {
                // a pane claimed the seat meanwhile and loads it itself
                // (`mounted`), or a newer roster read asked again
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

/// One preload's bytes read ahead ([`read_ahead`]), shown read once they
/// are (`Fetching` all of them: the rail counts it read, still on its way).
/// A panic in the read is its failure, as a loader's is (`Load::land`): the
/// read ahead goes on, and every other program with it.
async fn read_one(load: Load) -> (Load, Read, LoadTiming) {
    use futures::FutureExt as _;
    let mut timing = LoadTiming::default();
    let read = std::panic::AssertUnwindSafe(read_bytes(&load, &mut timing))
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| {
            let failure = panicked("reading", &*panic);
            let reason = failure.to_string();
            log_source(load.program, None, "Failed", load.generation, &reason);
            timing.outcome = "Failed";
            Read::Failed(failure)
        });
    (load, read, timing)
}

/// A program a seat test's read panics on ([`read_bytes`]).
#[cfg(test)]
const PANICS_READING: &str = "seat-tests-panics-reading";

/// [`read_one`]'s read itself.
async fn read_bytes(load: &Load, timing: &mut LoadTiming) -> Read {
    #[cfg(test)]
    assert_ne!(load.program, PANICS_READING, "the read panicked");
    let (Some((blob, bare)), Some(client), None) = (
        load.code,
        load.asked_of.client.clone(),
        view_override(load.program),
    ) else {
        return Read::Loader;
    };
    let show = |stage: Slot| lock(&load.seat).show(load.generation, stage);
    timing.started = Some(Instant::now());
    timing.path = "read";
    let bytes = match crate::contract::os().bundled_view(&blob) {
        Some(view) => Ok(Some((view, "bundle"))),
        None => {
            show(Slot::Fetching {
                received: 0,
                total: None,
            });
            crate::backend::views::view_of(&client, &blob, bare).await
        }
    };
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
            let hash = code_digest(&blob);
            log_source(load.program, Some(&hash), "Missing", load.generation, "");
            (timing.hash, timing.outcome) = (Some(hash), "Missing");
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
    /// The load itself, on a loader: [`View::load`], landed.
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

    /// Installs what `loading` comes back with, only if the seat still
    /// waits for this very load AND, for a network's view, the app is still
    /// on the node it was asked of. A view the developer's override
    /// supplies is the same on every node: its load lands wherever the app
    /// has moved to meanwhile. A panic in `loading` or in the install is the
    /// load's failure, held off and offered Retry like any other, and the
    /// loader goes on to the next load.
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
        // the blob this load is asked for, as the roster reads compare it:
        // a failure is held off under this, not under the view section's
        // own hash, which never equals a blob id
        let asked_for = self.code.map(|(code, _)| code_digest(&code));
        let mut timing = LoadTiming::default();
        let loaded = catch_unwind(AssertUnwindSafe(|| loading(&self, &mut timing))).unwrap_or_else(
            |panic| {
                Err(Unloaded {
                    hash: asked_for,
                    failure: panicked("loading", &*panic),
                })
            },
        );
        // the window thread holds this lock while it ticks the seat
        let waited = Instant::now();
        let mut locked = lock(&self.seat);
        timing.lock_wait = waited.elapsed();
        let key = crate::perf::Key::View {
            program,
            instance: locked.instance,
        };
        let installed = Instant::now();
        // the guard outlives a panic in here, so the seat is not poisoned
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
            let failure = panicked("installing", &**panic);
            locked.load_failed(
                program,
                asked_for,
                Unloaded {
                    hash: asked_for,
                    failure,
                },
            );
        }
        // claimed while this very load preloaded it: the pane's own load
        // starts it. Only this load's own install decides: a stale or a
        // left-node load lands nothing, and hands off nothing
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

/// A loader's panic as the failure its load came back with: what it was
/// doing, in the panic's own words.
fn panicked(doing: &str, panic: &(dyn std::any::Any + Send)) -> Failure {
    let words = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message");
    Failure::Refused(format!("the loader stopped {doing} the view: {words}"))
}

/// Puts what a load came back with into the seat, if the seat still waits
/// for this very load and, for a network's view, the app is still on the
/// node it was asked of. A swap also drops the old `View` here, its store
/// and memory with it. `true` when what it put there is `Compiled`: a seat
/// no pane held when the load began, which a pane may hold by now.
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
    // the connection lock is a leaf: read under the seat's lock, never
    // across it. A connect bumps the revision before it reaches this
    // seat, so the revision read here is the one the seat installs
    // against — and `connected` reconnects a seat it passed already
    let current = connection().lock().expect("views rpc").clone();
    let from_the_node = view_override(program).is_none();
    let node_since_left = current.rev != asked_of.rev;
    if from_the_node && node_since_left {
        return false;
    }
    // a load that came back at all clears the hold-off; only a failure
    // puts one back. What it replaces is replaced, or stays for good
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
        // the seat's thread seats and inits it (`start_in`), which `land`
        // asks for once the lock is let go; the rail reads the manifest
        // meanwhile, and a view drawn (the developer's file read again)
        // stays up until the new one is
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
        // the same tab, the same surface handle, the same host-side
        // input text and pictures: only the instance behind them moves, on
        // the seat's own thread (`swap_in`), which this wakes
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

/// A replacement the loader parked (`Mounted::swap`), run on the seat's
/// thread (SPEC §4.6): the drawn view asked for its snapshot, a fresh
/// instance seated, restored from it and its first frame taken, each
/// answer awaited as it lands (at once on the wasmtime runner, so the
/// whole sequence runs in one poll there), then installed on the
/// conditions a load installs on. The seat does not turn the drawn view
/// meanwhile (`Mounted::owned`); the seat is woken when it is done. A
/// swap refused or failed keeps the drawn view and is asked again at the
/// next roster read.
pub(super) async fn swap_in(mounted: Arc<Mutex<Mounted>>, program: &'static str) {
    let started = {
        let mut locked = lock(&mounted);
        let Some(pending) = locked.swap.take() else {
            return;
        };
        // a later load moved the seat on: this swap would install nowhere
        if pending.generation != locked.generation {
            return;
        }
        let Slot::Ready(old) = &locked.slot else {
            return;
        };
        // a tick in flight, or a kept snapshot (`keep_in`), has the
        // instance: the swap waits for it, and the turn after asks again
        // (`Seat::after_turn`); so does a death the turn has to take up
        // first (`take_up`), which wakes the seat when done
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
    let current = connection().lock().expect("views rpc").clone();
    let stale = locked.generation != generation
        || (view_override(program).is_none() && current.rev != asked_of.rev);
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

/// A view a load parked to start where none is drawn (`Mounted::to_start`),
/// run on the seat's thread (SPEC §4.4: `instantiate` and every `begin_*`
/// are that thread's): an instance seated, then its `init`, each answer
/// awaited as it lands (at once on the wasmtime runner, so the whole
/// sequence runs in one poll there, later on JSC), then installed on the
/// conditions a load installs on (`install`). It logs the `view_source`
/// line and the `view_load` line the load left to it.
pub(super) async fn start_in(mounted: Arc<Mutex<Mounted>>, program: &'static str) {
    let (pending, instance) = {
        let mut locked = lock(&mounted);
        let Some(pending) = locked.to_start.take() else {
            return;
        };
        // a later load moved the seat on: this start would install nowhere
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

/// The drawn view's kept replay started again from a new snapshot (SPEC
/// §6.5), on the seat's thread, the view owned meanwhile so no tick
/// crosses it; the seat is woken when it lands. A page that died under the
/// snapshot is parked on the view (`View::parked`) for the turn that wake
/// runs to take up ([`take_up`]): an idle view begins no tick that would
/// find it, and in the background no call is begun.
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

/// A death parked on the drawn view outside a tick (`View::parked`: under a
/// kept snapshot or a swap's handover), taken up on the seat's thread in
/// front (the turn spawns this only there): the view revived with nothing
/// in flight, or stopped on it (`View::died`), owned meanwhile so no tick
/// or swap crosses it; the seat is woken when it is done.
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

/// `fresh`, prepared as the replacement of the drawn view `alive` names,
/// made ready to take its seat: the view's editor projections and pictures
/// carried over. Why there is no replacement, if there is none.
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

/// What a load came back with.
pub(super) enum Loaded {
    /// A view where there was none.
    Fresh(Box<View>),
    /// The view already drawn is the active deployment's.
    Unchanged,
    /// Code prepared to replace the view drawn, `shown` naming it: the
    /// seat's thread runs the swap (`swap_in`).
    Swap { code: Box<ViewCode>, shown: String },
    /// The deployment ships no view.
    Empty,
    /// The view compiles, and was not started: no pane holds the seat.
    Compiled(Box<ViewCode>),
    /// Code prepared to start where no view is drawn, `shown` naming it:
    /// the seat's thread seats and inits it (`start_in`).
    Seat { code: Box<ViewCode>, shown: String },
}

/// A load that came back with no view, and the view bytes it failed on —
/// none when it never got as far as any, which is what tells the block
/// check whether there is a code to hold off at all.
pub(super) struct Unloaded {
    pub(super) hash: Option<[u8; 32]>,
    pub(super) failure: Failure,
}

/// One `view_source` line per outcome, with the same fields every time:
/// stable keys for anyone reading loads out of app.log.
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

/// How long each stage of one program-owned load took, and where its bytes
/// and code came from: `fetch` is the node's answer (`blob`: `disk` or
/// `node`), `compile` is cranelift (`code`: `memory`, `disk` or `cold`),
/// both `seat` for a claimed seat started from the code it compiled,
/// then `instantiate`, a swap's `snapshot` of the old view and `restore`
/// into the new, `init` for a fresh start, `first_frame` the tree a
/// replacement proves (a fresh view draws its first on the window thread),
/// and `install` under the seat's lock after `lock_wait` for it. `path` is
/// `first` for a load over an empty slot, `swap` over a view drawn.
/// `started` is set once the load got past the roster: only those log.
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
    /// One `view_load` line per load, every field every time, once the
    /// load is installed, on the perf target.
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

/// A VIEW DEVELOPER'S OVERRIDE, and nothing else: the directory `main`
/// reads out of `DUCKTAPE_VIEWS_DIR`. While it is set, `<id>_view.wasm` in
/// it is loaded for any view the app seats, in place of the network's
/// bytes — unverified, and said so in app.log each time it is loaded.
/// The file the developer's override directory (`Dirs::views_override`)
/// supplies for `program`, if any.
pub(super) fn view_override(program: &str) -> Option<PathBuf> {
    let dir = crate::backend::dirs().views_override.as_ref()?;
    let path = dir.join(format!("{program}_view.wasm"));
    path.is_file().then_some(path)
}

// the seat's gpui entity and the stand-in it draws while its view comes
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
