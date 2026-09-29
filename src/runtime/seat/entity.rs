//! One pane's guest, stepped off the draw path: every wake (a reply, a
//! clock item, a `ViewTree` event, moved props, the theme, a load landing,
//! a busy frame) ends in one `turn`, never in a draw. What the pane draws
//! is read off the seat: its `ViewTree`, or the `Standin` to show instead.
use super::standin::{Standin, stage_words};
use super::*;
use gpui_kit::{AnyWindowHandle, Context, Entity, EventEmitter, Subscription};

/// Frame pacing for a busy guest (`frame.busy`): one more turn, a frame later.
const BUSY_FRAME: Duration = Duration::from_millis(16);

pub(crate) struct Seat {
    module: &'static str,
    instance: u64,
    mounted: Arc<Mutex<Mounted>>,
    /// The window the pane is placed in now; set by `Seats::reconcile`
    /// through `place`. Widget commands and the carried presentation need it.
    window: Option<AnyWindowHandle>,
    tree: Option<Entity<crate::render::ViewTree>>,
    _tree_events: Option<Subscription>,
    generation: u64,
    alive: Option<Arc<()>>,
    revision: u64,
    props: Option<Vec<u8>>,
    focused: bool,
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
    #[cfg(test)]
    pub(super) turns: u64,
}

impl EventEmitter<Intent> for Seat {}

/// The guest's root, sized to the seat: a wire root that names no size
/// would otherwise take its content's, and a Fill-sized pane inside it
/// (a room, a document) collapses to zero height.
///
/// UNNAMED ON PURPOSE: `EditorStore` keys every editor by the `AuthoredPath`
/// it walks from the guest's OWN root (`guest.frame.root`, never wrapped —
/// see `guest/requests.rs`'s `tick`). Giving this wrapper an id would push
/// it onto every descendant's path here and nowhere else, so no editor's
/// native field could ever find its `EditorStore` entry: it would type
/// locally (GPUI's own default text handling) while the guest never saw an
/// edit and every gate on the guest's document (Send, key claims like
/// Enter) stayed stuck. An id-less `Container` gets GPUI's own synthetic
/// element id (`ViewTree::container`) instead, so identity is unaffected.
pub(super) fn native_root(root: wire::Node) -> wire::Node {
    use gpui_kit::Styled as _;
    let mut host_root = gpui_kit::div().size_full();
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: host_root.style().clone(),
        interactivity: Default::default(),
        children: vec![root],
    })
}

