use super::*;

/// The loads an event started; nobody waits on them, the seats swap in place.
pub struct Loads {
    pub(super) _threads: Vec<std::thread::JoinHandle<()>>,
}

/// One seat: the load state, props and retry hold-off of one view instance
/// of one module (instance 0 is the preloaded seat no tab has claimed yet).
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
    /// The code a load last failed on, and when the next block may try it
    /// again. Cleared by any load that comes back, so only a repeated
    /// failure on the same code widens the gap.
    pub(super) retry: Option<Retry>,
    /// When a tab last drew this seat: a load for a seat on screen does not
    /// queue for the link.
    pub(super) shown: Option<Instant>,
}

/// A failed load's hold-off: the code (the roster's blob id, as
/// `code_digest` spells it) the load was asked for when it failed, when the
/// next attempt at that same code is due, and the gap that produced it.
pub(super) struct Retry {
    pub(super) code: Option<[u8; 32]>,
    pub(super) next: Instant,
    pub(super) gap: Duration,
}

impl Retry {
    /// The hold-off after a load for `code` failed: the gap doubles up to
    /// `RETRY_MAX` while the same code keeps failing, and starts over at
    /// `RETRY_FIRST` for a different one.
    pub(super) fn after(previous: Option<&Retry>, code: Option<[u8; 32]>) -> Retry {
        let gap = match previous {
            Some(previous) if previous.code == code => (previous.gap * 2).min(RETRY_MAX),
            _ => RETRY_FIRST,
        };
        Retry {
            code,
            next: Instant::now() + gap,
            gap,
        }
    }
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
            shown: None,
        }))
    }

    /// Load `generation`'s stage, shown only while the seat shows a load and
    /// still waits for this one: a view drawn, the network's "none" and a
    /// failure stay up until the load lands.
    pub(super) fn show(&mut self, generation: u64, stage: Slot) {
        if self.generation == generation && self.slot.loading() {
            self.slot = stage;
        }
    }

    /// Whether a roster read at `now` starts a load for this seat, given
    /// that the program's `code` is the `same_code` the last roster listed
    /// and the seat was `asked_of_this_node` already:
    ///
    /// | hold-off from a failed load | load when                              |
    /// |-----------------------------|----------------------------------------|
    /// | none                        | the code moved, or this node not asked |
    /// | still running               | it is not for this very `code`         |
    /// | its gap is up               | always: the re-attempt it promised     |
    ///
    /// The hold-off and `code` both name the blob the roster lists
    /// (`Retry.code`, [`code_digest`]), so a failed view is left alone until
    /// its gap is up and then asked for again, RETRY_FIRST's way.
    pub(super) fn reload_due(
        &self,
        same_code: bool,
        asked_of_this_node: bool,
        code: [u8; 32],
        now: Instant,
    ) -> bool {
        match &self.retry {
            None => !(same_code && asked_of_this_node),
            Some(held_off) if now < held_off.next => held_off.code != Some(code),
            Some(_) => true,
        }
    }

    /// A load for `asked_for` came back with no view: the next block leaves
    /// that code alone until the gap is up, widened against `held_off` when
    /// it is the same code failing again. A failure before any candidate (a
    /// status or transport error, `hash` none) holds nothing off, and any
    /// other code is unaffected. A view that is there stays: the failure is
    /// the replacement's, not its own.
    pub(super) fn load_failed(
        &mut self,
        module: &str,
        held_off: Option<Retry>,
        asked_for: Option<[u8; 32]>,
        Unloaded {
            hash: failed_on,
            failure,
        }: Unloaded,
    ) {
        tracing::warn!(
            target: "ducktape::app",
            module,
            reason = "module_view_unloadable",
            error = %failure,
            "module view not loaded"
        );
        self.retry = Some(Retry::after(held_off.as_ref(), failed_on.and(asked_for)));
        if !matches!(self.slot, Slot::Ready(_)) {
            self.slot = Slot::Failed(failure);
        }
    }

    /// Opens the next load generation, so an older load still on its way
    /// lands nowhere, and names it.
    pub(super) fn start(&mut self) -> u64 {
        self.generation += 1;
        self.generation
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
    /// Verified; its code compiles.
    Compiling,
    Ready(Box<Guest>),
    /// The active deployment verified, and it ships no view.
    Empty,
    Failed(Failure),
}

