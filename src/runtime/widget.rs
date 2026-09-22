use super::*;

// ---------- the widget ----------

/// The native window retains this entity while a tab is open. A deployment
/// replacement gets a fresh native tree, so no focus or event route survives
/// across guest instances; ordinary guest frames retain keyed control state.
pub(crate) struct NativeModuleView {
    pub(super) module: &'static str,
    pub(super) instance: u64,
    pub(super) seat: Arc<Mutex<Mounted>>,
    pub(super) focused: bool,
    pub(super) observed_window: Option<gpui_kit::WindowId>,
    pub(super) content: Option<gpui_kit::Entity<crate::render::ViewTree>>,
    pub(super) subscription: Option<gpui_kit::Subscription>,
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) alive: Option<Arc<()>>,
    pub(super) replies_changed: Option<gpui_kit::Task<()>>,
    pub(super) deadline: Option<(Instant, gpui_kit::Task<()>)>,
    pub(super) observers: Vec<gpui_kit::Subscription>,
    pub(super) hovered_files: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    pub(super) pointer_inside: std::rc::Rc<std::cell::Cell<bool>>,
    /// The hitbox whose press took the pointer; GPUI numbers hitboxes per
    /// frame, so the presenter re-arms the capture on each one it paints.
    pub(super) pointer_held: std::rc::Rc<std::cell::Cell<Option<gpui_kit::HitboxId>>>,
    /// Drawn since the props were last set: a layer mounted this frame.
    pub(super) drawn: bool,
}

impl gpui_kit::EventEmitter<ModuleViewEvent> for NativeModuleView {}

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
    let mut host_root = gpui_kit::div();
    host_root = host_root.size_full();
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: host_root.style().clone(),
        interactivity: Default::default(),
        children: vec![root],
    })
}

/// What a tab draws where its view is not: the load's stage over a skeleton
/// of a view, or why there is none — named, with Retry where a retry helps.
pub(super) struct Standin {
    pub(super) title: Option<&'static str>,
    pub(super) words: String,
    pub(super) loading: bool,
    pub(super) retry: bool,
}

impl From<String> for Standin {
    fn from(words: String) -> Self {
        Standin {
            title: None,
            words,
            loading: false,
            retry: false,
        }
    }
}

impl From<&Failure> for Standin {
    fn from(failure: &Failure) -> Self {
        Standin {
            title: Some(failure.title()),
            words: failure.to_string(),
            loading: false,
            retry: true,
        }
    }
}

/// The words a loading tab says for its stage.
pub(super) fn stage_words(slot: &Slot) -> String {
    let size = |bytes: u64| match bytes < 1_000_000 {
        true => format!("{} KB", bytes.div_ceil(1000)),
        false => format!("{:.1} MB", bytes as f64 / 1e6),
    };
    match slot {
        Slot::Fetching {
            received,
            total: Some(total),
        } => format!(
            "Fetching the view — {} of {}",
            size(*received),
            size(*total)
        ),
        Slot::Fetching { received, .. } => format!("Fetching the view — {}", size(*received)),
        Slot::Compiling => "Compiling the view…".into(),
        _ => "Loading the view…".into(),
    }
}

impl NativeModuleView {
    /// The id around the view's tree: the test door reads the module off it.
    pub(super) fn ax_mark(&self) -> gpui_kit::ElementId {
        gpui_kit::ElementId::Name(format!("{}{}", crate::ax::VIEW_MARK, self.module).into())
    }

