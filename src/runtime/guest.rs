use super::*;

mod abi;
mod lifecycle;
mod requests;

use abi::{Exports, HostState, first_line, panic_message};

// ---------- the guest ----------

/// How long a press or key stays a view's activation: the web's transient
/// activation duration.
pub(crate) const ACTIVATION_EXPIRY: std::time::Duration = std::time::Duration::from_secs(5);

/// What a fresh instance did with the state the drawn view left it. A trap
/// is not one of these — it takes the instance with it and is the load's
/// error. A refusal is the guest's own word, reported before it builds
/// anything, so the instance is untouched and can start clean instead.
pub(super) enum Restored {
    Carried,
    Refused(String),
}

/// What a drawn view answered when asked for its state: the state, a
/// state past `MAX_SNAPSHOT_BYTES` (the host's word, before it copied a
/// byte), or the guest's own word that it cannot hand it over now. A trap
/// is not one of these: it ends the instance.
pub(super) enum Snapshot {
    Taken(Vec<u8>),
    TooLarge,
    Refused(String),
}

/// One instantiated view: its wasm store and exports, the last frame it
/// sent, and everything the host keeps on its behalf between ticks — the
/// events owed to it, its pictures, and the
/// subscriptions it opened. Every subscription here (`props`,
/// `visibility`, `offset`, `route`, `live`, `tasks`, `clocks`) has one
/// lifecycle: opened by a request, retired by its cancel in `redraw`, and
/// gone with the instance. A replacement prepared by `load` is a second
/// `Guest` seated over this one (`alive`, `staged`).
pub(super) struct Guest {
    /// Node requests belong to the network selected when this instance starts.
    pub(crate) connection_rev: u64,
    /// Transient user activation, as the web has it: when the host last
    /// received a real press or key aimed at this view's tree
    /// (`ViewTree::activate`, carried here by the seat's turn). Fresh for
    /// `ACTIVATION_EXPIRY`; a gated call (`link.open`, the clipboard) takes
    /// it, so one input admits one call (`take_activation`).
    pub(crate) activation: Option<std::time::Instant>,
    /// When each recent `link.open` was admitted, for the per-minute budget.
    pub(crate) links: Vec<std::time::Instant>,
    pub(crate) module: &'static str,
    /// The seat entity showing this seat (`Seat.instance`;
    /// 0 for a preloaded seat no tab has claimed): with `module`, the key
    /// this instance's perf samples land under.
    pub(crate) instance: u64,
    /// The manifest's name: what a registered view's tab is called.
    pub(crate) name: String,
    /// The capabilities the manifest declares: a method whose capability is
    /// not among them is refused before it is routed.
    pub(crate) capabilities: Vec<Capability>,
    /// The programs the manifest names as targets: a node method naming
    /// another is refused before it is routed.
    pub(crate) targets: Vec<String>,
    /// The manifest's `MIN_WINDOW_WIDTH`: the narrowest the view is laid
    /// out, and so the narrowest a window holding it is sized.
    pub(crate) min_width: u32,
    /// The undeclared capabilities already logged, so each is logged once.
    pub(crate) undeclared_logged: Vec<Capability>,
    store: Store<HostState>,
    exports: Exports,
    /// The guest's events for its next tick.
    pub(crate) pending: Vec<wire::Event>,
    pub(crate) theme_dark: Option<bool>,
    /// Requests wait for the native layout of a frame that still mounts
    /// their target, inside this instance only.
    pub(crate) widget_commands: Vec<(u64, wire::WidgetCommand)>,
    /// The last frame, its `root` kept across `unchanged` ticks and patched
    /// in place by a frame that carries patches instead of a tree.
    pub(crate) frame: wire::Frame,
    /// The style table of `frame.root`: a whole frame's entries, and each
    /// one a later frame brought.
    pub(crate) styles: wire::Styles,
    pub(crate) frame_reports: wire::SanitizeReport,
    pub(crate) display_diagnostics: display_diagnostics::DisplayDiagnostics,
    pub(crate) installed_generation: Option<u64>,
    /// Bumped when `frame.root` changes: the widget rebuilds when it sees a
    /// number it has not rendered.
    pub(crate) frame_rev: u64,
    /// What the guest built for tooltip routes since the seat last took
    /// them, each with the table of the frame it came in: for the tree the
    /// seat draws, which keeps them by route.
    pub(crate) tooltip_responses: Vec<(wire::TooltipResponse, wire::Styles)>,
    pub(crate) ticks: u64,
    /// Every picture the guest has sent, by hash: the bytes cross once.
    pub(crate) pictures: Pictures,
    /// The guest's `<module>.props` subscription, once it asked, and the
    /// props it was last given on it.
    pub(crate) props_subscription: Option<u64>,
    pub(crate) props_sent: Option<Vec<u8>>,
    pub(crate) visible: bool,
    pub(crate) visibility_change: Option<bool>,
    pub(crate) visibility_subscriptions: Vec<u64>,
    /// `host.offset` subscriptions, and the offset they were last handed.
    pub(crate) offset_subscriptions: Vec<u64>,
    pub(crate) offset_sent: Option<i32>,
    /// `host.route` subscriptions; the first is handed a pending link route.
    pub(crate) route_subscriptions: Vec<u64>,
    /// What the guest asked the app to do this redraw.
    pub(crate) intents: Vec<Intent>,
    /// The kernel's answers to this guest's node calls, on their way in.
    pub(crate) replies: Arc<kernel::Replies>,
    /// The guest's `module.changes` subscriptions and the program each one
    /// watches: told on every block that wrote to that program.
    pub(crate) live_subscriptions: Vec<(u64, String)>,
    /// Pending host requests and subscriptions, each owned by this guest
    /// the kernel opened for it: retired with the cancel, and with the guest.
    pub(crate) tasks: Vec<(u64, kernel::NodeTask)>,
    pub(crate) clipboard: clipboard::Clipboard,
    /// The guest's `clock.ticks` subscriptions: the period it asked for and
    /// the instant its next item is due. A module has no clock of its own,
    /// so periodic guest subscriptions use this list — driven from the window
    /// thread's own redraw, never from a thread that would have to wake it.
    pub(crate) clocks: Vec<kernel::Clock>,
    /// The trap that ended the view, if one did. A faulted guest never ticks again.
    pub(crate) fault: Option<String>,
    /// sha256 of the view bytes this instance was built from; none for a
    /// file the developer's override supplies.
    pub(crate) hash: Option<[u8; 32]>,
    /// This instance's identity: a replacement prepared against it is
    /// installed only over it.
    pub(crate) alive: Arc<()>,
    /// A replacement's first tree is in `frame` with its requests still
    /// to dispatch — the first redraw does that, without another tick.
    pub(crate) staged: bool,
}