impl Seat {
    pub(crate) fn new(module: &'static str, cx: &mut Context<Self>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let instance = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let theme = cx.observe_global::<gpui_kit::component::Theme>(|this, cx| this.turn(cx));
        let mounted = mounted(module, instance);
        let mut woken = mounted.lock().expect("module view lock").wake.subscribe();
        let wake = cx.spawn(async move |this, cx| {
            while woken.changed().await.is_ok() {
                if this.update(cx, |this, cx| this.turn(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            module,
            instance,
            mounted,
            window: None,
            tree: None,
            _tree_events: None,
            generation: 0,
            alive: None,
            revision: 0,
            props: None,
            focused: true,
            min_width: 0.,
            standin: None,
            _replies: None,
            _wake: wake,
            clock: None,
            busy: None,
            _observers: vec![theme],
            #[cfg(test)]
            turns: 0,
        }
    }

    pub(crate) fn module(&self) -> &'static str {
        self.module
    }

    /// The seat's own instance number (its perf key), not the pane's.
    pub(crate) fn instance(&self) -> u64 {
        self.instance
    }

    /// The id around the view's tree: the AX test door (`ax::tree`) reads
    /// the module off it.
    pub(crate) fn ax_mark(&self) -> gpui_kit::ElementId {
        gpui_kit::ElementId::Name(format!("{}{}", crate::ax::VIEW_MARK, self.module).into())
    }

    pub(crate) fn tree(&self) -> Option<Entity<crate::render::ViewTree>> {
        self.tree.clone()
    }

    pub(crate) fn min_width(&self) -> f32 {
        self.min_width
    }

    pub(crate) fn standin(&self) -> Option<&Standin> {
        self.standin.as_ref()
    }

    #[cfg(test)]
    pub(super) fn props(&self) -> Option<&[u8]> {
        self.props.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> Option<AnyWindowHandle> {
        self.window
    }

    /// A turn from a window callback (`on_next_frame`, `observe_in`, a
    /// `window.update` closure): the window is borrowed there, so a nested
    /// `window.update` in `turn`/`mount` would fail ("window not found").
    /// Deferred, the turn runs after that update returns, at app level.
    pub(crate) fn wake(&self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| this.turn(cx));
        });
    }

    /// `Seats::reconcile` places the pane: the window its commands run in.
    pub(crate) fn place(&mut self, window: AnyWindowHandle, cx: &mut Context<Self>) {
        if self.window != Some(window) {
            self.window = Some(window);
            self.wake(cx);
        }
    }

    /// Whether the pane is in front: a `Focus` the guest asks for is
    /// refused while it is not. Nothing draws from it, so no notify.
    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Session, Account and the theme land here as encoded props: a turn
    /// only when the bytes moved. Deferred: until s10 this is called from
    /// `Desktop::dispatch`, which runs inside window event handlers.
    pub(crate) fn set_props(&mut self, props: Vec<u8>, cx: &mut Context<Self>) {
        if self.props.as_ref() == Some(&props) {
            return;
        }
        self.props = Some(props);
        self.wake(cx);
    }

    /// The pane left every desk: one bounded update, then no more turns.
    /// Returns the intents that update produced: the caller (`Seats`) is
    /// about to drop this seat and its subscription in the same update, so
    /// an emit here would be lost.
    #[must_use]
    pub(crate) fn hide(&mut self) -> Vec<Intent> {
        let mounted = self.mounted.clone();
        let mut locked = mounted.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *locked;
        let Slot::Ready(guest) = slot else {
            return Vec::new();
        };
        let owns = self.generation == guest.seated_generation()
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &guest.alive));
        if !owns || !guest.visible {
            return Vec::new();
        }
        guest.set_visible(false);
        guest.redraw(props);
        std::mem::take(&mut guest.intents)
    }

    /// Every wake ends here: a reply, a clock item, a `ViewTree` event,
    /// moved props, the theme, a load landing, a busy frame. Never a draw,
    /// never a window callback (those go through `wake`).
    pub(crate) fn turn(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.turns += 1;
        }
        let mounted = self.mounted.clone();
        let mut locked = mounted.lock().expect("module view lock");
        let shown = Instant::now();
        locked.shown = Some(shown);
        let module = self.module;
        let Mounted { slot, props, .. } = &mut *locked;
        let guest = match slot {
            Slot::Ready(guest) => guest,
            Slot::Empty => {
                drop(locked);
                let words = format!(
                    "This network has no {module} view. An admin can activate a deployment that ships one."
                );
                return self.show_standin(words.into(), cx);
            }
            Slot::Failed(failure) => {
                let standin = Standin::from(&*failure);
                drop(locked);
                return self.show_standin(standin, cx);
            }
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
        guest.instance = self.instance;
        *props = self.props.clone();
        let ticks = guest.ticks;
        guest.set_visible(true);
        guest.sync_theme(gpui_kit::component::Theme::global(cx).is_dark());
        let again = guest.redraw(props);
        clipboard::mount(guest, cx);
        // a clipboard answer lands after `redraw` judged the frame: it is
        // still a reason to turn again
        let again = again || !guest.pending.is_empty();
        let next_tick = kernel::next_tick(&guest.clocks);
        if let Some(fault) = &guest.fault {
            let standin = Standin::from(&Failure::Trapped(fault.clone()));
            drop(locked);
            return self.show_standin(standin, cx);
        }
        let generation = guest.seated_generation();
        let key = guest.perf_key();
        let same = self.generation == generation
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &guest.alive));
        let fresh = (!same || self.revision != guest.frame_rev).then(|| {
            let mut root = native_root(guest.frame.root.clone().unwrap_or_else(wire::Node::empty));
            guest.pictures.hydrate(&mut root);
            (root, guest.frame_rev)
        });
        let adopt = (!same).then(|| {
            (
                guest.alive.clone(),
                guest.replies.changes(),
                guest.replies.answer_owed(),
                guest.inputs.clone(),
            )
        });
        let commands = guest.runnable_widget_commands();
        let intents = std::mem::take(&mut guest.intents);
        let min_width = guest.min_width as f32;
        let ticked = ticks != guest.ticks;
        let first_tree = ticks == 0 && guest.ticks == 1;
        drop(locked);

        if self.standin.take().is_some() {
            cx.notify();
        }
        if self.min_width != min_width {
            self.min_width = min_width;
            cx.notify();
        }
        if let Some((root, rev)) = fresh {
            let _replacing = crate::perf::time(key, "replace");
            self.revision = rev;
            match (&self.tree, adopt) {
                // the tree's own notify dirties its window; nothing the
                // pane reads off the seat moved
                (Some(tree), None) => tree.update(cx, |tree, cx| tree.replace(root, cx)),
                (_, adopt) => {
                    self.mount(root, generation, key, adopt, cx);
                    cx.notify();
                }
            }
            if first_tree && crate::perf::on() {
                // a fresh view's first tree: from this turn's start, the
                // first to find it seated, to its tree mounted
                crate::perf::record(key, "first_tree", shown.elapsed().as_micros() as u64);
            }
        } else if ticked && let Some(tree) = &self.tree {
            tree.update(cx, |_, cx| cx.notify());
        }
        if commands > 0 {
            self.run_widget_commands(generation, cx);
        }
        for intent in intents {
            cx.emit(intent);
        }
        self.arm_clock(next_tick, cx);
        // frame-paced, never back to back: the guest ran out of budget and
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
        if self.standin.as_ref() != Some(&standin) {
            self.standin = Some(standin);
            cx.notify();
        }
    }

    /// A first tree, or a new deployment's: a fresh `ViewTree` carrying the
    /// old presentation, its editor store, a wake on replies and the route
    /// of its events back into the guest.
    fn mount(
        &mut self,
        root: wire::Node,
        generation: u64,
        key: crate::perf::Key,
        adopt: Option<(Arc<()>, tokio::sync::watch::Receiver<()>, bool, EditorStore)>,
        cx: &mut Context<Self>,
    ) {
        let Some((alive, mut changes, owed, inputs)) = adopt else {
            return;
        };
        // a first view here, or a new deployment's: its minimum may be
        // new, so the desk fits its windows to it
        cx.emit(Intent::Seated);
        if crate::perf::on() {
            crate::perf::mark(intern(&format!("first_seated.{}", self.module)));
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
        });
        tree.update(cx, |tree, cx| tree.set_editor_store(inputs, cx));
        let seat = self.mounted.clone();
        self._tree_events = Some(cx.subscribe(&tree, move |this, source, event, cx| {
            let activation = source.read(cx).take_user_activation(event);
            let mut locked = seat.lock().expect("module view lock");
            let Slot::Ready(guest) = &mut locked.slot else {
                return;
            };
            let current =
                guest.seated_generation() == generation && Arc::ptr_eq(&alive, &guest.alive);
            if current && guest.frame_rev == this.revision {
                guest.user_activation = activation;
                guest.pending.push(event.clone());
            }
            drop(locked);
            this.turn(cx);
        }));
        self.tree = Some(tree);
        if owed {
            // an answer that landed before the subscription still needs delivery
            self.wake(cx);
        }
    }

    /// A timer for the guest's next `clock.ticks`, re-armed when it moves.
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

    /// The guest's runnable `host.widget` commands, run in the pane's window
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
                        let mut locked = seat.lock().expect("module view lock");
                        let Slot::Ready(guest) = &mut locked.slot else {
                            return;
                        };
                        // the GUEST has to be the one that asked: a
                        // replacement or a reseat takes its queue with it.
                        // The FRAME does not: each command is judged
                        // against the tree standing now.
                        let same = guest.seated_generation() == generation
                            && this
                                .alive
                                .as_ref()
                                .is_some_and(|alive| Arc::ptr_eq(alive, &guest.alive));
                        if !same {
                            return;
                        }
                        let Some(tree) = &this.tree else {
                            return;
                        };
                        let focused = this.focused;
                        guest.execute_widget_commands(|command| {
                            if !focused && matches!(command, wire::WidgetCommand::Focus { .. }) {
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
        registry()
            .lock()
            .expect("module views")
            .remove(&(self.module, self.instance));
    }
}
