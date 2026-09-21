use super::*;

/// The loads an event started; nobody waits on them, the seats swap in place.
pub struct Loads(#[allow(dead_code)] pub(super) Vec<std::thread::JoinHandle<()>>);

/// The node's deployments moved (a new block): every module-owned view
/// whose module's active code is not the one it was drawn from is loaded
/// again, under a new generation, and swapped in place when it is ready.
/// One check in flight at a time; a block that lands during one is
/// covered by the next. The loads it starts swap in place, so nobody waits
/// on them but a test; the block stream drops them.
pub(super) struct Mounted {
    pub(super) changes: tokio::sync::watch::Sender<()>,
    /// The connection this seat was last asked of.
    pub(super) rev: u64,
    pub(super) slot: Slot,
    pub(super) props: Option<Vec<u8>>,
    pub(super) generation: u64,
    /// The deployment the slot answers for — the view drawn, or the empty
    /// slot of a deployment without one — so a block moves it only when the
    /// active code moved. A load that failed never seated anything, so it
    /// leaves this alone and is held off by `retry` instead.
    pub(super) hash: Option<[u8; 32]>,
    /// A load is on its way for `generation`, and the deployment it is
    /// after when a block named one: a block that names it again waits
    /// for it instead of starting over.
    pub(super) in_flight: bool,
    /// The proposed frame this device tastes in place of the active one:
    /// the seat's wanted hash is `tasting.or(active)`. Cleared, with a
    /// notice, when the hash leaves the taste set — withdrawn, or
    /// activated into the very hash the seat already draws.
    /// The candidate a load last failed on, and when the next block may try
    /// it again. Cleared by any load that comes back, so only a repeated
    /// failure on the same candidate widens the gap.
    pub(super) retry: Option<Retry>,
    /// When a tab last drew this seat: a load for a seat on screen does not
    /// queue for the link.
    pub(super) shown: Option<Instant>,
}

/// A failed load's hold-off: the candidate it failed on, when the next
/// attempt at that same candidate is due, and the gap that produced it.
pub(super) struct Retry {
    pub(super) hash: Option<[u8; 32]>,
    pub(super) next: Instant,
    pub(super) gap: Duration,
}

impl Retry {
    /// The hold-off after a load for `hash` failed: the gap doubles up to
    /// `RETRY_MAX` while the same candidate keeps failing, and starts over
    /// at `RETRY_FIRST` for a different one.
    pub(super) fn after(previous: Option<&Retry>, hash: Option<[u8; 32]>) -> Retry {
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
    pub(super) fn seat() -> Arc<Mutex<Self>> {
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
    pub(super) fn show(&mut self, generation: u64, stage: Slot) {
        if self.generation == generation && self.slot.loading() {
            self.slot = stage;
        }
    }

    /// A retry is scheduled and not yet due.
    pub(super) fn held_off_now(&self) -> bool {
        self.retry
            .as_ref()
            .is_some_and(|retry| Instant::now() < retry.next)
    }

    /// Whether a block naming `active` is still inside the hold-off a
    /// failed load for that same candidate left behind.
    pub(super) fn held_off(&self, active: Option<[u8; 32]>) -> bool {
        self.retry
            .as_ref()
            .is_some_and(|retry| retry.hash == active && Instant::now() < retry.next)
    }

    /// Opens the next generation for a load after `wanted` (None: whatever
    /// the node holds active), and names it.
    pub(super) fn start(&mut self) -> u64 {
        self.changes.send_replace(());
        self.generation += 1;
        self.in_flight = true;
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
    /// The view speaks a wire epoch this app does not.
    WireEpoch(String),
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
            Failure::WireEpoch(_) => "This view speaks a wire this app does not",
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
        | Failure::WireEpoch(reason)) = self;
        formatter.write_str(reason)
    }
}

pub(super) type Registry = Mutex<HashMap<&'static str, Arc<Mutex<Mounted>>>>;

pub(super) fn registry() -> &'static Registry {
    static MOUNTED: OnceLock<Registry> = OnceLock::new();
    MOUNTED.get_or_init(Mutex::default)
}

/// The seat of `module`'s view, made on its first ask. Making it asks for
/// nothing: a load starts at the view's source event, never at a draw, so
/// a seat a tab is first to ask for waits for the next [`connected`] or
/// [`deployments_checked`] like every other.
pub(super) fn mounted(module: &'static str) -> Arc<Mutex<Mounted>> {
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
pub(super) fn spawn_load(
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
    /// The deployment (this hash) ships no view.
    Empty([u8; 32]),
}

/// A load that came back with no view, and the candidate it failed on —
/// none when it never got as far as one — so the block check's hold-off
/// keys on the bytes that failed, never on what the load was asked after.
pub(super) struct Unloaded {
    pub(super) hash: Option<[u8; 32]>,
    pub(super) failure: Failure,
}

/// One `view_source` line per outcome, with the same fields every time —
/// the canary greps them.
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

/// How long each stage of one module-owned load took: `status` and
/// `fetch` are the node's answers, `compile` is cranelift, `init` the
/// instance and its `on mount` or restore, `first_frame` the tree a
/// replacement proves (a fresh view draws its first on the window thread),
/// `check` the second look at the registry before the seat. `path` is
/// `first` for a load over an empty slot, `swap` over a view drawn.
#[derive(Default)]
pub(super) struct LoadTiming {
    pub(super) path: &'static str,
    pub(super) status: Duration,
    pub(super) fetch: Duration,
    pub(super) compile: Duration,
    pub(super) init: Duration,
    pub(super) first_frame: Option<Duration>,
    pub(super) check: Duration,
}

impl LoadTiming {
    /// One `view_load` line per load, every field every time, through the
    /// same logger and test tap as `view_source`.
    pub(super) fn log(&self, module: &str, hash: Option<&[u8; 32]>, started: Instant, state: &str) {
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
