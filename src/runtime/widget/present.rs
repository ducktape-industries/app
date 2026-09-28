//! One GPUI frame of a seat: the guest stepped, its tree put under the
//! entity as a `ViewTree`, its events routed back, its widget commands
//! run once the tree has mounted — or the standin to draw instead.
use super::standin::stage_words;
use super::*;

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

impl NativeModuleView {
    /// One GPUI frame of the seat: the guest is stepped and its tree put
    /// under this entity, with the width it is laid out from; or the
    /// standin to draw instead comes back.
    pub(super) fn frame(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> Result<f32, Standin> {
        let mounted = self.seat.clone();
        let mut locked = mounted.lock().expect("module view lock");
        let shown = Instant::now();
        locked.shown = Some(shown);
        let Mounted { slot, props, .. } = &mut *locked;
        let guest = seated(slot, self.module, window)?;
        guest.instance = self.instance;
        let ticks = guest.ticks;
        guest.set_visible(true);
        guest.sync_theme(gpui_kit::component::Theme::global(cx).is_dark());
        let again = guest.redraw(props);
        clipboard::mount(guest, cx);
        if again {
            window.request_animation_frame();
        }
        self.arm_clock_deadline(kernel::next_tick(&guest.clocks), cx);
        if let Some(fault) = &guest.fault {
            return Err((&Failure::Trapped(fault.clone())).into());
        }
        let same_instance = self.generation == guest.seated_generation()
            && self
                .alive
                .as_ref()
                .is_some_and(|alive| Arc::ptr_eq(alive, &guest.alive));
        if !same_instance || self.revision != guest.frame_rev {
            self.mount_content(guest, &mounted, same_instance, window, cx);
        }
        if let Some(content) = &self.content {
            if ticks != guest.ticks {
                content.update(cx, |_, cx| cx.notify());
            }
            if guest.runnable_widget_commands() > 0 {
                self.schedule_widget_commands(guest, &mounted, window, cx);
            }
        }
        for intent in std::mem::take(&mut guest.intents) {
            cx.emit(intent);
        }
        if ticks == 0 && guest.ticks == 1 && crate::perf::on() {
            // a fresh view's first tree: from this frame's start, the first
            // to find it seated, to its tree mounted
            crate::perf::record(
                guest.perf_key(),
                "first_tree",
                shown.elapsed().as_micros() as u64,
            );
        }
        Ok(laid_out_from(guest))
    }

    /// A timer for the guest's next `clock.ticks`, re-armed when it moves;
    /// it wakes this entity, whose redraw fires the tick.
    fn arm_clock_deadline(&mut self, next: Option<Instant>, cx: &mut gpui_kit::Context<Self>) {
        if self.deadline.as_ref().map(|(due, _)| *due) == next {
            return;
        }
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

    /// The guest's tree, drawn: replaced in the `ViewTree` this entity
    /// holds for the same guest instance, or, for a new one, a fresh tree
    /// carrying the old presentation, its editor store, a wake on replies
    /// and the route of its events back into the guest.
    fn mount_content(
        &mut self,
        guest: &mut Guest,
        seat: &Arc<Mutex<Mounted>>,
        same_instance: bool,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let generation = guest.seated_generation();
        let key = guest.perf_key();
        let _replacing = crate::perf::time(key, "replace");
        let mut root = guest.frame.root.clone().unwrap_or_else(wire::Node::empty);
        guest.pictures.hydrate(&mut root);
        let root = native_root(root);
        self.revision = guest.frame_rev;
        if let (Some(content), true) = (&self.content, same_instance) {
            content.update(cx, |tree, cx| tree.replace(root, cx));
            return;
        }
        // a first view here, or a new deployment's: its minimum may be
        // new, so the desk fits its windows to it
        cx.emit(Intent::Seated);
        if crate::perf::on() {
            crate::perf::mark(intern(&format!("first_seated.{}", self.module)));
        }
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
            crate::render::ViewTree::new(root)
                .with_presentation(presentation)
                .with_perf_key(key)
        });
        content.update(cx, |tree, cx| {
            tree.set_editor_store(guest.inputs.clone(), cx)
        });
        let seat = seat.clone();
        let alive = guest.alive.clone();
        self.subscription = Some(cx.subscribe(&content, move |this, source, event, cx| {
            let activation = source.read(cx).take_user_activation(event);
            let mut locked = seat.lock().expect("module view lock");
            let Slot::Ready(guest) = &mut locked.slot else {
                return;
            };
            let current_instance =
                guest.seated_generation() == generation && Arc::ptr_eq(&alive, &guest.alive);
            if !current_instance || guest.frame_rev != this.revision {
                cx.notify();
                return;
            }
            guest.user_activation = activation;
            guest.pending.push(event.clone());
            cx.notify();
        }));
        self.content = Some(content);
    }

    /// The guest's runnable `host.widget` commands, run once this frame's
    /// child tree has mounted: a newly opened menu or input cannot take
    /// focus before that render.
    fn schedule_widget_commands(
        &self,
        guest: &Guest,
        seat: &Arc<Mutex<Mounted>>,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let generation = guest.seated_generation();
        let view = cx.entity().downgrade();
        let seat = seat.clone();
        let alive = guest.alive.clone();
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
                let same_guest =
                    guest.seated_generation() == generation && Arc::ptr_eq(&alive, &guest.alive);
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
                    if !this.focused && matches!(command, wire::WidgetCommand::Focus { .. }) {
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

/// The seated guest, or the standin for a seat with none: the network's
/// word for an empty slot, the failure, or the load's stage words (and
/// another frame asked for, to follow the load).
fn seated<'a>(
    slot: &'a mut Slot,
    module: &str,
    window: &mut gpui_kit::Window,
) -> Result<&'a mut Guest, Standin> {
    match slot {
        Slot::Ready(guest) => Ok(guest),
        Slot::Empty => Err(format!(
            "This network has no {module} view. An admin can activate a deployment that ships one."
        )
        .into()),
        Slot::Failed(failure) => Err((&*failure).into()),
        loading => {
            window.request_animation_frame();
            Err(Standin {
                title: None,
                words: stage_words(loading),
                loading: true,
                retry: false,
            })
        }
    }
}
