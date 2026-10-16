//! One pane's view, stepped off the draw path: every wake (a reply, a
//! clock item, a `ViewTree` event, moved props, the theme, a load landing,
//! a busy frame) ends in one `turn`, never in a draw. What the pane draws
//! is read off the seat: its `ViewTree`, or the `Standin` to show instead.
//!
//! One tick per draw: a turn whose tick gave the tree something to draw (a
//! new tree, or a moved editor document) dirties the tree and holds the
//! seat until it has drawn (`render::Drawn`); a turn asked for meanwhile
//! runs right after the draw. So every frame the view makes is drawn
//! before it makes the next, as when it ticked on the draw path: the first
//! draw of a fresh view shows its first frame (the keys a pane hands its
//! first control go by it), and a reply's chain of ticks advances one
//! frame per draw. A tick that answered `unchanged` and moved no document
//! draws nothing and holds nothing: the tree the pane shows is the tree the
//! view would send.
//!
//! One tick in flight: a tick completes when its frame lands on the
//! instance's `Landing` (`contract::ViewInstance`), never by a timer. The
//! wasmtime runner lands it before `begin_tick` returns, and the turn takes
//! it right there (`View::landed_now`): the desktop's order. A runner that
//! answers later leaves the tick in flight; the turn ends, a task of the
//! seat's waits for the landing and finishes the turn (`landed`), and a
//! turn asked for meanwhile runs after it, as one asked for during a hold
//! runs after the draw.
use super::standin::{Standin, stage_words};
use super::*;
use gpui_kit::{AnyWindowHandle, App, Context, Entity, EventEmitter, Subscription};

/// Frame pacing for a busy view (`frame.busy`): one more turn, a frame later.
const BUSY_FRAME: Duration = Duration::from_millis(16);

/// The app is in the background (`Signal::Lifecycle`, SPEC §4.3): no seat
/// turns, so no call into a view is begun; replies and clock items wait in
/// the views, and every seat turns once the app is in front again. A
/// global of the app's, set by `start`'s drain; the desktop never sends
/// the phase.
pub struct Backgrounded(pub bool);

impl gpui_kit::Global for Backgrounded {}

/// Whether the app is in the background (`Backgrounded`).
fn backgrounded(cx: &App) -> bool {
    cx.try_global::<Backgrounded>().is_some_and(|phase| phase.0)
}

