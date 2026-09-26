use super::*;

mod lifecycle;
mod requests;

// ---------- the guest ----------

/// What a view's store holds: its limits, and the message its panic hook
/// handed over before the trap that follows.
pub(super) struct HostState {
    limits: StoreLimits,
    panic: Option<String>,
}

/// The guest's exports and the memory their bytes cross in (`wire::abi`).
pub(super) struct Exports {
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    init: TypedFunc<u32, ()>,
    tick: TypedFunc<(u32, u32), u64>,
    snapshot: TypedFunc<(), u64>,
    restore: TypedFunc<(u32, u32, u32), u64>,
}

impl Exports {
    pub(super) fn bind(
        store: &mut Store<HostState>,
        instance: &wasmtime::Instance,
    ) -> wasmtime::Result<Self> {
        Ok(Self {
            memory: instance
                .get_memory(&mut *store, "memory")
                .ok_or_else(|| wasmtime::Error::msg("the view exports no memory"))?,
            alloc: instance.get_typed_func(&mut *store, "alloc")?,
            init: instance.get_typed_func(&mut *store, "init")?,
            tick: instance.get_typed_func(&mut *store, "tick")?,
            snapshot: instance.get_typed_func(&mut *store, "snapshot")?,
            restore: instance.get_typed_func(&mut *store, "restore")?,
        })
    }

    /// `bytes` in a buffer the guest allocated and owns from the next call on.
    fn give(&self, store: &mut Store<HostState>, bytes: &[u8]) -> wasmtime::Result<(u32, u32)> {
        let len = u32::try_from(bytes.len())?;
        let ptr = self.alloc.call(&mut *store, len)?;
        self.memory.write(&mut *store, ptr as usize, bytes)?;
        Ok((ptr, len))
    }

    /// The bytes an answer names, copied out before the guest is entered
    /// again; nothing about the pair is trusted.
    fn answer(&self, store: &Store<HostState>, packed: u64) -> wasmtime::Result<Vec<u8>> {
        let (ptr, len) = wire::abi::unpack(packed);
        let start = ptr as usize;
        self.memory
            .data(store)
            .get(start..start.saturating_add(len as usize))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| wasmtime::Error::msg("the view answered outside its memory"))
    }

    fn result(
        &self,
        store: &Store<HostState>,
        packed: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        wire::abi::decode_result(&self.answer(store, packed)?)
            .ok_or_else(|| wasmtime::Error::msg("the view's answer is not a result"))
    }

    pub(super) fn init(&self, store: &mut Store<HostState>, macos: bool) -> wasmtime::Result<()> {
        self.init.call(store, u32::from(macos))
    }

    pub(super) fn tick(
        &self,
        store: &mut Store<HostState>,
        events: &[u8],
    ) -> wasmtime::Result<Vec<u8>> {
        let (ptr, len) = self.give(store, events)?;
        let packed = self.tick.call(&mut *store, (ptr, len))?;
        self.answer(store, packed)
    }

    pub(super) fn snapshot(
        &self,
        store: &mut Store<HostState>,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        let packed = self.snapshot.call(&mut *store, ())?;
        self.result(store, packed)
    }

    pub(super) fn restore(
        &self,
        store: &mut Store<HostState>,
        state: &[u8],
        macos: bool,
    ) -> wasmtime::Result<Result<(), String>> {
        let (ptr, len) = self.give(store, state)?;
        let packed = self
            .restore
            .call(&mut *store, (ptr, len, u32::from(macos)))?;
        Ok(self.result(store, packed)?.map(|_| ()))
    }
}

/// What a fresh instance did with the state the drawn view left it. A trap
/// is not one of these — it takes the instance with it and is the load's
/// error. A refusal is the guest's own word, reported before it builds
/// anything, so the instance is untouched and can start clean instead.
pub(super) enum Restored {
    Carried,
    Refused(String),
}

pub(super) struct Guest {
    /// Node requests belong to the network selected when this instance starts.
    pub(crate) connection_rev: u64,
    pub(crate) user_activation: Option<()>,
    pub(crate) module: &'static str,
    /// The manifest's name: what a registered view's tab is called.
    pub(crate) name: String,
    /// The capabilities the manifest declares: a method whose capability is
    /// not among them is refused before it is routed.
    pub(crate) capabilities: Vec<String>,
    /// The undeclared capabilities already logged, so each is logged once.
    pub(crate) undeclared_logged: Vec<String>,
    pub(crate) store: Store<HostState>,
    pub(crate) exports: Exports,
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
    /// `host.route` subscriptions; the first is handed a pending link route.
    pub(crate) route_subscriptions: Vec<u64>,
    /// What the guest asked the app to do this redraw.
    pub(crate) intents: Vec<Intent>,
    /// The kernel's answers to this guest's node calls, on their way in.
    pub(crate) replies: Arc<kernel::Replies>,
    /// The guest's `module.changes` subscriptions, each with the plane it named:
    /// told on every block that moves that plane.
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
    /// The assets the deployment shipped beside this view, for the host
    /// surfaces that paint them by canonical relative path; swapped with the
    /// instance as one unit. Empty for a view the developer's override supplies.
    /// The deployment this instance came from; none for a file the
    /// developer's override supplies.
    pub(crate) hash: Option<[u8; 32]>,
    /// This instance's identity: a replacement prepared against it is
    /// installed only over it.
    pub(crate) alive: Arc<()>,
    /// A replacement's first tree is in `frame` with its requests still
    /// to dispatch — the first redraw does that, without another tick.
    pub(crate) staged: bool,
}

