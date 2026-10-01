use super::*;

mod abi;
mod lifecycle;
mod requests;

use abi::{Exports, HostState, first_line, panic_message};

// ---------- the guest ----------

/// What a fresh instance did with the state the drawn view left it. A trap
/// is not one of these — it takes the instance with it and is the load's
/// error. A refusal is the guest's own word, reported before it builds
/// anything, so the instance is untouched and can start clean instead.
pub(super) enum Restored {
    Carried,
    Refused(String),
}

/// One instantiated view: its wasm store and exports, the last frame it
/// sent, and everything the host keeps on its behalf between ticks — the
/// events owed to it, the live text of its inputs, its pictures, and the
/// subscriptions it opened. Every subscription here (`props`,
/// `visibility`, `offset`, `route`, `live`, `tasks`, `clocks`) has one
/// lifecycle: opened by a request, retired by its cancel in `redraw`, and
/// gone with the instance. A replacement prepared by `load` is a second
/// `Guest` seated over this one (`alive`, `staged`).
pub(super) struct Guest {
    /// Node requests belong to the network selected when this instance starts.
    pub(crate) connection_rev: u64,
    pub(crate) user_activation: Option<()>,
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
    pub(crate) frame_reports: display_diagnostics::FrameReports,
    pub(crate) display_diagnostics: display_diagnostics::DisplayDiagnostics,
    pub(crate) installed_generation: Option<u64>,
    /// Bumped when `frame.root` changes: the widget rebuilds when it sees a
    /// number it has not rendered.
    pub(crate) frame_rev: u64,
    pub(crate) ticks: u64,
    /// The live text of every input in the tree — the host's, not the guest's.
    pub(crate) inputs: EditorStore,
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

/// A tick may cancel up to twice what it may request.
pub(super) const MAX_CANCELS_PER_TICK: usize = 2 * MAX_REQUESTS_PER_TICK;

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

/// The tooltip in a node that route `request` builds, if it has one. A
/// plain tooltip caches the content alone; a rich text's caches the
/// character index it was built for too.
enum TooltipRoute<'a> {
    Plain(&'a mut wire::Tooltip),
    Rich(&'a mut wire::TooltipResponse),
}

fn tooltip_route(node: &mut wire::Node, request: u32) -> Option<TooltipRoute<'_>> {
    match node {
        wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
        | wire::Node::UniformList { interactivity, .. }
        | wire::Node::Image { interactivity, .. }
        | wire::Node::Svg { interactivity, .. } => interactivity
            .tooltip
            .as_mut()
            .filter(|tooltip| tooltip.request == request)
            .map(TooltipRoute::Plain),
        wire::Node::RichText {
            tooltip: Some(tooltip),
            ..
        } if tooltip.request == request => Some(TooltipRoute::Rich(tooltip)),
        _ => None,
    }
}

/// Brings the tree the host holds into `frame`: an `unchanged` frame takes
/// it as is, a frame without a tree patches it, a frame with one replaces
/// it. The flag is a tree the widget has to rebuild for; the report is what
/// sanitizing the result cut.
pub(super) fn merge(
    held: &mut Option<wire::Node>,
    frame: &mut wire::Frame,
) -> Result<(bool, wire::SanitizeReport), &'static str> {
    let mut tooltip_changed = false;
    if let Some(root) = held {
        let responses = std::mem::take(&mut frame.tooltip_responses);
        let mut response_ids = std::collections::HashSet::new();
        for response in &responses {
            if !response_ids.insert(response.request) {
                return Err("duplicate tooltip response request");
            }
            let mut matches = 0usize;
            let mut index_matches = true;
            root.for_each_mut(&mut |node| match tooltip_route(node, response.request) {
                Some(TooltipRoute::Plain(_)) => {
                    matches += 1;
                    index_matches &= response.character_index.is_none();
                }
                Some(TooltipRoute::Rich(_)) => {
                    matches += 1;
                    index_matches &= response.character_index.is_some();
                }
                None => {}
            });
            if matches > 1 {
                return Err("duplicate tooltip request route");
            }
            if matches == 1 && !index_matches {
                return Err("tooltip response index mismatch");
            }
        }
        for response in responses {
            let request = response.request;
            let mut response = Some(response);
            root.for_each_mut(&mut |node| {
                if response.is_none() {
                    return;
                }
                match tooltip_route(node, request) {
                    Some(TooltipRoute::Plain(tooltip)) => {
                        tooltip.content = response.take().unwrap().content;
                    }
                    Some(TooltipRoute::Rich(tooltip)) => {
                        let value = response.take().unwrap();
                        tooltip.character_index = value.character_index;
                        tooltip.content = value.content;
                    }
                    None => {}
                }
            });
            if response.is_none() {
                tooltip_changed = true;
            }
        }
    }
    if frame.unchanged {
        frame.root = held.take();
        let upstream = frame.upstream_sanitization;
        let report = wire::sanitize(frame)?;
        frame.upstream_sanitization = upstream;
        return Ok((tooltip_changed, report));
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

/// What the host is willing to take from one tick's bytes, already held to
/// `MAX_FRAME_BYTES`: nothing in here is trusted — the counts, the tree.
pub(super) fn shape(
    bytes: &[u8],
) -> Result<(wire::Frame, display_diagnostics::FrameReports), String> {
    let mut frame: wire::Frame = wire::decode(bytes)?;
    let requests_exceed_budget = frame.requests.len() > MAX_REQUESTS_PER_TICK;
    let cancels_exceed_budget = frame.cancels.len() > MAX_CANCELS_PER_TICK;
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