/// A seat a load parked a start on (`Mounted::to_start`), and its program.
type ToStart = (Arc<Mutex<Mounted>>, &'static str);

/// The window thread's end of the parked starts, installed by `start`
/// ([`serve_starts`]).
static STARTS: OnceLock<async_channel::Sender<ToStart>> = OnceLock::new();

/// Runs every start a load parks (`seat::start_in`) on this, the window
/// thread, whether or not a pane holds the seat: a seat `Preload::Seat`
/// preloads has no `Seat` to turn it. A pane's seat usually starts its own
/// first (`start_here`), and this finds nothing parked. Nothing starts in
/// the background; the pane's turn in front again does. Installed once per
/// process, by `start`.
pub fn serve_starts(cx: &mut App) {
    let (send, starts) = async_channel::unbounded();
    if STARTS.set(send).is_err() {
        return;
    }
    cx.spawn(async move |cx: &mut gpui_kit::AsyncApp| {
        while let Ok((mounted, program)) = starts.recv().await {
            if !cx.update(|cx| backgrounded(cx)) {
                cx.spawn(async move |_: &mut gpui_kit::AsyncApp| start_in(mounted, program).await)
                    .detach();
            }
        }
    })
    .detach();
}

/// Asks the window thread to start the view a load just parked on
/// `mounted` ([`serve_starts`]); a test with no host started has its
/// pane's turn start it, or starts it by hand.
pub(in crate::runtime) fn begin_start(mounted: Arc<Mutex<Mounted>>, program: &'static str) {
    if let Some(starts) = STARTS.get() {
        let _ = starts.try_send((mounted, program));
    }
}

/// The start parked on `mounted`, run here on the window thread: polled at
/// once, so on the wasmtime runner the view is up before this returns (a
/// pane opening a program it compiled shows no stand-in between), and
/// finished by a task when its answers land later. Whether it finished.
fn start_here(mounted: &Arc<Mutex<Mounted>>, program: &'static str, cx: &mut App) -> bool {
    use futures::FutureExt as _;
    if lock(mounted).to_start.is_none() {
        return false;
    }
    let mut started = Box::pin(start_in(mounted.clone(), program));
    if (&mut started).now_or_never().is_some() {
        return true;
    }
    cx.spawn(async move |_: &mut gpui_kit::AsyncApp| started.await)
        .detach();
    false
}

pub struct Seat {
    program: &'static str,
    instance: u64,
    mounted: Arc<Mutex<Mounted>>,
    /// The window the pane is placed in now; set by `Seats::reconcile`
    /// through `place`. Widget commands and the carried presentation need it.
    window: Option<AnyWindowHandle>,
    /// The body the pane lays the view out in, as the layout has it before
    /// the draw (`place`): the view hears it ahead of every tick, so its
    /// first frame is laid out to its pane. None until the pane's desk is
    /// drawn, and the view is not ticked until then (`turn`).
    body: Option<(f32, f32)>,
    /// Whether the pane's window is the one in front (`place`): the view
    /// is on screen (`host.visible`) only then. A pane in a window behind
    /// another, or in an app the person left, is not being looked at.
    in_front: bool,
    tree: Option<Entity<crate::render::ViewTree>>,
    _tree_events: Option<Subscription>,
    _tree_drawn: Option<Subscription>,
    /// A tick the tree has not drawn yet: turns wait for that draw.
    holding: bool,
    /// A turn asked for while holding or while a tick is in flight; it runs
    /// once the tree has drawn, or the frame has landed.
    owed: bool,
    /// A tick begun whose frame has not landed: the view's tick count and
    /// the instant its turn began, which the turn's second half reads.
    in_flight: Option<(u64, Instant)>,
    /// The wait on that landing.
    _landing: Option<gpui_kit::Task<()>>,
    /// Gone from every desk (`hide`): no turn after, and a frame landing
    /// after is kept on the view, never drawn.
    hidden: bool,
    generation: u64,
    alive: Option<Arc<()>>,
    revision: u64,
    props: Option<Vec<u8>>,
    /// Whether a view here may move the keys: its pane in front, nothing
    /// open over the desk, no keyboard hold (`PaneLayer` says; `false`
    /// until it has). The web's rule: a background document takes no focus.
    /// Every focus-moving widget command is judged by it as it runs, and
    /// the tree mirrors it for a dialog's auto-focus (`keys_grant`).
    keys_free: bool,
    /// `MIN_WINDOW_WIDTH` of the seated view; the pane lays out from it.
    min_width: f32,
    standin: Option<Standin>,
    _replies: Option<gpui_kit::Task<()>>,
    /// The seat's load wake (`Mounted.wake`): install, a stage shown, a retry.
    _wake: gpui_kit::Task<()>,
    clock: Option<(Instant, gpui_kit::Task<()>)>,
    busy: Option<gpui_kit::Task<()>>,
    /// Theme; Session and Account land as props through `Seats`.
    _observers: Vec<Subscription>,
    #[cfg(any(test, feature = "test-support"))]
    pub(super) turns: u64,
}

impl EventEmitter<Intent> for Seat {}

/// The view's root, sized to the seat: a wire root that names no size
/// would otherwise take its content's, and a Fill-sized pane inside it
/// (a room, a document) collapses to zero height.
///
/// It carries no id: an id here would be one segment on every descendant's
/// authored path that the view never sent and the AX door would read. An
/// id-less `Container` gets GPUI's own synthetic element id
/// (`ViewTree::container`) instead, so identity is unaffected.
///
/// Its style is the host's own, no entry of the view's table
/// (`wire::Styles::host`).
pub(super) fn native_root(tree: crate::render::Tree) -> crate::render::Tree {
    use gpui_kit::Styled as _;
    static FILL: std::sync::LazyLock<Arc<gpui_kit::StyleRefinement>> =
        std::sync::LazyLock::new(|| Arc::new(gpui_kit::div().size_full().style().clone()));
    let crate::render::Tree { root, mut styles } = tree;
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: styles.host(FILL.clone()),
        interactivity: Default::default(),
        children: vec![root],
    });
    crate::render::Tree { root, styles }
}