    pub(crate) fn new(module: &'static str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let instance = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            module,
            instance,
            seat: mounted(module, instance),
            focused: true,
            observed_window: None,
            content: None,
            subscription: None,
            generation: 0,
            revision: 0,
            alive: None,
            replies_changed: None,
            deadline: None,
            observers: Vec::new(),
            hovered_files: Default::default(),
            pointer_inside: Default::default(),
            pointer_held: Default::default(),
            drawn: false,
        }
    }

    pub(crate) fn set_focused(&mut self, focused: bool, cx: &mut gpui_kit::Context<Self>) {
        if self.focused != focused {
            self.focused = focused;
            self.observe_window(
                if focused {
                    wire::events::Window::Focused
                } else {
                    wire::events::Window::Unfocused
                },
                cx,
            );
            cx.notify();
        }
    }

    pub(crate) fn set_props(&mut self, props: Vec<u8>, cx: &mut gpui_kit::Context<Self>) {
        let seat = self.seat.clone();
        let mut seat = seat.lock().expect("module view lock");
        let changed = seat.props.as_ref() != Some(&props);
        if changed {
            seat.props = Some(props);
            cx.notify();
        }
        // a seat no layer draws this frame is still turned once, after it:
        // an overlay mounts only once it draws, and it draws only once it
        // has run (#110)
        self.drawn = false;
        let view = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = view.update(cx, |view, cx| {
                if !view.drawn {
                    view.turn(cx);
                }
            });
        });
    }

    /// One turn of a seated guest no layer draws: the props, replies and
    /// presses it is owed go in, and the requests its tick makes are
    /// answered — a chord claim among them.
    pub(super) fn turn(&mut self, cx: &mut gpui_kit::Context<Self>) {
        let seat = self.seat.clone();
        let mut locked = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *locked;
        let Slot::Ready(guest) = slot else {
            return;
        };
        guest.sync_theme(gpui_kit::component::Theme::global(cx).is_dark());
        let again = guest.redraw(props);
        filesystem::mount(guest, cx);
        media::mount(guest, cx);
        let intents = std::mem::take(&mut guest.intents);
        drop(locked);
        for intent in intents {
            cx.emit(intent);
        }
        if again {
            cx.notify();
        }
    }

    /// A chord was pressed: if this seat's module claimed it, its guest is
    /// told and redrawn, and the press is spent. Says whether it landed, so
    /// the shell stops at the seat that took it.
    pub(crate) fn chord(&mut self, chord: &str, cx: &mut gpui_kit::Context<Self>) -> bool {
        if !self.focused || chord_holder(chord) != Some(self.module) {
            return false;
        }
        let seat = self.seat.clone();
        let mut mounted = seat.lock().expect("module view lock");
        let Slot::Ready(guest) = &mut mounted.slot else {
            return false;
        };
        let taken = guest.chord_pressed(chord);
        drop(mounted);
        if taken && !self.drawn {
            self.turn(cx);
        }
        if taken {
            cx.notify();
        }
        taken
    }

    /// A hidden tab gets one bounded update before its native presenter leaves.
    pub(crate) fn hide(&mut self) -> Vec<ModuleViewEvent> {
        let Some(alive) = &self.alive else {
            return Vec::new();
        };
        let seat = self.seat.clone();
        let mut mounted = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *mounted;
        let Slot::Ready(guest) = slot else {
            return Vec::new();
        };
        let owns_instance =
            guest.seated_generation() == self.generation && Arc::ptr_eq(alive, &guest.alive);
        if !owns_instance || !guest.visible {
            return Vec::new();
        }
        guest.set_visible(false);
        guest.redraw(props);
        std::mem::take(&mut guest.intents)
    }

    /// Closing has no next paint. Deliver the final semantic observation through
    /// one bounded guest redraw and return its intents to the surviving shell.
    pub(crate) fn observe_final_window_event(
        &mut self,
        event: wire::events::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Vec<ModuleViewEvent> {
        let Some(alive) = &self.alive else {
            return Vec::new();
        };
        let seat = self.seat.clone();
        let mut mounted = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *mounted;
        let Slot::Ready(guest) = slot else {
            return Vec::new();
        };
        if guest.seated_generation() != self.generation || !Arc::ptr_eq(alive, &guest.alive) {
            return Vec::new();
        }
        let accepted = input::deliver(
            guest,
            wire::Event::Observation {
                event: wire::events::Event::Window(event),
                captured: false,
            },
        );
        if !accepted {
            return Vec::new();
        }
        guest.redraw(props);
        cx.notify();
        std::mem::take(&mut guest.intents)
    }

    pub(super) fn frame(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Result<(), Standin> {
        let mounted = self.seat.clone();
        let mut locked = mounted.lock().expect("module view lock");
        locked.shown = Some(Instant::now());
        let Mounted { slot, props, .. } = &mut *locked;
        let guest = match slot {
            Slot::Ready(guest) => guest,
            Slot::Empty => {
                return Err(format!(
                    "This network has no {} view. An admin can activate a deployment that ships one.",
                    self.module
                )
                .into());
            }
            Slot::Failed(failure) => return Err((&*failure).into()),
            loading => {
                window.request_animation_frame();
                return Err(Standin {
                    title: None,
                    words: stage_words(loading),
                    loading: true,
                    retry: false,
                });
            }
        };
        let generation = guest.seated_generation();
        let ticks = guest.ticks;
        guest.set_visible(true);
        guest.sync_theme(gpui_kit::component::Theme::global(cx).is_dark());
        let again = guest.redraw(props);
        filesystem::mount(guest, cx);
        media::mount(guest, cx);
        if again {
            window.request_animation_frame();
        }
        let next = kernel::next_tick(&guest.clocks);
        let deadline_changed = self.deadline.as_ref().map(|(due, _)| *due) != next;
        if deadline_changed {
            self.deadline = next.map(|due| {
                let timer = cx
                    .background_executor()
                    .timer(due.saturating_duration_since(Instant::now()));
                let task = cx.spawn(async move |view, cx| {
                    timer.await;
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
                (due, task)
            });
        }
        if let Some(fault) = &guest.fault {
            return Err((&Failure::Trapped(fault.clone())).into());
        }
        let same_instance = self.generation == generation
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &guest.alive));
        let changed = !same_instance || self.revision != guest.frame_rev;
        if changed {
            let mut root = guest.frame.root.clone().unwrap_or_else(wire::Node::empty);
            guest.pictures.hydrate(&mut root);
            let root = native_root(root);
            self.revision = guest.frame_rev;
            match (&self.content, same_instance) {
                (Some(content), true) => content.update(cx, |tree, cx| tree.replace(root, cx)),
                _ => {
                    self.generation = generation;
                    self.alive = Some(guest.alive.clone());
                    let mut changes = guest.replies.changes();
                    self.replies_changed = Some(cx.spawn(async move |view, cx| {
                        while changes.changed().await.is_ok() {
                            if view.update(cx, |_, cx| cx.notify()).is_err() {
                                break;
                            }
                        }
                    }));
                    // An answer that landed before subscription still needs
                    // delivery; later answers wake the entity directly.
                    if guest.replies.answer_owed() {
                        window.request_animation_frame();
                    }
                    let presentation = self
                        .content
                        .as_ref()
                        .map(|content| content.read(cx).presentation(window, cx))
                        .unwrap_or_default();
                    let content = cx.new(|_| {
                        crate::render::ViewTree::new(root).with_presentation(presentation)
                    });
                    content.update(cx, |tree, cx| {
                        tree.set_editor_store(guest.inputs.clone(), cx)
                    });
                    let seat = mounted.clone();
                    let alive = guest.alive.clone();
                    self.subscription =
                        Some(cx.subscribe(&content, move |this, source, event, cx| {
                            if !this.focused && input::needs_focus(event) {
                                return;
                            }
                            let activation = source.read(cx).take_user_activation(event);
                            let mut locked = seat.lock().expect("module view lock");
                            let Slot::Ready(guest) = &mut locked.slot else {
                                return;
                            };
                            let current_instance = guest.seated_generation() == generation
                                && Arc::ptr_eq(&alive, &guest.alive);
                            if !current_instance || guest.frame_rev != this.revision {
                                cx.notify();
                                return;
                            }
                            guest.user_activation = activation;
                            input::deliver(guest, event.clone());
                            cx.notify();
                        }));
                    self.content = Some(content);
                }
            }
        }
        if let Some(content) = &self.content {
            if ticks != guest.ticks {
                content.update(cx, |_, cx| cx.notify());
            }
            let commands_ready = guest.runnable_widget_commands() > 0;
            if commands_ready {
                let view = cx.entity().downgrade();
                let seat = mounted.clone();
                let alive = guest.alive.clone();
                // The child tree mounts during this frame. A newly opened
                // menu or input cannot receive focus before that render.
                window.defer(cx, move |window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        let mut locked = seat.lock().expect("module view lock");
                        let Slot::Ready(guest) = &mut locked.slot else {
                            return;
                        };
                        // The GUEST has to be the one that asked — a
                        // replacement or a reseat takes its queue with it.
                        // The FRAME does not: a live view replaces its frame
                        // between the press and this deferred run, and each
                        // command is judged against the tree standing now.
                        let same_guest = guest.seated_generation() == generation
                            && Arc::ptr_eq(&alive, &guest.alive);
                        if !same_guest {
                            return;
                        }
                        if guest.runnable_widget_commands() == 0 {
                            cx.notify();
                            return;
                        }
                        let Some(content) = &this.content else {
                            return;
                        };
                        guest.execute_widget_commands(|command| {
                            if !this.focused && matches!(command, wire::WidgetCommand::Focus { .. })
                            {
                                return Err("view is not focused".into());
                            }
                            content.update(cx, |tree, cx| {
                                tree.execute_widget_command(command, window, cx)
                            })
                        });
                        cx.notify();
                    });
                });
            }
        }
        for intent in std::mem::take(&mut guest.intents) {
            cx.emit(intent);
        }
        Ok(())
    }
}