impl Guest {
    pub(crate) fn perf_key(&self) -> crate::perf::Key {
        crate::perf::Key::View {
            module: self.module,
            instance: self.instance,
        }
    }

    /// How much of the budget the last call into the view took.
    pub(crate) fn fuel_used(&self) -> u64 {
        FUEL_PER_TICK - self.store.get_fuel().unwrap_or(0)
    }

    /// What the seat hands its tree to draw: the held tree with its style
    /// table, pictures named by hash alone, and the bytes those hashes
    /// resolve to, shared.
    pub(crate) fn drawn(&self) -> (crate::render::Tree, Arc<crate::render::PictureBytes>) {
        let tree = crate::render::Tree {
            root: self.frame.root.clone().unwrap_or_else(wire::Node::empty),
            styles: self.styles.clone(),
        };
        (tree, self.pictures.held())
    }
}

/// One `view_perf` line as an installed instance leaves: a swap drops the
/// old guest, a retry and a roster removal empty its slot.
impl Drop for Guest {
    fn drop(&mut self) {
        if self.installed_generation.is_some() {
            crate::perf::retire(self.module, self.instance);
        }
    }
}

pub(super) const COMPILED_VIEW_LIMIT: usize = 16;
pub(super) const COMPILED_VIEW_SOURCE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ViewCodeCache {
    entries: VecDeque<CompiledView>,
}

pub(super) struct CompiledView {
    hash: [u8; 32],
    source_bytes: usize,
    module: Arc<Module>,
}