/// The asset at `path` in a deployment's map: the canonical relative path,
/// exactly — no normalisation, no file, no network.
pub(super) fn hex_short(hash: &[u8; 32]) -> String {
    hash[..6].iter().map(|byte| format!("{byte:02x}")).collect()
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

pub(super) fn compiled_view(bytes: &[u8]) -> Result<Arc<Module>, String> {
    static CODE: OnceLock<Mutex<ViewCodeCache>> = OnceLock::new();
    compile_view(engine(), CODE.get_or_init(Mutex::default), bytes)
}

// Cache code only: every load still creates its own Store, instance and assets.
// The cache belongs to this one Engine. Compile outside the lock so unrelated
// views can prepare concurrently; a competing result adopts the existing entry.
pub(super) fn compile_view(
    engine: &Engine,
    cache: &Mutex<ViewCodeCache>,
    bytes: &[u8],
) -> Result<Arc<Module>, String> {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes).into();
    if let Some(module) = cache.lock().expect("view code cache").get(&hash) {
        return Ok(module);
    }
    let module = Arc::new(Module::new(engine, bytes).map_err(|error| error.to_string())?);
    let mut cache = cache.lock().expect("view code cache");
    if let Some(existing) = cache.get(&hash) {
        return Ok(existing);
    }
    cache.insert(CompiledView {
        hash,
        source_bytes: bytes.len(),
        module: module.clone(),
    });
    Ok(module)
}

pub(super) fn engine() -> &'static Engine {
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
pub(super) fn arm(store: &mut Store<HostState>) {
    let _ = store.set_fuel(FUEL_PER_TICK);
}

/// Brings the tree the host holds into `frame`: an `unchanged` frame takes
/// it as is, a frame without a tree patches it, a frame with one replaces
/// it. `Ok(true)` is a tree the widget has to rebuild for.
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
            root.for_each_mut(&mut |node| match node {
                wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
                | wire::Node::UniformList { interactivity, .. }
                | wire::Node::Image { interactivity, .. }
                | wire::Node::Svg { interactivity, .. } => {
                    if interactivity
                        .tooltip
                        .as_ref()
                        .is_some_and(|tooltip| tooltip.request == response.request)
                    {
                        matches += 1;
                        index_matches &= response.character_index.is_none();
                    }
                }
                wire::Node::RichText {
                    tooltip: Some(tooltip),
                    ..
                } if tooltip.request == response.request => {
                    matches += 1;
                    index_matches &= response.character_index.is_some();
                }
                _ => {}
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
                match node {
                    wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
                    | wire::Node::UniformList { interactivity, .. }
                    | wire::Node::Image { interactivity, .. }
                    | wire::Node::Svg { interactivity, .. }
                        if interactivity
                            .tooltip
                            .as_ref()
                            .is_some_and(|tooltip| tooltip.request == request) =>
                    {
                        interactivity.tooltip.as_mut().unwrap().content =
                            response.take().unwrap().content;
                    }
                    wire::Node::RichText {
                        tooltip: Some(tooltip),
                        ..
                    } if tooltip.request == request => {
                        let value = response.take().unwrap();
                        tooltip.character_index = value.character_index;
                        tooltip.content = value.content;
                    }
                    _ => {}
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

/// What the host is willing to take from one tick's bytes: nothing in here
/// is trusted — the length, the counts, the tree.
pub(super) fn shape(
    bytes: &[u8],
) -> Result<(wire::Frame, display_diagnostics::FrameReports), String> {
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

pub(super) fn panic_message(store: &mut Store<HostState>) -> Option<String> {
    let text = store.data_mut().panic.take()?;
    (!text.is_empty()).then_some(text)
}

/// Why a call failed: the trap itself, not the wrapper and backtrace
/// wasmtime prints around it.
pub(super) fn first_line(error: &wasmtime::Error) -> String {
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
pub(super) fn wire_epoch(epoch: u32) -> Result<(), String> {
    (epoch == wire::WIRE_EPOCH).then_some(()).ok_or_else(|| {
        format!(
            "this view speaks wire epoch {epoch}; this app speaks {}",
            wire::WIRE_EPOCH
        )
    })
}

/// The methods a view was built against, refused at load when this app has
/// fewer: the view would otherwise run until it asks for one.
pub(super) fn methods_revision(needed: u32) -> Result<(), String> {
    (needed <= wire::methods::METHODS_REVISION)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "this view needs methods revision {needed}; this app has {}",
                wire::methods::METHODS_REVISION
            )
        })
}

#[cfg(test)]
mod tests;