impl Seat {
    /// The seat of the pane with this layout `instance`, which is its key
    /// in the registry: a link's route waits for it by that key.
    pub fn new(program: &'static str, instance: u64, cx: &mut Context<Self>) -> Self {
        let theme = cx.observe_global::<gpui_kit::component::Theme>(|this, cx| this.turn(cx));
        let phase = cx.observe_global::<Backgrounded>(|this, cx| this.turn(cx));
        let mounted = mounted(program, instance);
        // a seat claimed with its code compiled starts here and now
        start_here(&mounted, program, cx);
        let mut woken = lock(&mounted).wake.subscribe();
        let wake = cx.spawn(async move |this, cx| {
            while woken.changed().await.is_ok() {
                if this.update(cx, |this, cx| this.turn(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            program,
            instance,
            mounted,
            window: None,
            body: None,
            in_front: false,
            tree: None,
            _tree_events: None,
            _tree_drawn: None,
            holding: false,
            owed: false,
            in_flight: None,
            _landing: None,
            hidden: false,
            generation: 0,
            alive: None,
            revision: 0,
            props: None,
            keys_free: false,
            min_width: 0.,
            standin: None,
            _replies: None,
            _wake: wake,
            clock: None,
            busy: None,
            _observers: vec![theme, phase],
            #[cfg(any(test, feature = "test-support"))]
            turns: 0,
        }
    }

    pub fn program(&self) -> &'static str {
        self.program
    }

    /// Its pane's layout instance (its perf key).
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// The id around the view's tree, `view/<program>`: the AX test door
    /// (`ax::tree`) reads the program off it. A host id, so no id a view
    /// sends is one.
    pub fn ax_mark(&self) -> gpui_kit::ElementId {
        crate::render::host_id(format!("{}{}", crate::render::VIEW_MARK, self.program))
    }

    pub fn tree(&self) -> Option<Entity<crate::render::ViewTree>> {
        self.tree.clone()
    }

    pub fn min_width(&self) -> f32 {
        self.min_width
    }

    pub fn standin(&self) -> Option<&Standin> {
        self.standin.as_ref()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn window(&self) -> Option<AnyWindowHandle> {
        self.window
    }

    /// A turn from a window callback (`on_next_frame`, `observe_in`, a
    /// `window.update` closure): the window is borrowed there, so a nested
    /// `window.update` in `turn`/`mount` would fail ("window not found").
    /// Deferred, the turn runs after that update returns, at app level.
    pub fn wake(&self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| this.turn(cx));
        });
    }

    /// `Seats::reconcile` places the pane: the window its commands run in,
    /// the body its view is laid out in (none until the desk is drawn), and
    /// whether that window is in front.
    pub fn place(
        &mut self,
        window: AnyWindowHandle,
        body: Option<(f32, f32)>,
        in_front: bool,
        cx: &mut Context<Self>,
    ) {
        if self.window != Some(window) || self.body != body || self.in_front != in_front {
            self.window = Some(window);
            self.body = body;
            self.in_front = in_front;
            self.wake(cx);
        }
    }

    /// `PaneLayer` says whether a view here may move the keys (see
    /// `keys_free`); the tree hears it too. Nothing draws from it, so no
    /// notify.
    pub fn set_keys_free(&mut self, keys_free: bool, cx: &mut Context<Self>) {
        self.keys_free = keys_free;
        if let Some(tree) = &self.tree {
            tree.update(cx, |tree, _| tree.set_keys_grant(keys_free));
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn keys_free(&self) -> bool {
        self.keys_free
    }

    /// Whether it was placed in front (`place`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_front(&self) -> bool {
        self.in_front
    }

    /// The body its view is laid out in (`place`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn body(&self) -> Option<(f32, f32)> {
        self.body
    }

    /// Session, Account and the theme land here as encoded props: a turn
    /// only when the bytes moved. Called from `Seats`' observers and
    /// `Seats::reconcile`.
    pub fn set_props(&mut self, props: Vec<u8>, cx: &mut Context<Self>) {
        if self.props.as_ref() == Some(&props) {
            return;
        }
        self.props = Some(props);
        self.wake(cx);
    }

    /// The person asked to go back (the phone's gesture or its bar) while
    /// this seat's view is in front. The view hears it, as `Event::Back`,
    /// when the root of the last frame that landed has an `on_back` (a page
    /// under its root, an open sheet); otherwise it has nowhere to go back
    /// to, and the shell leaves the program. With a tick in flight that
    /// frame may be one tick old: a view that has just popped to its root
    /// hears a Back it has no use for and ignores it, and its next frame
    /// has no `on_back`.
    pub fn back(&mut self, cx: &mut Context<Self>) -> Back {
        let mounted = self.mounted.clone();
        let mut locked = lock(&mounted);
        let Slot::Ready(view) = &mut locked.slot else {
            return Back::Leave;
        };
        let on_back = view
            .frame
            .root
            .as_ref()
            .and_then(wire::Node::interactivity)
            .and_then(|interactivity| interactivity.on_back);
        let heard = view.fault.is_none() && self.generation == view.seated_generation();
        let Some(handler) = on_back.filter(|_| heard) else {
            return Back::Leave;
        };
        view.pending.push(wire::Event::Back { handler });
        drop(locked);
        self.turn(cx);
        Back::Heard
    }

    /// The pane left every desk: one bounded update, then no more turns.
    /// Returns the intents that update produced: the caller (`Seats`) is
    /// about to drop this seat and its subscription in the same update, so
    /// an emit here would be lost. In the background there is no update:
    /// no call into a view is begun there (SPEC §4.3).
    #[must_use]
    pub fn hide(&mut self, cx: &App) -> Vec<Intent> {
        let mounted = self.mounted.clone();
        let mut locked = lock(&mounted);
        let Mounted {
            slot, props, owned, ..
        } = &mut *locked;
        let Slot::Ready(view) = slot else {
            return Vec::new();
        };
        let owns = self.generation == view.seated_generation()
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &view.alive));
        self.hidden = true;
        // a view never stepped here has nothing to hand over; one that was
        // gets its last update whether or not its window was in front
        if !owns || view.ticks == 0 {
            return Vec::new();
        }
        view.set_visible(false);
        // a tick in flight lands on the view as it is, kept, never drawn
        // (`landed`); this update's own waits behind it as any turn would,
        // nor runs while a swap owns the view: its replacement hears it
        // hidden. `Seats` drops this seat after, and the view goes with it
        if !view.ticking() && !*owned && !backgrounded(cx) {
            match view.begin_redraw(props) {
                Redraw::Quiet => {}
                Redraw::Ready => {
                    view.finish_redraw(props);
                }
                Redraw::Ticking => match view.landed_now() {
                    // a death where a replay is kept is no fault of the
                    // view's (SPEC §6.5), and this update begins no call
                    // to take it up again: it stays on the view
                    Some(landed @ Landed::Fault(Fault::Runner(_))) if view.kept.is_some() => {
                        view.parked = Some(landed);
                    }
                    Some(landed) => {
                        view.redraw_landed(landed);
                        view.finish_redraw(props);
                    }
                    None => {}
                },
            }
        }
        std::mem::take(&mut view.intents)
    }

    /// Whether a tick is in flight (begun, its frame not landed) or a swap
    /// runs (`seat::swap_in`): the AX door parks a read until neither.
    pub fn in_flight(&self) -> bool {
        self.in_flight.is_some() || lock(&self.mounted).owned
    }

    /// Every wake ends here: a reply, a clock item, a `ViewTree` event,
    /// moved props, the theme, a load landing, a busy frame, the AX door
    /// before a read (`Seats::settle`). Never a draw, never a window
    /// callback (those go through `wake`). While a tick that changed what
    /// the tree draws waits for its draw, the turn waits too (`Drawn` runs
    /// it): one tick per draw, as when the view ticked on the draw path.
    pub fn turn(&mut self, cx: &mut Context<Self>) {
        if self.hidden {
            return;
        }
        // a swap that runs owns the drawn view until it is done; it wakes
        // the seat then
        if self.holding || self.in_flight.is_some() || lock(&self.mounted).owned || backgrounded(cx)
        {
            self.owed = true;
            return;
        }
        self.owed = false;
        #[cfg(any(test, feature = "test-support"))]
        {
            self.turns += 1;
        }
        let mounted = self.mounted.clone();
        let mut locked = lock(&mounted);
        let shown = Instant::now();
        let program = self.program;
        let Mounted {
            slot,
            props,
            swap,
            to_start,
            ..
        } = &mut *locked;
        let view = match slot {
            Slot::Ready(view) => view,
            Slot::Empty => {
                drop(locked);
                let words = format!(
                    "This network has no {program} view. An admin can activate a deployment that ships one."
                );
                return self.show_standin(words.into(), cx);
            }
            Slot::Failed(failure) => {
                let standin = Standin::from(&*failure);
                drop(locked);
                return self.show_standin(standin, cx);
            }
            // compiled, a load parked its start: seated and inited here,
            // then drawn
            Slot::Compiled(_) if to_start.is_some() => {
                drop(locked);
                if start_here(&mounted, program, cx) {
                    return self.turn(cx);
                }
                return self.show_standin(
                    Standin {
                        title: None,
                        words: stage_words(&Slot::Loading),
                        loading: true,
                        retry: false,
                    },
                    cx,
                );
            }
            // on its way, or compiled and being started for this pane
            loading => {
                let standin = Standin {
                    title: None,
                    words: stage_words(loading),
                    loading: true,
                    retry: false,
                };
                drop(locked);
                return self.show_standin(standin, cx);
            }
        };
        // The view lays itself out in its pane's body, and hears it ahead
        // of every tick. A pane with none yet (its desk is not drawn) does
        // not tick: `place` turns the seat when the body comes.
        // a tick another seat began (one let go of, a `hide`) lands here
        // first, as this seat's own would
        if view.ticking() {
            let ticks = view.ticks;
            let parked = view.parked.take();
            return self.wait(view, ticks, shown, parked, cx);
        }
        // a page that died outside a tick (a kept snapshot's, a swap's) is
        // taken up again before anything else begins a call: in a task the
        // seat does not own, which wakes the seat when done
        if view.parked.is_some() {
            drop(locked);
            cx.spawn(async move |_, cx| {
                // backgrounded before the task ran: the turn in front again
                // spawns it again
                if !cx.update(|cx| backgrounded(cx)) {
                    take_up(mounted).await;
                }
            })
            .detach();
            return;
        }
        // a replacement the loader prepared runs on this thread once the
        // view owes nothing (its handover would refuse): a task the seat
        // does not own, so letting the seat go mid-swap stops nothing
        if swap.is_some() && view.settled() {
            drop(locked);
            cx.spawn(async move |_, cx| {
                // backgrounded before the task ran: the turn in front again
                // spawns it again
                if !cx.update(|cx| backgrounded(cx)) {
                    swap_in(mounted, program).await;
                }
            })
            .detach();
            return;
        }
        let Some((width, height)) = self.body else {
            return;
        };
        view.instance = self.instance;
        // the press or key the host last received for this tree is the
        // view's activation for the requests this redraw answers
        if let Some(tree) = &self.tree
            && let Some(at) = tree.read(cx).take_activation()
        {
            view.activation = Some(at);
        }
        *props = self.props.clone();
        let ticks = view.ticks;
        view.set_visible(self.in_front);
        view.sync_theme(gpui_kit::component::Theme::global(cx).is_dark());
        let touch = crate::contract::os().facts().touch_first;
        view.sync_pointer(touch);
        // A manifest's minimum is the narrowest pointer window the view
        // takes: under a pointer the view is never laid out narrower
        // (`layers/panes.rs` gives its body `min_w` and scrolls it); a
        // phone lays every view out at its screen's width, whatever
        // minimum it declares: it never refuses, scrolls or zooms a view
        // for it
        let width = if touch {
            width
        } else {
            width.max(view.min_width as f32)
        };
        view.sync_viewport(width, height);
        let again = match view.begin_redraw(props) {
            Redraw::Quiet => false,
            Redraw::Ready => view.finish_redraw(props),
            Redraw::Ticking => match view.landed_now() {
                // dead already: taken up again as a later death is
                Some(landed @ Landed::Fault(Fault::Runner(_))) if view.kept.is_some() => {
                    return self.wait(view, ticks, shown, Some(landed), cx);
                }
                Some(landed) => {
                    view.redraw_landed(landed);
                    view.finish_redraw(props)
                }
                // the frame lands later: the turn's second half waits for it
                None => return self.wait(view, ticks, shown, None, cx),
            },
        };
        self.finish(locked, ticks, shown, again, cx);
        self.after_turn(cx);
    }

    /// After a turn: a swap parked while the view owed work runs once it
    /// owes none (the turn that settled the view asks for one more, which
    /// starts it); else a kept replay due a new snapshot takes it
    /// (`keep_in`), in a task the seat does not own.
    fn after_turn(&self, cx: &mut Context<Self>) {
        let (swap, keep) = {
            let locked = lock(&self.mounted);
            match &locked.slot {
                Slot::Ready(view) if !locked.owned => (
                    locked.swap.is_some() && view.settled() && !view.ticking(),
                    locked.swap.is_none() && view.keep_due(),
                ),
                _ => (false, false),
            }
        };
        if swap {
            self.wake(cx);
        } else if keep {
            let mounted = self.mounted.clone();
            cx.spawn(async move |_, cx| {
                // no call is begun in the background (a frame that landed
                // there finished its turn here): the turn in front again
                // comes back
                if !cx.update(|cx| backgrounded(cx)) {
                    keep_in(mounted).await;
                }
            })
            .detach();
        }
    }

    /// The turn's second half waits for `view`'s tick in flight: a task of
    /// the seat's takes the frame when it lands (`landed`).
    fn wait(
        &mut self,
        view: &View,
        ticks: u64,
        shown: Instant,
        first: Option<Landed>,
        cx: &mut Context<Self>,
    ) {
        let mut landing = view.landing();
        let alive = view.alive.clone();
        let mounted = self.mounted.clone();
        self.in_flight = Some((ticks, shown));
        self._landing = Some(cx.spawn(async move |this, cx| {
            let mut first = first;
            let landed = loop {
                let landed = match first.take() {
                    Some(landed) => landed,
                    None => super::super::view::next(&landing).await,
                };
                // the instance died with the tick in flight: where a replay
                // is kept it is taken up again on a new one, whose frame
                // this waits for instead (SPEC §6.5). In the background,
                // where no call is begun, the death waits on the view for
                // the turn in front again
                if matches!(landed, Landed::Fault(Fault::Runner(_))) {
                    if cx.update(|cx| backgrounded(cx)) {
                        let mut locked = lock(&mounted);
                        if let Slot::Ready(view) = &mut locked.slot
                            && Arc::ptr_eq(&view.alive, &alive)
                            && view.kept.is_some()
                        {
                            view.parked = Some(landed);
                            break None;
                        }
                    } else if let Ok(revived) = View::revive(&mounted, &alive, true).await {
                        landing = revived;
                        continue;
                    }
                }
                break Some(landed);
            };
            let _ = this.update(cx, |this, cx| this.landed(&alive, landed, cx));
        }));
    }

    /// A frame that landed after its turn returned: taken in by the view it
    /// was asked of, and the turn finished. After `hide` it is kept on the
    /// view, its requests answered, and nothing is drawn. A turn owed
    /// meanwhile runs now, unless the tree holds for its draw. `None`: a
    /// death parked on the view in the background, which the turn in front
    /// again takes up (`View::parked`).
    fn landed(&mut self, alive: &Arc<()>, landed: Option<Landed>, cx: &mut Context<Self>) {
        let Some((ticks, shown)) = self.in_flight.take() else {
            return;
        };
        self._landing = None;
        let Some(landed) = landed else {
            self.owed = true;
            return;
        };
        let mounted = self.mounted.clone();
        let mut locked = lock(&mounted);
        let Mounted { slot, props, .. } = &mut *locked;
        match slot {
            // the instance it was asked of is still the seat's
            Slot::Ready(view) if Arc::ptr_eq(alive, &view.alive) => {
                view.redraw_landed(landed);
                let again = view.finish_redraw(props);
                if self.hidden {
                    return;
                }
                self.finish(locked, ticks, shown, again, cx);
                self.after_turn(cx);
            }
            // replaced or stopped meanwhile: the answer went with it
            _ => drop(locked),
        }
        if self.owed && !self.holding {
            self.turn(cx);
        }
    }

    /// A turn's second half, its tick (if any) landed: what the view asked
    /// for goes to the tree, the window and the app.
    fn finish(
        &mut self,
        mut locked: std::sync::MutexGuard<'_, Mounted>,
        ticks: u64,
        shown: Instant,
        again: bool,
        cx: &mut Context<Self>,
    ) {
        let Slot::Ready(view) = &mut locked.slot else {
            return;
        };
        clipboard::mount(view, cx);
        // a clipboard answer lands after `redraw` judged the frame: it is
        // still a reason to turn again
        let again = again || !view.pending.is_empty();
        let next_tick = kernel::next_tick(&view.clocks);
        if let Some(fault) = &view.fault {
            let standin = Standin::from(&view.failure(fault.clone()));
            drop(locked);
            return self.show_standin(standin, cx);
        }
        let generation = view.seated_generation();
        let key = view.perf_key();
        let same = self.generation == generation
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &view.alive));
        let fresh = (!same || self.revision != view.frame_rev).then(|| {
            let (tree, pictures) = view.drawn();
            (native_root(tree), pictures, view.frame_rev)
        });
        let adopt = (!same).then(|| {
            (
                view.alive.clone(),
                view.replies.changes(),
                view.replies.answer_owed(),
            )
        });
        let commands = view.widget_commands.len();
        // the field edits still queued, which run frames from now against
        // the text they read: the tree keeps the edits since for them
        let asked: Vec<(wire::WidgetTarget, u64)> = view
            .widget_commands
            .iter()
            .filter_map(|(_, command)| match command {
                wire::WidgetCommand::Replace {
                    target, revision, ..
                } => Some((target.clone(), *revision)),
                _ => None,
            })
            .collect();
        let intents = std::mem::take(&mut view.intents);
        let tooltips = std::mem::take(&mut view.tooltip_responses);
        let min_width = view.min_width as f32;
        let body = view.viewport_sent;
        let first_tree = ticks == 0 && view.ticks == 1;
        drop(locked);

        // a pane draws a placed seat's tree; nothing draws an unplaced one,
        // and nothing redraws for a tick that gave the tree nothing new
        self.holding = fresh.is_some() && self.window.is_some();
        if self.standin.take().is_some() {
            cx.notify();
        }
        if self.min_width != min_width {
            self.min_width = min_width;
            cx.notify();
        }
        if let Some((root, pictures, rev)) = fresh {
            let _replacing = crate::perf::time(key, "replace");
            self.revision = rev;
            match (&self.tree, adopt) {
                // the tree's own notify dirties its window; nothing the
                // pane reads off the seat moved
                (Some(tree), None) => tree.update(cx, |tree, cx| {
                    tree.set_pictures(pictures);
                    tree.replace(root, &asked, cx)
                }),
                (_, adopt) => {
                    self.mount(root, pictures, generation, key, adopt, cx);
                    cx.notify();
                }
            }
            if first_tree && crate::perf::on() {
                // a fresh view's first tree: from this turn's start, the
                // first to find it seated, to its tree mounted
                crate::perf::record(key, "first_tree", shown.elapsed().as_micros() as u64);
            }
        }
        if let Some(tree) = &self.tree {
            // the body the view was told: a sheet docks by its class
            tree.update(cx, |tree, cx| tree.set_body(body, cx));
        }
        if !tooltips.is_empty()
            && let Some(tree) = &self.tree
        {
            tree.update(cx, |tree, cx| tree.tooltip_responses(tooltips, cx));
        }
        if commands > 0 {
            self.run_widget_commands(generation, cx);
        }
        for intent in intents {
            cx.emit(intent);
        }
        self.arm_clock(next_tick, cx);
        // frame-paced, never back to back: the view ran out of budget and
        // wants the next tick soon, not now
        if again && self.busy.is_none() {
            self.busy = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(BUSY_FRAME).await;
                let _ = this.update(cx, |this, cx| {
                    this.busy = None;
                    this.turn(cx);
                });
            }));
        }
    }

    fn show_standin(&mut self, standin: Standin, cx: &mut Context<Self>) {
        // the standin goes over the tree, whose draw would release the hold
        self.holding = false;
        if self.standin.as_ref() != Some(&standin) {
            self.standin = Some(standin);
            cx.notify();
        }
    }

    /// A first tree, or a new deployment's: a fresh `ViewTree` carrying the
    /// old presentation, its editor store, a wake on replies and the route
    /// of its events back into the view.
    fn mount(
        &mut self,
        root: crate::render::Tree,
        pictures: Arc<crate::render::PictureBytes>,
        generation: u64,
        key: crate::perf::Key,
        adopt: Option<(Arc<()>, tokio::sync::watch::Receiver<()>, bool)>,
        cx: &mut Context<Self>,
    ) {
        let Some((alive, mut changes, owed)) = adopt else {
            return;
        };
        // a first view here, or a new deployment's: its minimum may be
        // new, so the desk fits its windows to it
        cx.emit(Intent::Seated);
        if crate::perf::on() {
            crate::perf::mark(intern(&format!("first_seated.{}", self.program)));
        }
        self.generation = generation;
        self.alive = Some(alive.clone());
        self._replies = Some(cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                if this.update(cx, |this, cx| this.turn(cx)).is_err() {
                    break;
                }
            }
        }));
        // the presentation carried over needs the window the old tree drew
        // in; `turn` is app-level (see `wake`), so this update succeeds
        let presentation = match (&self.tree, self.window) {
            (Some(tree), Some(window)) => window
                .update(cx, |_, window, cx| tree.read(cx).presentation(window, cx))
                .unwrap_or_default(),
            _ => Default::default(),
        };
        let tree = cx.new(|_| {
            crate::render::ViewTree::new(root)
                .with_presentation(presentation)
                .with_perf_key(key)
                .with_keys_grant(self.keys_free)
        });
        tree.update(cx, |tree, _| tree.set_pictures(pictures));
        let seat = self.mounted.clone();
        self._tree_events = Some(
            cx.subscribe(&tree, move |this, _, event: &wire::Event, cx| {
                let mut locked = lock(&seat);
                let Slot::Ready(view) = &mut locked.slot else {
                    return;
                };
                let current =
                    view.seated_generation() == generation && Arc::ptr_eq(&alive, &view.alive);
                if current {
                    view.pending.push(event.clone());
                }
                drop(locked);
                this.turn(cx);
            }),
        );
        self._tree_drawn = Some(
            cx.subscribe(&tree, |this, _, _: &crate::render::Drawn, cx| {
                this.holding = false;
                if this.owed {
                    this.turn(cx);
                }
            }),
        );
        self.tree = Some(tree);
        if owed {
            // an answer that landed before the subscription still needs delivery
            self.wake(cx);
        }
    }

    /// A timer for the view's next `clock.ticks`, re-armed when it moves.
    fn arm_clock(&mut self, next: Option<Instant>, cx: &mut Context<Self>) {
        if self.clock.as_ref().map(|(due, _)| *due) == next {
            return;
        }
        self.clock = next.map(|due| {
            let timer = cx
                .background_executor()
                .timer(due.saturating_duration_since(Instant::now()));
            let task = cx.spawn(async move |this, cx| {
                timer.await;
                let _ = this.update(cx, |this, cx| this.turn(cx));
            });
            (due, task)
        });
    }

    /// The view's runnable `host.widget` commands, run in the pane's window
    /// one frame after the tree they target has drawn (the #110 rule: an
    /// input or menu the new tree mounts is created in that draw, so a
    /// `Focus` before it has no handle). A next-frame callback runs before
    /// the draw, hence the callback inside the callback.
    fn run_widget_commands(&self, generation: u64, cx: &mut Context<Self>) {
        let Some(window) = self.window else {
            return;
        };
        let this = cx.weak_entity();
        let seat = self.mounted.clone();
        let _ = window.update(cx, |_, window, _| {
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        let mut locked = lock(&seat);
                        let Slot::Ready(view) = &mut locked.slot else {
                            return;
                        };
                        // the VIEW has to be the one that asked: a
                        // replacement or a reseat takes its queue with it.
                        // The FRAME does not: each command is judged
                        // against the tree standing now.
                        let same = view.seated_generation() == generation
                            && this
                                .alive
                                .as_ref()
                                .is_some_and(|alive| Arc::ptr_eq(alive, &view.alive));
                        if !same {
                            return;
                        }
                        let Some(tree) = &this.tree else {
                            return;
                        };
                        // judged as it runs, not as it was asked: a command
                        // the view held back answers to the desk as it is now
                        view.execute_widget_commands(|command| {
                            if View::moves_keys(&command) && !this.keys_free {
                                return Err("view is not focused".into());
                            }
                            tree.update(cx, |tree, cx| {
                                tree.execute_widget_command(command, window, cx)
                            })
                        });
                        drop(locked);
                        // the window is borrowed here: the turn that carries
                        // the answers back is deferred, never nested
                        this.wake(cx);
                    });
                });
            });
        });
    }
}

impl Drop for Seat {
    fn drop(&mut self) {
        // back as the program's preloaded seat, its code kept (`let_go`)
        let_go(self.program, self.instance);
        // a route its view never read goes with it: no seat takes its key again
        _ = take_route(self.program, self.instance);
        // the program's row is read off the seat put back, or another
        rail_moved();
    }
}