impl Slot {
    /// On its way: one of the stages a load shows before it lands.
    pub(super) fn loading(&self) -> bool {
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
    /// The view was built against another wire than this app's.
    Wire(String),
}

/// What a view and the screen hear when the node did not answer — a load that
/// could not ask, or a request that ran out of retries. The transport's own
/// text stays in the log.
pub(crate) const NODE_UNREACHABLE: &str = "The node could not be reached";

impl Failure {
    pub(super) fn title(&self) -> &'static str {
        match self {
            Failure::Unreachable(_) => NODE_UNREACHABLE,
            Failure::HashMismatch(_) => "The bytes do not match the network's code hash",
            Failure::NotListed(_) => "This network does not list this view",
            Failure::Trapped(_) => "This view stopped",
            Failure::Refused(_) => "This view could not be loaded",
            Failure::Wire(_) => "This view speaks a wire this app does not",
        }
    }
}

impl Failure {
    pub(super) fn of(error: crate::backend::views::Fetch) -> Failure {
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
        | Failure::Wire(reason)) = self;
        formatter.write_str(reason)
    }
}

pub(super) type Registry = Mutex<HashMap<(&'static str, u64), Arc<Mutex<Mounted>>>>;

pub(super) fn registry() -> &'static Registry {
    static MOUNTED: OnceLock<Registry> = OnceLock::new();
    MOUNTED.get_or_init(Mutex::default)
}

/// Claim a preloaded seat or create a distinct guest for this view instance.
pub(super) fn mounted(module: &'static str, instance: u64) -> Arc<Mutex<Mounted>> {
    let mut registry = registry().lock().expect("module views");
    let seat = registry.remove(&(module, 0)).unwrap_or_else(Mounted::seat);
    registry.insert((module, instance), seat.clone());
    let snapshot = connection().lock().expect("views rpc").clone();
    let mut locked = seat.lock().expect("module view lock");
    locked.instance = instance;
    if let Slot::Ready(guest) = &mut locked.slot {
        guest.instance = instance;
    }
    if locked.generation == 0 && (snapshot.client.is_some() || view_override(module).is_some()) {
        locked.rev = snapshot.rev;
        let generation = locked.start();
        drop(locked);
        drop(spawn_load(module, &seat, generation, snapshot));
    } else {
        drop(locked);
    }
    seat
}

/// Retry, as a failed or stopped tab offers it: the seat's view is asked for
/// again now, under a new generation, past any hold-off a block's retries
/// left. A failure goes back to loading, and a stopped view gives up its
/// seat, so what lands is a fresh instance rather than a swap against it.
pub(crate) fn retry(module: &'static str, instance: u64) -> Loads {
    let registry = registry().lock().expect("module views");
    let Some(seat) = registry.get(&(module, instance)) else {
        return Loads {
            _threads: Vec::new(),
        };
    };
    let snapshot = connection().lock().expect("views rpc").clone();
    let mut locked = seat.lock().expect("module view lock");
    let stopped = matches!(&locked.slot, Slot::Ready(guest) if guest.fault.is_some());
    if stopped || matches!(locked.slot, Slot::Failed(_)) {
        locked.slot = Slot::Loading;
    }
    locked.retry = None;
    let generation = locked.start();
    drop(locked);
    Loads {
        _threads: vec![spawn_load(module, seat, generation, snapshot)],
    }
}

/// Loads the view on its own thread — a cold cranelift compile is a second
/// or more; the window thread shows "Loading" instead of freezing for it —
/// and installs it only if `mounted` still waits for this very load AND,
/// for a network's view, the app is still on the node it was asked of. A
/// view the developer's override supplies is the same on every node: its
/// load lands wherever the app has moved to meanwhile.
pub(super) fn spawn_load(
    module: &'static str,
    mounted: &Arc<Mutex<Mounted>>,
    generation: u64,
    asked_of: Connection,
) -> std::thread::JoinHandle<()> {
    let loading = mounted.clone();
    std::thread::spawn(move || {
        // the blob this load is asked for, as the roster reads compare it:
        // a failure is held off under this, not under the view section's
        // own hash, which never equals a blob id
        let asked_for = roster().code(module).map(|(code, _)| code_digest(&code));
        let mut timing = LoadTiming::default();
        let loaded = Guest::load(module, &asked_of, generation, &loading, &mut timing);
        // the window thread holds this lock while it ticks the seat
        let waited = Instant::now();
        let mut locked = loading.lock().expect("module view lock");
        timing.lock_wait = waited.elapsed();
        let key = crate::perf::Key::View {
            module,
            instance: locked.instance,
        };
        let installed = Instant::now();
        install(
            module,
            &mut locked,
            generation,
            &asked_of,
            asked_for,
            loaded,
        );
        timing.install = installed.elapsed();
        if timing.started.is_some() {
            crate::perf::record(
                key,
                "install.lock_wait",
                timing.lock_wait.as_micros() as u64,
            );
            crate::perf::record(key, "install", timing.install.as_micros() as u64);
            timing.log(module);
        }
    })
}

