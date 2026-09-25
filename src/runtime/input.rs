//! Native observations never dispatch a second input into controls. The wire
//! observes the completed native dispatch, behind the widget's own output.
use super::*;
use gpui_kit::{
    self as gpui, AnyElement, App, Bounds, Context, DispatchPhase, Element, ElementId,
    GlobalElementId, Hitbox, HitboxBehavior, HitboxId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Point, Style, WeakEntity, Window,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub(super) fn deliver(guest: &mut Guest, event: wire::Event) -> bool {
    match event {
        wire::Event::Mouse { event, captured } => mouse(guest, event, captured),
        wire::Event::Observation { event, captured } => {
            if !guest.frame.event_interest.accepts(&event) || event.validate().is_err() {
                return false;
            }
            guest
                .pending
                .push(wire::Event::Observation { event, captured });
            true
        }
        event => {
            guest.pending.push(event);
            true
        }
    }
}

pub(super) fn mouse(guest: &mut Guest, event: wire::mouse::Event, captured: bool) -> bool {
    if !guest.frame.mouse_interest {
        return false;
    }
    let Some(event) = event.sanitize() else {
        return false;
    };
    if matches!(event, wire::mouse::Event::CursorMoved { .. }) {
        while matches!(
            guest.pending.last(),
            Some(wire::Event::Mouse {
                event: wire::mouse::Event::CursorMoved { .. },
                ..
            })
        ) {
            guest.pending.pop();
        }
    }
    guest.pending.push(wire::Event::Mouse { event, captured });
    true
}

pub(super) fn needs_focus(event: &wire::Event) -> bool {
    matches!(
        event,
        wire::Event::Keyboard { .. }
            | wire::Event::Observation {
                event: wire::events::Event::InputMethod(_),
                ..
            }
    )
}

#[derive(Clone)]
struct Route {
    seat: Arc<Mutex<Mounted>>,
    generation: u64,
    revision: u64,
    alive: Arc<()>,
    view: WeakEntity<NativeModuleView>,
}
impl Route {
    fn deliver(&self, event: wire::Event, cx: &mut App) {
        if needs_focus(&event)
            && self
                .view
                .upgrade()
                .is_none_or(|view| !view.read(cx).focused)
        {
            return;
        }
        let mut locked = self.seat.lock().expect("module view lock");
        let Slot::Ready(guest) = &mut locked.slot else {
            return;
        };
        if guest.seated_generation() != self.generation
            || !Arc::ptr_eq(&guest.alive, &self.alive)
            || guest.frame_rev != self.revision
        {
            let view = self.view.clone();
            cx.defer(move |cx| {
                let _ = view.update(cx, |_, cx| cx.notify());
            });
            return;
        }
        let accepted = deliver(guest, event);
        drop(locked);
        if accepted {
            let view = self.view.clone();
            cx.defer(move |cx| {
                let _ = view.update(cx, |_, cx| cx.notify());
            });
        }
    }
    fn deferred(&self, mut event: wire::Event, captured: Rc<Cell<bool>>, cx: &mut App) {
        let route = self.clone();
        // A capture callback runs before an input/button queues its Emit effect.
        // The second turn waits behind that effect and its subscription callback.
        cx.defer(move |cx| {
            cx.defer(move |cx| {
                match &mut event {
                    wire::Event::Mouse { captured: flag, .. }
                    | wire::Event::Keyboard { captured: flag, .. }
                    | wire::Event::Observation { captured: flag, .. } => *flag = captured.get(),
                    _ => {}
                }
                route.deliver(event, cx);
            })
        });
    }
}

/// The guest under the pointer, or the one whose drag holds it. The hitbox
/// blocks the mouse, so GPUI's hit test stops at the topmost layer: a guest
/// under another one is never hovered, as if each were its own window.
fn owns(hitbox: &Hitbox, position: Point<Pixels>, window: &Window) -> bool {
    match window.captured_hitbox() {
        Some(captured) => captured == hitbox.id,
        None => hitbox.is_hovered_at(position, window),
    }
}

/// A transparent element puts listeners outside the child's dispatch subtree.
pub(super) struct Observe {
    content: AnyElement,
    route: Route,
    files: Rc<RefCell<Vec<String>>>,
    pointer_inside: Rc<Cell<bool>>,
    pointer_held: Rc<Cell<Option<HitboxId>>>,
}
impl Observe {
    pub(super) fn new(
        content: AnyElement,
        view: &NativeModuleView,
        cx: &Context<NativeModuleView>,
    ) -> Self {
        Self {
            content,
            route: Route {
                seat: view.seat.clone(),
                generation: view.generation,
                revision: view.revision,
                alive: view.alive.clone().expect("mounted instance"),
                view: cx.entity().downgrade(),
            },
            files: view.hovered_files.clone(),
            pointer_inside: view.pointer_inside.clone(),
            pointer_held: view.pointer_held.clone(),
        }
    }
}
impl IntoElement for Observe {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Observe {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // The layer's whole rectangle is the guest's, not just its content's box.
        let content = self.content.request_layout(window, cx);
        let style = Style {
            size: gpui::Size::full(),
            ..Style::default()
        };
        (window.request_layout(style, [content], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::BlockMouse);
        if window
            .captured_hitbox()
            .is_some_and(|held| self.pointer_held.get() == Some(held))
        {
            window.capture_pointer(hitbox.id);
            self.pointer_held.set(Some(hitbox.id));
        }
        self.content.prepaint(window, cx);
        hitbox
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let hitbox = hitbox.clone();
        let pointer_inside = self.pointer_inside.clone();
        let inside = hitbox.clone();
        listen_mouse(
            window,
            self.route.clone(),
            move |event: &gpui::MouseMoveEvent, window| {
                let now = inside.is_hovered_at(event.position, window);
                match (pointer_inside.replace(now), now) {
                    (false, true) => Some(wire::mouse::Event::CursorEntered),
                    (true, false) => Some(wire::mouse::Event::CursorLeft),
                    _ => None,
                }
            },
        );
        let moved = hitbox.clone();
        listen_mouse(
            window,
            self.route.clone(),
            move |event: &gpui::MouseMoveEvent, window| {
                owns(&moved, event.position, window).then(|| wire::mouse::Event::CursorMoved {
                    x: f32::from(event.position.x - bounds.origin.x),
                    y: f32::from(event.position.y - bounds.origin.y),
                })
            },
        );
        // A press takes the pointer: the drag keeps delivering moves and
        // its release to this guest wherever the pointer goes.
        let pressed = hitbox.clone();
        let held = self.pointer_held.clone();
        listen_mouse(
            window,
            self.route.clone(),
            move |event: &gpui::MouseDownEvent, window| {
                owns(&pressed, event.position, window).then(|| {
                    window.capture_pointer(pressed.id);
                    held.set(Some(pressed.id));
                    wire::mouse::Event::ButtonPressed(button(event.button))
                })
            },
        );
        let released = hitbox.clone();
        listen_mouse(
            window,
            self.route.clone(),
            move |event: &gpui::MouseUpEvent, window| {
                owns(&released, event.position, window)
                    .then(|| wire::mouse::Event::ButtonReleased(button(event.button)))
            },
        );
        let pointer_inside = self.pointer_inside.clone();
        listen_mouse(
            window,
            self.route.clone(),
            move |_: &gpui::MouseExitEvent, _| {
                pointer_inside
                    .replace(false)
                    .then_some(wire::mouse::Event::CursorLeft)
            },
        );
        let scrolled = hitbox.clone();
        listen_mouse(
            window,
            self.route.clone(),
            move |event: &gpui::ScrollWheelEvent, window| {
                if !owns(&scrolled, event.position, window) {
                    return None;
                }
                Some(wire::mouse::Event::WheelScrolled {
                    delta: match event.delta {
                        gpui::ScrollDelta::Pixels(point) => wire::mouse::ScrollDelta::Pixels {
                            x: f32::from(point.x),
                            y: f32::from(point.y),
                        },
                        gpui::ScrollDelta::Lines(point) => wire::mouse::ScrollDelta::Lines {
                            x: point.x,
                            y: point.y,
                        },
                    },
                })
            },
        );
        listen_key(window, self.route.clone(), |event: &gpui::KeyDownEvent| {
            wire::keyboard::Event::Press {
                state: key_state(&event.keystroke),
                text: event.keystroke.key_char.clone(),
                repeat: event.is_held,
            }
        });
        listen_key(window, self.route.clone(), |event: &gpui::KeyUpEvent| {
            wire::keyboard::Event::Release(key_state(&event.keystroke))
        });
        // A modifier changing is NOT a keystroke: GPUI keeps its own listener
        // list for it, and a key listener registered for the event is never
        // called. Registered as a key listener, the guest learned a modifier
        // only from a key pressed while it was held — so ⌘-wheel never zoomed,
        // Alt-drag never duplicated, and Shift never held a run straight.
        // Nothing captures a modifier, so it is never captured.
        let route = self.route.clone();
        window.on_modifiers_changed(move |event: &gpui::ModifiersChangedEvent, _, cx| {
            route.deferred(
                wire::Event::Keyboard {
                    event: wire::keyboard::Event::Modifiers(modifiers(event.modifiers)),
                    captured: false,
                },
                Rc::new(Cell::new(false)),
                cx,
            );
        });
        let route = self.route.clone();
        let files = self.files.clone();
        let dispatch = RefCell::new(None);
        window.on_mouse_event(move |event: &gpui::FileDropEvent, phase, _, cx| {
            let Some(captured) = capture_flag(phase, &dispatch) else {
                return;
            };
            use wire::events::{Event as E, Window as W};
            let events = match event {
                gpui::FileDropEvent::Entered { paths, .. } => {
                    *files.borrow_mut() = paths
                        .0
                        .iter()
                        .filter_map(|path| {
                            path.to_str()
                                .filter(|path| path.len() <= wire::MAX_STRING_BYTES)
                                .map(str::to_owned)
                        })
                        .collect();
                    files
                        .borrow()
                        .iter()
                        .cloned()
                        .map(W::FileHovered)
                        .collect::<Vec<_>>()
                }
                gpui::FileDropEvent::Submit { .. } => std::mem::take(&mut *files.borrow_mut())
                    .into_iter()
                    .map(W::FileDropped)
                    .collect(),
                gpui::FileDropEvent::Exited | gpui::FileDropEvent::Ended => {
                    files.borrow_mut().clear();
                    vec![W::FilesHoveredLeft]
                }
                gpui::FileDropEvent::Pending { .. } => Vec::new(),
            };
            for event in events {
                route.deferred(
                    wire::Event::Observation {
                        event: E::Window(event),
                        captured: true,
                    },
                    captured.clone(),
                    cx,
                );
            }
        });
        self.content.paint(window, cx);
    }
}
fn capture_flag(
    phase: DispatchPhase,
    dispatch: &RefCell<Option<Rc<Cell<bool>>>>,
) -> Option<Rc<Cell<bool>>> {
    if phase == DispatchPhase::Bubble {
        if let Some(captured) = dispatch.borrow_mut().take() {
            captured.set(false);
        }
        return None;
    }
    let captured = Rc::new(Cell::new(true));
    *dispatch.borrow_mut() = Some(captured.clone());
    Some(captured)
}
fn listen_mouse<E: gpui::MouseEvent>(
    window: &mut Window,
    route: Route,
    convert: impl Fn(&E, &mut Window) -> Option<wire::mouse::Event> + 'static,
) {
    let dispatch = RefCell::new(None);
    window.on_mouse_event(move |event: &E, phase, window, cx| {
        let Some(captured) = capture_flag(phase, &dispatch) else {
            return;
        };
        if let Some(event) = convert(event, window) {
            route.deferred(
                wire::Event::Mouse {
                    event,
                    captured: true,
                },
                captured.clone(),
                cx,
            );
        }
    });
}
fn listen_key<E: gpui::KeyEvent>(
    window: &mut Window,
    route: Route,
    convert: impl Fn(&E) -> wire::keyboard::Event + 'static,
) {
    let dispatch = RefCell::new(None);
    window.on_key_event(move |event: &E, phase, _, cx| {
        let Some(captured) = capture_flag(phase, &dispatch) else {
            return;
        };
        route.deferred(
            wire::Event::Keyboard {
                event: convert(event),
                captured: true,
            },
            captured.clone(),
            cx,
        );
    });
}
fn button(value: gpui::MouseButton) -> wire::mouse::Button {
    match value {
        gpui::MouseButton::Left => wire::mouse::Button::Left,
        gpui::MouseButton::Right => wire::mouse::Button::Right,
        gpui::MouseButton::Middle => wire::mouse::Button::Middle,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Back) => wire::mouse::Button::Back,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward) => {
            wire::mouse::Button::Forward
        }
    }
}
fn modifiers(value: gpui::Modifiers) -> wire::keyboard::Modifiers {
    wire::keyboard::Modifiers {
        shift: value.shift,
        control: value.control,
        alt: value.alt,
        logo: value.platform,
        function: value.function,
    }
}
fn key(value: &str) -> wire::keyboard::Key {
    use wire::keyboard::{Key as K, Named as N};
    let named = match value {
        "enter" => N::Enter,
        "tab" => N::Tab,
        "space" => N::Space,
        "escape" => N::Escape,
        "backspace" => N::Backspace,
        "delete" => N::Delete,
        "insert" => N::Insert,
        "up" => N::ArrowUp,
        "down" => N::ArrowDown,
        "left" => N::ArrowLeft,
        "right" => N::ArrowRight,
        "home" => N::Home,
        "end" => N::End,
        "pageup" => N::PageUp,
        "pagedown" => N::PageDown,
        "shift" => N::Shift,
        "control" => N::Control,
        "alt" => N::Alt,
        "platform" => N::Super,
        "function" => N::Fn,
        "f1" => N::F1,
        "f2" => N::F2,
        "f3" => N::F3,
        "f4" => N::F4,
        "f5" => N::F5,
        "f6" => N::F6,
        "f7" => N::F7,
        "f8" => N::F8,
        "f9" => N::F9,
        "f10" => N::F10,
        "f11" => N::F11,
        "f12" => N::F12,
        "f13" => N::F13,
        "f14" => N::F14,
        "f15" => N::F15,
        "f16" => N::F16,
        "f17" => N::F17,
        "f18" => N::F18,
        "f19" => N::F19,
        "f20" => N::F20,
        "f21" => N::F21,
        "f22" => N::F22,
        "f23" => N::F23,
        "f24" => N::F24,
        "f25" => N::F25,
        "f26" => N::F26,
        "f27" => N::F27,
        "f28" => N::F28,
        "f29" => N::F29,
        "f30" => N::F30,
        "f31" => N::F31,
        "f32" => N::F32,
        "f33" => N::F33,
        "f34" => N::F34,
        "f35" => N::F35,
        other if other.chars().count() == 1 => return K::Character(other.into()),
        _ => return K::Unidentified,
    };
    K::Named(named)
}
fn key_state(value: &gpui::Keystroke) -> wire::keyboard::KeyState {
    wire::keyboard::KeyState {
        key: key(&value.key),
        modified_key: value.key_char.as_ref().map_or_else(
            || key(&value.key),
            |text| wire::keyboard::Key::Character(text.clone()),
        ),
        // GPUI publishes logical keys only; do not invent a physical scan code.
        physical_key: wire::keyboard::Physical::Unidentified(
            wire::keyboard::NativeCode::Unidentified,
        ),
        location: wire::keyboard::Location::Standard,
        modifiers: modifiers(value.modifiers),
    }
}
impl NativeModuleView {
    pub(crate) fn observe_window(&mut self, event: wire::events::Window, cx: &mut Context<Self>) {
        let Some(alive) = self.alive.clone() else {
            return;
        };
        let route = Route {
            seat: self.seat.clone(),
            generation: self.generation,
            revision: self.revision,
            alive,
            view: cx.entity().downgrade(),
        };
        route.deliver(
            wire::Event::Observation {
                event: wire::events::Event::Window(event),
                captured: false,
            },
            cx,
        );
    }
    pub(super) fn bind_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            self.observers
                .push(cx.observe_window_activation(window, |this, window, cx| {
                    this.observe_window(
                        if window.is_window_active() && this.focused {
                            wire::events::Window::Focused
                        } else {
                            wire::events::Window::Unfocused
                        },
                        cx,
                    );
                }));
            let window_id = window.window_handle();
            let context_id = format!("view{}", cx.entity().entity_id().as_u64());
            // Bound actions consume their key before low-level key dispatch.
            // This native post-action callback is the complementary route.
            self.observers
                .push(cx.observe_keystrokes(move |this, event, window, cx| {
                    let here = event.context_stack.iter().any(|context| {
                        context
                            .get("ducktape_guest")
                            .is_some_and(|id| id.as_ref() == context_id)
                    });
                    if window.window_handle() != window_id || event.action.is_none() || !here {
                        return;
                    }
                    let Some(alive) = this.alive.clone() else {
                        return;
                    };
                    let route = Route {
                        seat: this.seat.clone(),
                        generation: this.generation,
                        revision: this.revision,
                        alive,
                        view: cx.entity().downgrade(),
                    };
                    route.deferred(
                        wire::Event::Keyboard {
                            event: wire::keyboard::Event::Press {
                                state: key_state(&event.keystroke),
                                text: event.keystroke.key_char.clone(),
                                repeat: false,
                            },
                            captured: true,
                        },
                        Rc::new(Cell::new(true)),
                        cx,
                    );
                }));
        }
    }
}

#[path = "input/ime.rs"]
mod ime;
pub(crate) use ime::{ImeState, ime_events};