impl NativeModuleView {
    /// THE ONE STAND-IN every tab draws where its view is not, native, so it
    /// draws before any wasm exists: a loading view says its stage over a
    /// skeleton laid out as a view lays itself out — a heading, then rows,
    /// from the top of the full pane the view will take, so nothing moves
    /// when it seats — and a failed one says what failed, why, and offers
    /// Retry.
    pub(super) fn standin(
        &self,
        standin: Standin,
        cx: &mut gpui_kit::Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::component::button::Button;
        use gpui_kit::{
            FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Role,
            StatefulInteractiveElement as _, Styled as _, div, px, relative,
        };
        let Standin {
            title,
            words,
            loading,
            retry: offers_retry,
        } = standin;
        let module = self.module;
        let instance = self.instance;
        // a stable id per kind, so the tree tells a load on its way from a
        // view that is not there
        let (id, words_id) = match loading {
            true => ("view-loading", "view-loading-stage"),
            false => ("view-unavailable", "view-unavailable-reason"),
        };
        // a failure is announced like any other alert (screens.rs's
        // connect-error, sign_in.rs's unlock-error): a name, not just a
        // role, or the door's compact filter drops it
        let alert_label = match &title {
            Some(title) => format!("{title}: {words}"),
            None => words.clone(),
        };
        // the whole sentence, wrapped inside the pane: one line wider than
        // the pane is centred off both edges, and a reader loses its start
        // and its end — what failed, and what to do about it
        let reason = div().id(id).max_w_full().flex().flex_col().gap_2();
        let reason = match loading {
            true => reason,
            false => reason.role(Role::Alert).aria_label(alert_label),
        };
        let reason = reason
            .children(title.map(|title| {
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(gpui_kit::Text::new(
                        "view-unavailable-title".into(),
                        title.into(),
                    ))
            }))
            .child(gpui_kit::Text::new(words_id.into(), words.into()))
            .children(offers_retry.then(|| {
                Button::new("view-retry")
                    .label("Retry")
                    .outline()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        drop(retry(module, instance));
                        cx.notify();
                    }))
            }));
        #[cfg(test)]
        let reason = {
            use gpui_kit::test::TestSupportExt as _;
            reason.test_support()
        };
        let pane = div().id(self.ax_mark()).size_full().flex().flex_col().p_4();
        if !loading {
            return pane
                .items_center()
                .justify_center()
                .child(reason)
                .into_any_element();
        }
        let faint = gpui_kit::hsla(0., 0., 0.5, 0.14);
        let bar = |width: f32, height: f32| {
            div()
                .w(relative(width))
                .h(px(height))
                .rounded(px(4.))
                .bg(faint)
        };
        pane.gap_3()
            .child(bar(0.3, 22.))
            .child(div().text_size(px(12.)).opacity(0.7).child(reason))
            .children([0.9, 0.7, 0.8, 0.55, 0.85, 0.65].map(|width| bar(width, 14.)))
            .into_any_element()
    }
}