/// Puts what a load came back with into the seat, if the seat still waits
/// for this very load and, for a network's view, the app is still on the
/// node it was asked of. A swap also drops the old `Guest` here, its store
/// and memory with it.
fn install(
    module: &'static str,
    locked: &mut Mounted,
    generation: u64,
    asked_of: &Connection,
    asked_for: Option<[u8; 32]>,
    loaded: Result<Loaded, Unloaded>,
) {
    if locked.generation != generation {
        return;
    }
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
    // a load that came back at all clears the hold-off; only a failure
    // puts one back, widened against the one taken here
    let held_off = locked.retry.take();
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(unloaded) => return locked.load_failed(module, held_off, asked_for, unloaded),
    };
    let Mounted { slot, instance, .. } = &mut *locked;
    match loaded {
        Loaded::Fresh(mut guest) => {
            guest.instance = *instance;
            guest.installed_generation = Some(generation);
            guest.report_display_truncation();
            *slot = Slot::Ready(guest);
        }
        Loaded::Unchanged => {
            if let Slot::Ready(guest) = slot {
                guest.reconnect(current_rev);
            }
        }
        Loaded::Empty(_) => {
            *slot = Slot::Empty;
        }
        // the same tab, the same surface handle, the same host-side
        // input text and pictures: only the instance behind them moves
        Loaded::Swap {
            mut fresh,
            alive,
            ticks,
        } => {
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
                    wire::Node::Svg {
                        source: wire::SvgSource::Data { bytes, .. },
                        ..
                    } => *bytes = None,
                    wire::Node::Image { data, .. } => *data = None,
                    _ => {}
                });
            }
            fresh.pictures = std::mem::take(&mut old.pictures);
            fresh.instance = *instance;
            fresh.installed_generation = Some(generation);
            fresh.report_display_truncation();
            log_source(module, fresh.hash.as_ref(), "Swapped", generation, "");
            *slot = Slot::Ready(fresh);
        }
    }
}

/// What a load came back with.
pub(super) enum Loaded {
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
    /// The deployment ships no view. Nothing reads the hash any more; it
    /// goes when `Guest::load` (guest/lifecycle.rs) stops handing it over.
    #[allow(dead_code)]
    Empty([u8; 32]),
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
    module: &str,
    hash: Option<&[u8; 32]>,
    state: &str,
    generation: u64,
    reason: &str,
) {
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

/// How long each stage of one module-owned load took, and where its bytes
/// and code came from: `fetch` is the node's answer (`blob`: `disk` or
/// `node`), `compile` is cranelift (`code`: `memory`, `disk` or `cold`),
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
    pub(super) fn log(&self, module: &str) {
        let ms = |duration: Duration| duration.as_millis();
        let word = |word: &'static str| if word.is_empty() { "-" } else { word };
        let hash = self
            .hash
            .map_or_else(|| "-".to_owned(), |hash| crate::backend::hex_encode(&hash));
        let first_frame = self
            .first_frame
            .map_or_else(|| "-".to_owned(), |frame| ms(frame).to_string());
        let total = self.started.map_or(0, |started| ms(started.elapsed()));
        tracing::info!(
            target: "ducktape::perf",
            module,
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
pub(super) static VIEW_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets (or, with `None`, clears) the developer's override directory.
pub fn override_views_from(dir: Option<PathBuf>) {
    *VIEW_OVERRIDE.lock().expect("view override") = dir;
}

/// The file the developer's override supplies for `module`, if any.
pub(super) fn view_override(module: &str) -> Option<PathBuf> {
    let dir = VIEW_OVERRIDE.lock().expect("view override").clone()?;
    let path = dir.join(format!("{module}_view.wasm"));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests;
