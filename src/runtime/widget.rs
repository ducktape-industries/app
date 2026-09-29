use super::*;

mod present;
mod standin;
#[cfg(test)]
mod tests;

use standin::Standin;

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
    /// Drawn since the props were last set: a layer mounted this frame.
    pub(super) drawn: bool,
}

impl gpui_kit::EventEmitter<Intent> for NativeModuleView {}

impl NativeModuleView {
    /// The id around the view's tree: the AX test door (`ax::tree`) reads
    /// the module off it.
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
            drawn: false,
        }
    }

    pub(crate) fn set_focused(&mut self, focused: bool, cx: &mut gpui_kit::Context<Self>) {
        if self.focused != focused {
            self.focused = focused;
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
    /// answered.
    pub(super) fn turn(&mut self, cx: &mut gpui_kit::Context<Self>) {
        let seat = self.seat.clone();
        let mut locked = seat.lock().expect("module view lock");
        let Mounted { slot, props, .. } = &mut *locked;
        let Slot::Ready(guest) = slot else {
            return;
        };
        guest.sync_theme(gpui_kit::component::Theme::global(cx).is_dark());
        let again = guest.redraw(props);
        clipboard::mount(guest, cx);
        let intents = std::mem::take(&mut guest.intents);
        drop(locked);
        for intent in intents {
            cx.emit(intent);
        }
        if again {
            cx.notify();
        }
    }

    /// A hidden tab gets one bounded update before its native presenter leaves.
    pub(crate) fn hide(&mut self) -> Vec<Intent> {
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

    pub(super) fn bind_observers(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let current_window = window.window_handle().window_id();
        if self.observed_window != Some(current_window) {
            self.observers.clear();
            self.observed_window = Some(current_window);
        }
        if self.observers.is_empty() {
            self.observers
                .push(cx.observe_global::<gpui_kit::component::Theme>(|this, cx| {
                    this.turn(cx);
                    cx.notify();
                }));
            let window_id = window.window_handle().window_id();
            let view = cx.entity().downgrade();
            self.observers.push(cx.on_window_closed(move |cx, closed| {
                if closed != window_id {
                    return;
                }
                let view = view.clone();
                cx.defer(move |cx| {
                    let _ = view.update(cx, |view, cx| {
                        for intent in view.hide() {
                            cx.emit(intent);
                        }
                    });
                });
            }));
        }
    }
}

/// The narrowest a view is laid out, its manifest's `MIN_WINDOW_WIDTH`: a
/// window narrower scrolls it sideways.
fn laid_out_from(guest: &Guest) -> f32 {
    guest.min_width as f32
}

impl gpui_kit::Render for NativeModuleView {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{
            InteractiveElement as _, IntoElement as _, ParentElement as _,
            StatefulInteractiveElement as _, Styled as _,
        };
        self.bind_observers(window, cx);
        self.drawn = true;
        // every draw of the seat; the tree's own `renders` are the cache
        // misses among them
        crate::perf::count(
            crate::perf::Key::View {
                module: self.module,
                instance: self.instance,
            },
            "draws",
            1,
        );
        match self.frame(window, cx) {
            Ok(min_width) => match &self.content {
                Some(content) => {
                    let guest = crate::shell::layers::cached_unless_a11y(
                        content.clone().into(),
                        gpui_kit::StyleRefinement::default().size_full(),
                        window,
                    );
                    let mut context = gpui_kit::KeyContext::default();
                    context.set(
                        "ducktape_guest",
                        format!("view{}", cx.entity().entity_id().as_u64()),
                    );
                    // A view owns its own inset: a split pane runs to the edges.
                    // Narrower than its minimum, it scrolls sideways rather
                    // than being squeezed and cut at the window's edge. The
                    // layer occludes: a guest under another one is never
                    // hovered, as if each were its own window.
                    gpui_kit::div()
                        .id(self.ax_mark())
                        .key_context(context)
                        .size_full()
                        .overflow_hidden()
                        .overflow_x_scroll()
                        .child(
                            gpui_kit::div()
                                .size_full()
                                .min_w(gpui_kit::px(min_width))
                                .occlude()
                                .child(guest),
                        )
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
