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

/// The guest's `restore(state, macos)` export.
pub(super) type Restore = TypedFunc<(Vec<u8>, bool), (Result<(), String>,)>;

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
    pub(crate) store: Store<HostState>,
    pub(crate) tick: TypedFunc<(Vec<u8>,), (Vec<u8>,)>,
    /// The guest's events for its next tick.
    pub(crate) pending: Vec<wire::Event>,
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
    /// What the guest asked the app to do this redraw.
    pub(crate) intents: Vec<ModuleViewEvent>,
    /// The kernel's answers to this guest's node calls, on their way in.
    pub(crate) replies: Arc<kernel::Replies>,
    /// The guest's `rpc.live` subscriptions, each with the plane it named:
    /// told on every block that moves that plane.
    pub(crate) live_subscriptions: Vec<(u64, String)>,
    /// Pending host requests and subscriptions, each owned by this guest
    /// the kernel opened for it: retired with the cancel, and with the guest.
    pub(crate) tasks: Vec<(u64, kernel::NodeTask)>,
    pub(crate) filesystem: filesystem::Filesystem,
    /// The capture and playout devices this guest holds, and the one consent
    /// answer they all wait on.
    pub(crate) media: media::Media,
    /// The guest's `clock.ticks` subscriptions: the period it asked for and
    /// the instant its next item is due. A module has no clock of its own,
    /// so periodic guest subscriptions use this list — driven from the window
    /// thread's own redraw, never from a thread that would have to wake it.
    pub(crate) clocks: Vec<kernel::Clock>,
    /// The chords this guest claimed, each with the subscription its presses
    /// arrive on. The claim itself is global ([`claim_chord`]): two seats
    /// cannot answer to one chord.
    pub(crate) chords: Vec<(u64, String)>,
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
    pub(crate) snapshot: TypedFunc<(), (Result<Vec<u8>, String>,)>,
    pub(crate) restore: Restore,
    pub(crate) init: TypedFunc<(bool,), ()>,
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

pub(super) fn compiled_view(bytes: &[u8]) -> Result<Arc<Component>, String> {
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

/// Brings the tree the host holds into `frame`: an `unchanged` frame takes
/// it as is, a frame without a tree patches it, a frame with one replaces
/// it. `Ok(true)` is a tree the widget has to rebuild for.
pub(super) fn merge(
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