impl ViewCodeCache {
    fn get(&mut self, hash: &[u8; 32]) -> Option<Arc<Module>> {
        let index = self.entries.iter().position(|entry| &entry.hash == hash)?;
        let entry = self.entries.remove(index)?;
        let module = entry.module.clone();
        self.entries.push_back(entry);
        Some(module)
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

/// The view's code, and where it came from: `memory` for the in-process
/// cache, `disk` for wasmtime's compile cache, `cold` for a cranelift run.
/// ponytail: the disk/cold split reads wasmtime's process-wide hit counter
/// around the compile, and loaders compile concurrently on purpose, so a
/// hit landing from another thread can name a cold compile `disk`.
pub(super) fn compiled_view(bytes: &[u8]) -> Result<(Arc<Module>, &'static str), String> {
    static CODE: OnceLock<Mutex<ViewCodeCache>> = OnceLock::new();
    let (engine, disk) = runtime();
    let hits_before = disk.as_ref().map(Cache::cache_hits);
    let (module, compiled) = compile_view(engine, CODE.get_or_init(Mutex::default), bytes)?;
    let source = match (compiled, hits_before, disk.as_ref().map(Cache::cache_hits)) {
        (false, _, _) => "memory",
        (true, Some(before), Some(after)) if after > before => "disk",
        (true, _, _) => "cold",
    };
    Ok((module, source))
}

// Cache code only: every load still creates its own Store, instance and assets.
// The cache belongs to this one Engine. Compile outside the lock so unrelated
// views can prepare concurrently; a competing result adopts the existing entry.
// The flag says whether this call ran `Module::new`.
pub(super) fn compile_view(
    engine: &Engine,
    cache: &Mutex<ViewCodeCache>,
    bytes: &[u8],
) -> Result<(Arc<Module>, bool), String> {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes).into();
    if let Some(module) = cache.lock().expect("view code cache").get(&hash) {
        return Ok((module, false));
    }
    let module = Arc::new(Module::new(engine, bytes).map_err(|error| error.to_string())?);
    let mut cache = cache.lock().expect("view code cache");
    if let Some(existing) = cache.get(&hash) {
        return Ok((existing, true));
    }
    cache.insert(CompiledView {
        hash,
        source_bytes: bytes.len(),
        module: module.clone(),
    });
    Ok((module, true))
}

pub(super) fn engine() -> &'static Engine {
    &runtime().0
}

/// The one wasmtime engine, and a handle on its compile cache: `Cache` is
/// `Clone`, and the clone reads the hit counters the engine's copy writes.
fn runtime() -> &'static (Engine, Option<Cache>) {
    static ENGINE: OnceLock<(Engine, Option<Cache>)> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.cranelift_opt_level(OptLevel::Speed);
        config.consume_fuel(true);
        let mut disk = None;
        match crate::backend::cache_dir() {
            Ok(directory) => {
                let mut cache = CacheConfig::new();
                cache.with_directory(directory.join("view-code"));
                match Cache::new(cache) {
                    Ok(cache) => {
                        disk = Some(cache.clone());
                        config.cache(Some(cache));
                    }
                    Err(error) => tracing::warn!(reason = "view_cache_unavailable", %error),
                }
            }
            Err(error) => tracing::warn!(reason = "view_cache_directory_unavailable", %error),
        }
        (Engine::new(&config).expect("wasmtime engine"), disk)
    })
}

/// Reset the instruction allowance before entering the guest.
fn arm(store: &mut Store<HostState>) {
    let _ = store.set_fuel(FUEL_PER_TICK);
}

/// Brings the tree the host holds into `frame`: an `unchanged` frame takes
/// it as is, a frame without a tree patches it, a frame with one replaces
/// it. The flag is a tree the widget has to rebuild for; the report is what
/// sanitizing the result cut.
pub(super) fn merge(
    held: &mut Option<wire::Node>,
    frame: &mut wire::Frame,
    styles: &wire::Styles,
) -> Result<(bool, wire::SanitizeReport), wire::Refused> {
    if frame.unchanged {
        // the held tree passed when it arrived: taken as it is
        frame.root = held.take();
        return Ok((false, Default::default()));
    }
    if frame.root.is_some() {
        return Ok((true, Default::default()));
    }
    let patches = std::mem::take(&mut frame.patches);
    let mut root = held.as_ref().ok_or("no tree to patch")?.clone();
    let report = wire::apply(&mut root, patches, styles)?;
    frame.root = Some(root);
    Ok((true, report))
}

/// What the host is willing to take from one tick's bytes, already held to
/// `MAX_FRAME_BYTES`: nothing in here is trusted — the counts, the tree.
/// The frame's style entries join `styles`, the table of the tree the host
/// holds (a whole tree's replace it); a refused frame leaves it as it was.
pub(super) fn shape(
    bytes: &[u8],
    styles: &mut wire::Styles,
) -> Result<(wire::Frame, wire::SanitizeReport), String> {
    // a frame over `MAX_REQUESTS` or `MAX_CANCELS` does not decode
    let mut frame: wire::Frame = wire::decode(bytes)?;
    if frame.unchanged {
        frame.root = None;
    }
    if frame.unchanged || frame.root.is_some() {
        frame.patches = Vec::new();
    }
    let cuts = wire::sanitize(&mut frame, styles).map_err(|refused| refused.to_string())?;
    Ok((frame, cuts))
}

/// A view built against another wire than this app's, as the plain refusal
/// sentence.
pub(super) fn wire_id(id: &str) -> Result<(), String> {
    (id == wire::WIRE_ID).then_some(()).ok_or_else(|| {
        format!(
            "this view was built against wire {id}; this app speaks {}",
            wire::WIRE_ID
        )
    })
}

#[cfg(test)]
mod tests;
