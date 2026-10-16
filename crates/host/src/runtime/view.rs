use super::*;

mod lifecycle;
mod requests;

// ---------- the view ----------

/// How long a press or key stays a view's activation: the web's transient
/// activation duration.
pub const ACTIVATION_EXPIRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Where a redraw stands after its first half ([`View::begin_redraw`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Redraw {
    /// Nothing to deliver: no tick, nothing to answer.
    Quiet,
    /// No tick due, the requests still to route (a replacement's first
    /// tree): [`View::finish_redraw`] next.
    Ready,
    /// A tick begun: its frame lands, then [`View::finish_redraw`].
    Ticking,
}

/// What a fresh instance did with the state the drawn view left it. A trap
/// is not one of these — it takes the instance with it and is the load's
/// error. A refusal is the view's own word, reported before it builds
/// anything, so the instance is untouched and can start clean instead. A
/// death (`Fault::Runner`, its page gone) is no answer of the view's: the
/// state is still in hand, and the replacement waits for another try.
pub(super) enum Restored {
    Carried,
    Refused(String),
    Died(String),
}

/// What a death replay starts from (SPEC §6.5): the last accepted snapshot
/// (`None`: the view's `init`), when it was taken, and every tick's events
/// since, in order. An instance that dies is restored from it and replayed
/// to where it was (`revive`). `asked`: the view's tick count when a new
/// snapshot was last asked for, so one is asked at most once per tick,
/// whatever it answered (taken, refused, or the page died).
#[derive(Clone)]
pub(super) struct Kept {
    pub(super) snapshot: Option<Vec<u8>>,
    pub(super) at: Instant,
    pub(super) batches: Vec<Vec<u8>>,
    pub(super) asked: Option<u64>,
}

impl Kept {
    /// A fresh start's: replayed from `init`.
    fn new() -> Self {
        Self {
            snapshot: None,
            at: Instant::now(),
            batches: Vec::new(),
            asked: None,
        }
    }

    /// Whether the host keeps a replay for this view: a new instance's.
    pub(super) fn if_kept() -> Option<Self> {
        crate::contract::os()
            .limits()
            .snapshot_every
            .map(|_| Self::new())
    }
}

/// What a drawn view answered when asked for its state: the state, or the
/// view's own word that it cannot hand it over now. A trap is not one of
/// these: it ends the instance.
pub(super) enum Snapshot {
    Taken(Vec<u8>),
    Refused(String),
}

/// One instantiated view: its wasm store and exports, the last frame it
/// sent, and everything the host keeps on its behalf between ticks — the
/// events owed to it, its pictures, and the
/// subscriptions it opened. Every subscription here (`props`,
/// `visibility`, `offset`, `route`, `live`, `tasks`, `clocks`) has one
/// lifecycle: opened by a request, retired by its cancel in `redraw`, and
/// gone with the instance. A replacement prepared by `load` is a second
/// `View` seated over this one (`alive`, `staged`).
pub(super) struct View {
    /// The connection this instance started on, or was last reconnected
    /// to: its node requests and its `store` file go there, and only while
    /// it is still the app's (the same `rev`).
    pub connection: Connection,
    /// Transient user activation, as the web has it: when the host last
    /// received a real press or key aimed at this view's tree
    /// (`ViewTree::activate`, carried here by the seat's turn). Fresh for
    /// `ACTIVATION_EXPIRY`; a gated call (`link.open`, the clipboard) takes
    /// it, so one input admits one call (`take_activation`).
    pub activation: Option<std::time::Instant>,
    pub program: &'static str,
    /// The seat entity showing this seat (`Seat.instance`;
    /// 0 for a preloaded seat no tab has claimed): with `program`, the key
    /// this instance's perf samples land under.
    pub instance: u64,
    /// The manifest's name: what a registered view's tab is called.
    pub name: String,
    /// The manifest's icon: what the view's tab draws on a folded menu bar;
    /// empty when the view declares none.
    pub icon: String,
    /// The manifest's description: what Home's tile and product page say.
    pub description: String,
    /// The capabilities the manifest declares: a method whose capability is
    /// not among them is refused before it is routed.
    pub capabilities: Vec<Capability>,
    /// The programs the manifest names as targets: a node method naming
    /// another is refused before it is routed.
    pub targets: Vec<String>,
    /// The manifest's `MIN_WINDOW_WIDTH`: the narrowest the view is laid
    /// out, and so the narrowest a window holding it is sized.
    pub min_width: u32,
    /// The manifest's `wire_id`: the wire the view was built against. A
    /// view built against another than this app's runs all the same, and
    /// whatever then fails says so ([`View::failure`]).
    pub wire: String,
    /// The undeclared capabilities already logged, so each is logged once.
    pub undeclared_logged: Vec<Capability>,
    /// The running instance (`contract::ViewInstance`), and the runner that
    /// seated it: a sibling (a fresh start after a failed restore) is
    /// seated by the same one.
    running: Box<dyn ViewInstance>,
    runner: Arc<dyn ViewRunner>,
    /// The instance's answers (`contract::Landing`), drained after each
    /// call begun.
    landing: async_channel::Receiver<Landed>,
    /// The tick in flight's `tick.call` timer: from its call to its landing.
    tick_call: Option<crate::perf::Timer>,
    /// A tick begun, its frame not landed: whichever seat turns next waits
    /// for it before it begins another.
    ticking: bool,
    /// What a death replay starts from, where the host keeps one
    /// (`Limits::snapshot_every`; none on the desktop).
    pub(super) kept: Option<Kept>,
    /// A page's death, taken off its landing where it could not be taken up
    /// again at once (`View::revive` begins calls). The tick in flight's,
    /// in the background or in `Seat::hide`'s last update: the tick stays
    /// in flight, and the turn that next takes the view up (in front again)
    /// starts its wait from this. One outside a tick, under a kept
    /// snapshot or a swap's handover: the seat's next turn in front takes
    /// it up before anything else begins a call (`seat::take_up`).
    pub(super) parked: Option<Landed>,
    /// The view's events for its next tick.
    pub pending: Vec<wire::Event>,
    /// The host's facts as the view last heard them: the theme, whether
    /// the device's pointer is a finger, the body it is laid out in, the
    /// reader's UTC offset. Each crosses once (`sync_theme`,
    /// `sync_pointer`, `sync_viewport`, `sync_offset`), and a replacement
    /// hears them before its first frame (`seed`).
    pub theme_dark: Option<bool>,
    pub touch_sent: Option<bool>,
    pub viewport_sent: Option<(f32, f32)>,
    pub offset_sent: Option<i32>,
    /// The events every tick carried, in order, for tests of what a view
    /// knew when it drew.
    #[cfg(any(test, feature = "test-support"))]
    pub ticked_with: Vec<Vec<wire::Event>>,
    /// Requests wait for the native layout of a frame that still mounts
    /// their target, inside this instance only.
    pub widget_commands: Vec<(u64, wire::WidgetCommand)>,
    /// The last frame, its `root` kept across `unchanged` ticks and patched
    /// in place by a frame that carries patches instead of a tree.
    pub frame: wire::Frame,
    /// The style table of `frame.root`: a whole frame's entries, and each
    /// one a later frame brought.
    pub styles: wire::Styles,
    pub frame_reports: wire::SanitizeReport,
    pub display_diagnostics: display_diagnostics::DisplayDiagnostics,
    pub installed_generation: Option<u64>,
    /// Bumped when `frame.root` changes: the widget rebuilds when it sees a
    /// number it has not rendered.
    pub frame_rev: u64,
    /// What the view built for tooltip routes since the seat last took
    /// them, each with the table of the frame it came in: for the tree the
    /// seat draws, which keeps them by route.
    pub tooltip_responses: Vec<(wire::TooltipResponse, wire::Styles)>,
    pub ticks: u64,
    /// Every picture the view has sent, by hash: the bytes cross once.
    pub pictures: Pictures,
    /// The view's `<program>.props` subscription, once it asked, and the
    /// props it was last given on it.
    pub props_subscription: Option<u64>,
    pub props_sent: Option<Vec<u8>>,
    pub visible: bool,
    pub visibility_change: Option<bool>,
    pub visibility_subscriptions: Vec<u64>,
    /// `host.route` subscriptions; the first is handed a pending link route.
    pub route_subscriptions: Vec<u64>,
    /// What the view asked the app to do this redraw.
    pub intents: Vec<Intent>,
    /// The kernel's answers to this view's node calls, on their way in.
    pub replies: Arc<kernel::Replies>,
    /// The view's `program.changes` subscriptions and the program each one
    /// watches: told on every block that wrote to that program.
    pub live_subscriptions: Vec<(u64, String)>,
    /// The tracks this view publishes, where its `live.send`s go.
    pub tracks: kernel::live::Tracks,
    /// Pending host requests and subscriptions, each owned by this view
    /// the kernel opened for it: retired with the cancel, and with the view.
    pub tasks: Vec<(u64, kernel::NodeTask)>,
    pub clipboard: clipboard::Clipboard,
    /// The view's `clock.ticks` subscriptions: the period it asked for and
    /// the instant its next item is due. A view has no clock of its own,
    /// so periodic view subscriptions use this list — driven from the window
    /// thread's own redraw, never from a thread that would have to wake it.
    pub clocks: Vec<kernel::Clock>,
    /// The trap that ended the view, if one did. A faulted view never ticks again.
    pub fault: Option<String>,
    /// sha256 of the view bytes this instance was built from; none for a
    /// file the developer's override supplies.
    pub hash: Option<[u8; 32]>,
    /// This instance's identity: a replacement prepared against it is
    /// installed only over it.
    pub alive: Arc<()>,
    /// A replacement's first tree is in `frame` with its requests still
    /// to dispatch — the first redraw does that, without another tick.
    pub staged: bool,
}

impl View {
    pub fn perf_key(&self) -> crate::perf::Key {
        crate::perf::Key::View {
            program: self.program,
            instance: self.instance,
        }
    }

    /// How much of the budget the last call into the view took.
    pub fn fuel_used(&self) -> u64 {
        self.running.fuel_used()
    }

    /// The answer to the call just begun, when it lands: before the call
    /// returned on the wasmtime runner, later on one that answers across a
    /// process boundary.
    async fn landed(&mut self) -> Landed {
        next(&self.landing).await
    }

    /// The answer to the call just begun, taken at once, as a test takes it
    /// on the wasmtime runner, which lands it before the call returns.
    #[cfg(test)]
    fn at_once(&mut self) -> Landed {
        self.landing.try_recv().unwrap_or_else(|_| {
            Landed::Fault(Fault::Runner("the runner did not answer at once".into()))
        })
    }

    /// The wasmtime instance's linear memory, for the tests that read it.
    #[cfg(test)]
    pub fn memory_for_test(&self) -> &[u8] {
        self.running.memory_for_test()
    }

    /// [`Self::memory_for_test`], for the tests that script it.
    #[cfg(test)]
    pub fn memory_mut_for_test(&mut self) -> &mut [u8] {
        self.running.memory_mut_for_test()
    }

    /// What the seat hands its tree to draw: the held tree with its style
    /// table, pictures named by hash alone, and the bytes those hashes
    /// resolve to, shared.
    pub fn drawn(&self) -> (crate::render::Tree, Arc<crate::render::PictureBytes>) {
        let tree = crate::render::Tree {
            root: self.frame.root.clone().unwrap_or_else(wire::Node::empty),
            styles: self.styles.clone(),
        };
        (tree, self.pictures.held())
    }
}

/// The next answer on `landing`. An instance that stopped (its landing
/// closed) answers nothing more: the call it had out failed as the
/// runner's, as a page's death does.
pub(super) async fn next(landing: &async_channel::Receiver<Landed>) -> Landed {
    landing.recv().await.unwrap_or_else(|_| {
        Landed::Fault(Fault::Runner(
            "the instance stopped before it answered".into(),
        ))
    })
}

/// One `view_perf` line as an installed instance leaves: a swap drops the
/// old view, a retry and a roster removal empty its slot.
impl Drop for View {
    fn drop(&mut self) {
        if self.installed_generation.is_some() {
            crate::perf::retire(self.program, self.instance);
        }
    }
}

/// `code` (WAT or wasm) prepared on the installed runner, at the default
/// view memory: what a test instantiates.
#[cfg(any(test, feature = "test-support"))]
pub fn prepared_for_test(code: impl AsRef<[u8]>) -> [u8; 32] {
    prepared_on_for_test(crate::contract::runner(), code)
}

/// [`prepared_for_test`] on `runner`.
#[cfg(any(test, feature = "test-support"))]
pub fn prepared_on_for_test(
    runner: &Arc<dyn crate::contract::ViewRunner>,
    code: impl AsRef<[u8]>,
) -> [u8; 32] {
    use sha2::Digest as _;
    // a test seats its code by hash, later: held for the test process's life
    static HELD: Mutex<Vec<Arc<dyn std::any::Any + Send + Sync>>> = Mutex::new(Vec::new());
    let code = code.as_ref();
    let hash = sha2::Sha256::digest(code).into();
    // the tests write their views as text; a runner takes wasm
    let code = wat::parse_bytes(code).expect("the test's view parses");
    let view_bytes =
        usize::try_from(crate::backend::memory_bounds().view_bytes).unwrap_or(usize::MAX);
    let prepared = runner
        .prepare(hash, &code, view_bytes)
        .expect("the test's code compiles");
    HELD.lock().unwrap().push(prepared.hold);
    hash
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

/// What the host is willing to take from one tick's bytes: nothing in here
/// is trusted — the counts, the tree.
/// The frame's style entries join `styles`, the table of the tree the host
/// holds (a whole tree's replace it); a refused frame leaves it as it was.
pub(super) fn shape(
    bytes: &[u8],
    styles: &mut wire::Styles,
) -> Result<(wire::Frame, wire::SanitizeReport), String> {
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

/// A view built against another wire than this app's, as the words its
/// failures carry; `None` for one built against this app's.
pub(super) fn foreign_wire(id: &str) -> Option<String> {
    (id != wire::WIRE_ID).then(|| {
        format!(
            "this view was built against wire {id}; this app speaks {}",
            wire::WIRE_ID
        )
    })
}

#[cfg(test)]
mod tests;