impl gpui_kit::Render for NativeModuleView {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{
            InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _,
        };
        self.bind_observers(window, cx);
        self.drawn = true;
        match self.frame(window, cx) {
            Ok(()) => match &self.content {
                Some(content) => {
                    let content = content.clone();
                    // GPUI rebuilds the accessibility tree from prepaint on
                    // every frame and a cached view replays paint without
                    // it, so a cached guest tree has no nodes and no
                    // focus for a screen reader. While one is listening,
                    // render the tree in full each frame as before.
                    let guest = if window.is_a11y_active() {
                        content.clone().into_any_element()
                    } else {
                        content
                            .clone()
                            .cached(gpui_kit::StyleRefinement::default().size_full())
                            .into_any_element()
                    };
                    let mut context = gpui_kit::KeyContext::default();
                    context.set(
                        "ducktape_guest",
                        format!("view{}", cx.entity().entity_id().as_u64()),
                    );
                    // A view owns its own inset: a split pane runs to the edges.
                    gpui_kit::div()
                        .id(self.ax_mark())
                        .key_context(context)
                        .size_full()
                        // Observe wraps the guest: its hitbox sits under the
                        // guest's controls, a sibling after them would block
                        // their clicks.
                        .child(input::Observe::new(guest, self, cx))
                        .into_any_element()
                }
                None => gpui_kit::div().size_full().into_any_element(),
            },
            Err(standin) => self.standin(standin, cx),
        }
    }
}

impl Drop for NativeModuleView {
    fn drop(&mut self) {
        registry()
            .lock()
            .expect("module views")
            .remove(&(self.module, self.instance));
    }
}

#[cfg(test)]
#[path = "widget_tests.rs"]
mod tests;
