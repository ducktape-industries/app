//! A node's `wire::Interactivity` onto a gpui element: focus and tab
//! order, tooltips, one listener per handler the view set, each emitting
//! the matching `wire::Event`, the keys the view consumes, the `KeyUp` owed
//! to a node the keyboard left with a key down in it, and a long press.

use super::{AuthoredPath, Tree, ViewTree};
use gpui_kit::{
    App, AppContext as _, Context, DispatchPhase, FocusHandle, IntoElement, KeyDownEvent,
    KeyUpEvent, Keystroke, LongPressEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MousePressureEvent, MouseUpEvent, PinchEvent, ScrollWheelEvent,
    StatefulInteractiveElement, Subscription, TouchPhase, Window,
};
use std::sync::Arc;
use std::time::Duration;
use view_wire as wire;

fn drawn(styles: &wire::Styles, id: wire::StyleId, cx: &App) -> gpui_kit::StyleRefinement {
    let scale = crate::session::text_scale(cx);
    Arc::unwrap_or_clone(super::scaled(super::drawn_style(styles, id), scale))
}

pub(super) fn apply<E: StatefulInteractiveElement>(
    mut element: E,
    interactivity: &wire::Interactivity,
    styles: &wire::Styles,
    focus_handle: Option<FocusHandle>,
    path: Option<&AuthoredPath>,
    tooltip_held: bool,
    cx: &mut Context<ViewTree>,
) -> E {
    if let Some(value) = interactivity.tab_stop {
        element = element.tab_stop(value);
    }
    if let Some(value) = interactivity.tab_index {
        element = element.tab_index(value as isize);
    }
    if interactivity.tab_group {
        element = element.tab_group();
    }
    if let Some(handle) = focus_handle.as_ref() {
        element = element.track_focus(handle);
    }
    if let Some(style) = interactivity.focus {
        let style = drawn(styles, style, cx);
        element = element.focus(move |_| style);
    }
    if let Some(style) = interactivity.in_focus {
        let style = drawn(styles, style, cx);
        element = element.in_focus(move |_| style);
    }
    if let Some(style) = interactivity.focus_visible {
        let style = drawn(styles, style, cx);
        element = element.focus_visible(move |_| style);
    }
    if let Some(group) = &interactivity.group {
        element = element.group(group.clone());
    }
    // a finger leaves no hover: on a touch device gpui would keep the
    // hover where the last tap landed, so a view's hover look is not drawn
    // there (its pressed look, `active`, is)
    let hovers = !crate::contract::os().facts().touch_first;
    if let Some(style) = interactivity.hover.filter(|_| hovers) {
        let style = drawn(styles, style, cx);
        element = element.hover(move |_| style);
    }
    if let Some(style) = interactivity.active {
        let style = drawn(styles, style, cx);
        element = element.active(move |_| style);
    }
    if let Some(group) = interactivity.group_hover.as_ref().filter(|_| hovers) {
        let style = drawn(styles, group.style, cx);
        element = element.group_hover(group.group.clone(), move |_| style);
    }
    if let Some(group) = &interactivity.group_active {
        let style = drawn(styles, group.style, cx);
        element = element.group_active(group.group.clone(), move |_| style);
    }
    if interactivity.occlude {
        element = element.occlude();
    }
    if interactivity.block_mouse_except_scroll {
        element = element.block_mouse_except_scroll();
    }
    element = apply_mouse(element, interactivity, cx);
    element = apply_keyboard(element, interactivity, path, cx);
    element = apply_misc(element, interactivity, styles, cx);
    if let Some(tooltip) = &interactivity.tooltip
        && tooltip_held
    {
        let (parent, request) = (cx.entity().downgrade(), tooltip.request);
        let delay = Duration::from_millis(tooltip.delay_ms);
        element = element.tooltip_show_delay(delay);
        // what the tree holds for the route when gpui shows the tooltip, not
        // when this frame drew: gpui keeps the builder of the frame the hover
        // began in, and the view's answer to that hover lands in the delay.
        // The content is drawn with the table it came with.
        let build = move |cx: &mut App| {
            let held = parent.upgrade().and_then(|tree| {
                let held = tree.read(cx).tooltips.get(&request)?;
                Some(Tree {
                    root: (*held.content).clone(),
                    styles: held.styles.clone(),
                })
            });
            match held {
                Some(tree) => super::tooltip_containment::build(parent.clone(), tree, cx),
                // the view took it back during the delay
                None => cx.new(|_| gpui_kit::Empty).into(),
            }
        };
        if tooltip.hoverable {
            element = element.hoverable_tooltip(move |_, cx| build(cx));
        } else {
            element = element.tooltip(move |_, cx| build(cx));
        }
    }
    element
}

fn apply_mouse<E: StatefulInteractiveElement>(
    mut element: E,
    interactivity: &wire::Interactivity,
    cx: &mut Context<ViewTree>,
) -> E {
    if let Some(handler) = interactivity.on_mouse_down {
        element =
            element.on_any_mouse_down(cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                cx.emit(wire::Event::MouseDown {
                    handler,
                    phase: wire::DispatchPhase::Bubble,
                    event: event.into(),
                });
            }));
    }
    if let Some(handler) = interactivity.capture_mouse_down {
        element =
            element.capture_any_mouse_down(cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                cx.emit(wire::Event::MouseDown {
                    handler,
                    phase: wire::DispatchPhase::Capture,
                    event: event.into(),
                });
            }));
    }
    if let Some(handler) = interactivity.on_mouse_down_out {
        element =
            element.on_mouse_down_out(cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                cx.emit(wire::Event::MouseDownOut {
                    handler,
                    event: event.into(),
                });
            }));
    }
    if let Some(handler) = interactivity.on_mouse_up {
        for button in MouseButton::all() {
            element = element.on_mouse_up(
                button,
                cx.listener(move |_, event: &MouseUpEvent, _, cx| {
                    cx.emit(wire::Event::MouseUp {
                        handler,
                        phase: wire::DispatchPhase::Bubble,
                        event: event.into(),
                    });
                }),
            );
        }
    }
    if let Some(handler) = interactivity.capture_mouse_up {
        element =
            element.capture_any_mouse_up(cx.listener(move |_, event: &MouseUpEvent, _, cx| {
                cx.emit(wire::Event::MouseUp {
                    handler,
                    phase: wire::DispatchPhase::Capture,
                    event: event.into(),
                });
            }));
    }
    if let Some(handler) = interactivity.on_mouse_up_out {
        for button in MouseButton::all() {
            element = element.on_mouse_up_out(
                button,
                cx.listener(move |_, event: &MouseUpEvent, _, cx| {
                    cx.emit(wire::Event::MouseUpOut {
                        handler,
                        event: event.into(),
                    });
                }),
            );
        }
    }
    if let Some(handler) = interactivity.on_mouse_pressure {
        element =
            element.on_mouse_pressure(cx.listener(move |_, event: &MousePressureEvent, _, cx| {
                cx.emit(wire::Event::MousePressure {
                    handler,
                    phase: wire::DispatchPhase::Bubble,
                    event: event.into(),
                });
            }));
    }
    if let Some(handler) = interactivity.capture_mouse_pressure {
        element = element.capture_mouse_pressure(cx.listener(
            move |_, event: &MousePressureEvent, _, cx| {
                cx.emit(wire::Event::MousePressure {
                    handler,
                    phase: wire::DispatchPhase::Capture,
                    event: event.into(),
                });
            },
        ));
    }
    if let Some(handler) = interactivity.on_mouse_move {
        element = element.on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
            cx.emit(wire::Event::MouseMove {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.on_mouse_exit {
        element = element.on_mouse_exit(cx.listener(move |_, event: &MouseExitEvent, _, cx| {
            cx.emit(wire::Event::MouseExit {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.on_scroll_wheel {
        element =
            element.on_scroll_wheel(cx.listener(move |_, event: &ScrollWheelEvent, _, cx| {
                cx.emit(wire::Event::ScrollWheel {
                    handler,
                    phase: wire::DispatchPhase::Bubble,
                    event: event.into(),
                });
            }));
    }
    if let Some(handler) = interactivity.on_pinch {
        element = element.on_pinch(cx.listener(move |_, event: &PinchEvent, _, cx| {
            cx.emit(wire::Event::Pinch {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.capture_pinch {
        element = element.capture_pinch(cx.listener(move |_, event: &PinchEvent, _, cx| {
            cx.emit(wire::Event::Pinch {
                handler,
                phase: wire::DispatchPhase::Capture,
                event: event.into(),
            });
        }));
    }
    element
}

/// A key the person pressed in the view activates it, as a click does;
/// Escape does not (the web's user-activation rule: a dismissal grants nothing).
fn mark_key(tree: &ViewTree, event: &KeyDownEvent) {
    if event.keystroke.key != "escape" {
        tree.activate();
    }
}

fn apply_keyboard<E: StatefulInteractiveElement>(
    mut element: E,
    interactivity: &wire::Interactivity,
    path: Option<&AuthoredPath>,
    cx: &mut Context<ViewTree>,
) -> E {
    // a node that listens for keys coming up is owed the KeyUp of every key
    // it heard go down, whatever takes the keyboard from it first (`HeldKey`)
    let listens_up = interactivity.on_key_up.is_some() || interactivity.capture_key_up.is_some();
    let holds = path.filter(|_| listens_up).cloned();
    let own_focus = interactivity.focus_handle;
    if let Some(handler) = interactivity.on_key_down {
        let holds = holds.clone();
        element =
            element.on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                mark_key(this, event);
                cx.emit(wire::Event::KeyDown {
                    handler,
                    phase: wire::DispatchPhase::Bubble,
                    event: event.into(),
                });
                if let Some(path) = &holds {
                    this.hold(path, own_focus, &event.keystroke, window, cx);
                }
            }));
    }
    if let Some(handler) = interactivity.capture_key_down {
        let holds = holds.clone();
        element =
            element.capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                mark_key(this, event);
                cx.emit(wire::Event::KeyDown {
                    handler,
                    phase: wire::DispatchPhase::Capture,
                    event: event.into(),
                });
                if let Some(path) = &holds {
                    this.hold(path, own_focus, &event.keystroke, window, cx);
                }
            }));
    }
    if let Some(handler) = interactivity.on_key_up {
        let holds = holds.clone();
        element = element.on_key_up(cx.listener(move |this, event: &KeyUpEvent, _, cx| {
            if let Some(path) = &holds {
                this.came_up(path, &event.keystroke.key);
            }
            cx.emit(wire::Event::KeyUp {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.capture_key_up {
        element = element.capture_key_up(cx.listener(move |this, event: &KeyUpEvent, _, cx| {
            if let Some(path) = &holds {
                this.came_up(path, &event.keystroke.key);
            }
            cx.emit(wire::Event::KeyUp {
                handler,
                phase: wire::DispatchPhase::Capture,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.on_modifiers_changed {
        element = element.on_modifiers_changed(cx.listener(
            move |_, event: &ModifiersChangedEvent, _, cx| {
                cx.emit(wire::Event::ModifiersChanged {
                    handler,
                    event: event.into(),
                });
            },
        ));
    }
    // the keys the view consumes stop here once this node's own
    // listeners (and gpui's keyboard click, which notes Enter and Space
    // first) have heard them, as gpui's `cx.stop_propagation()` in them
    // would; the sanitizer refused a frame with a key gpui cannot read
    let keys: Vec<Keystroke> = interactivity
        .consumes_keys
        .iter()
        .filter_map(|key| Keystroke::parse(key).ok())
        .collect();
    if !keys.is_empty() {
        element = element.on_key_down(move |event: &KeyDownEvent, _, cx| {
            let stroke = &event.keystroke;
            if keys
                .iter()
                .any(|key| key.key == stroke.key && key.modifiers == stroke.modifiers)
            {
                cx.stop_propagation();
            }
        });
    }
    element
}

/// A key a node heard go down and has not heard come up. gpui hands a
/// `KeyUp` only to the keyboard's path as it is when the key comes up, so
/// a node the keyboard left meanwhile (focus moved out of it, or the
/// window went to the back) would never hear it: the host tells it then
/// ([`ViewTree::lift`]), once.
pub(super) struct HeldKey {
    /// The node, by its own path: found again in the tree to be told.
    path: AuthoredPath,
    /// The stroke that went down, which the `KeyUp` carries back.
    keystroke: Keystroke,
    /// Fires as the keyboard leaves the focus that put the node on its path.
    _focus_out: Subscription,
    /// Fires as the window goes to the back. gpui tells that as focus
    /// leaving too, but only in a frame drawn while the window is at the
    /// back, and a minimized or covered window draws none before it is in
    /// front again (as `Windows::activation` lets a desk's hold go).
    _to_back: Subscription,
}

impl ViewTree {
    /// `keystroke` went down in the node at `path`, whose view focus
    /// handle is `own`: held until it comes up there or the keyboard
    /// leaves the node. A repeat of a held key holds nothing more.
    fn hold(
        &mut self,
        path: &AuthoredPath,
        own: Option<u64>,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = |held: &HeldKey| held.path == *path && held.keystroke.key == keystroke.key;
        if self.held_keys.iter().any(same) {
            return;
        }
        // the handle the node tracks (a container focused by path tracks
        // that one over the view's), else the focused one, which is in it:
        // the keyboard leaves the node as it leaves that handle
        let focus = self
            .focus_targets
            .get(path)
            .map(|(_, handle)| handle.clone())
            .or_else(|| own.and_then(|id| self.view_focus_targets.get(&id).cloned()))
            .or_else(|| window.focused(cx));
        let Some(focus) = focus else {
            return;
        };
        let (at, key) = (path.clone(), keystroke.key.clone());
        let focus_out = cx.on_focus_out(&focus, window, move |this, _, _, cx| {
            this.lift(&at, &key, cx)
        });
        let (at, key) = (path.clone(), keystroke.key.clone());
        let to_back = cx.observe_window_activation(window, move |this, window, cx| {
            if !window.is_window_active() {
                this.lift(&at, &key, cx)
            }
        });
        self.held_keys.push(HeldKey {
            path: path.clone(),
            keystroke: keystroke.clone(),
            _focus_out: focus_out,
            _to_back: to_back,
        });
    }

    /// `key` came up in the node at `path`, which heard it: nothing is owed.
    fn came_up(&mut self, path: &AuthoredPath, key: &str) {
        self.held_keys
            .retain(|held| held.path != *path || held.keystroke.key != key);
    }

    /// The keyboard left the node at `path` with `key` down in it: the node
    /// hears it come up, on the key-up handlers it has in the tree now (a
    /// listener the view dropped and set again has a new route), capture
    /// first as gpui calls them. A node no longer in the tree hears nothing.
    fn lift(&mut self, path: &AuthoredPath, key: &str, cx: &mut Context<Self>) {
        let Some(at) = self
            .held_keys
            .iter()
            .position(|held| held.path == *path && held.keystroke.key == key)
        else {
            return;
        };
        let keystroke = self.held_keys.swap_remove(at).keystroke;
        let mut handlers = None;
        super::commands::walk_authored_paths(&self.root, None, &mut Vec::new(), &mut |node, at| {
            if at == path && node.identity().is_some() {
                handlers = node.interactivity().map(|listens| {
                    [
                        (listens.capture_key_up, wire::DispatchPhase::Capture),
                        (listens.on_key_up, wire::DispatchPhase::Bubble),
                    ]
                });
            }
        });
        let event = wire::interactivity::KeyUp::from(&KeyUpEvent { keystroke });
        for (handler, phase) in handlers.into_iter().flatten() {
            if let Some(handler) = handler {
                cx.emit(wire::Event::KeyUp {
                    handler,
                    phase,
                    event: event.clone(),
                });
            }
        }
    }
}

fn apply_misc<E: StatefulInteractiveElement>(
    mut element: E,
    interactivity: &wire::Interactivity,
    styles: &wire::Styles,
    cx: &mut Context<ViewTree>,
) -> E {
    let tooltip_request = interactivity
        .tooltip
        .as_ref()
        .map(|tooltip| tooltip.request);
    if interactivity.on_hover.is_some() || tooltip_request.is_some() {
        let handler = interactivity.on_hover;
        element = element.on_hover(cx.listener(move |_, hovered, _, cx| {
            if let Some(handler) = handler {
                cx.emit(wire::Event::Hover {
                    handler,
                    hovered: *hovered,
                });
            }
            if *hovered && let Some(request) = tooltip_request {
                cx.emit(wire::Event::TooltipRequest {
                    request,
                    character_index: None,
                });
            }
        }));
    }
    element = element.hover_listener_mode(match interactivity.hover_listener_mode {
        wire::HoverListenerMode::InputModalityAware => {
            gpui_kit::HoverListenerMode::InputModalityAware
        }
        wire::HoverListenerMode::InputModalityIndependent => {
            gpui_kit::HoverListenerMode::InputModalityIndependent
        }
    });
    if let Some(handler) = interactivity.on_file_drop_exit {
        element = element.on_file_drop_exit(cx.listener(move |_, _, _, cx| {
            cx.emit(wire::Event::FileDropExit { handler });
        }));
    }
    // files the OS drags in arrive as gpui's `ExternalPaths` drag: a drop
    // is the person's own gesture, and the seat opens the files for the
    // view (`super::Dropped`)
    if let Some(handler) = interactivity.on_file_drop {
        element = element.on_drop(cx.listener(
            move |this, paths: &gpui_kit::ExternalPaths, _, cx| {
                this.activate();
                cx.emit(super::Dropped {
                    handler,
                    paths: paths.paths().to_vec(),
                });
            },
        ));
    }
    if let Some(style) = interactivity.file_drag_over {
        let style = drawn(styles, style, cx);
        element = element.drag_over::<gpui_kit::ExternalPaths>(move |_, _, _, _| style.clone());
    }
    if let Some(handler) = interactivity.on_aux_click {
        element = element.on_aux_click(cx.listener(
            move |this, event: &gpui_kit::ClickEvent, _, cx| {
                this.activate();
                cx.emit(wire::Event::AuxClick {
                    handler,
                    event: event.into(),
                });
            },
        ));
    }
    element
}

/// A zero-paint canvas over a node that hears aux clicks: a finger held on
/// it is that click, as `TouchClickEvent::long_press` says (touch's twin of
/// the right button), which gpui-pre recognizes but hands to no aux-click
/// listener. The press is claimed, so no tap follows it and nothing under
/// the node hears it.
pub(super) fn long_press(
    handler: u32,
    follows: Follows,
    cx: &mut Context<ViewTree>,
) -> impl IntoElement + use<> {
    use gpui_kit::Styled as _;
    let view = cx.entity().downgrade();
    gpui_kit::canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui_kit::HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                if event.phase != TouchPhase::Started {
                    follow(&view, &follows, event, window, cx);
                    return;
                }
                if window.default_prevented() || !hitbox.is_hovered(window) {
                    return;
                }
                let Some(tree) = view.upgrade() else {
                    return;
                };
                window.prevent_default();
                window.capture_long_press(&tree);
                cx.stop_propagation();
                let click = gpui_kit::ClickEvent::Touch(gpui_kit::TouchClickEvent {
                    position: event.start_position,
                    tap_count: 1,
                    long_press: true,
                });
                tree.update(cx, |this, cx| {
                    this.activate();
                    this.long_pressed = Some(follows.at.clone());
                    cx.emit(wire::Event::AuxClick {
                        handler,
                        event: (&click).into(),
                    });
                });
            });
        },
    )
    .absolute()
    .inset_0()
}

#[derive(Clone, PartialEq)]
pub(super) struct Pressed {
    pub(super) path: AuthoredPath,
    pub(super) id: gpui_kit::ElementId,
}

pub(super) struct Follows {
    pub(super) at: Pressed,
    pub(super) moves: Option<u32>,
    pub(super) ups: Option<u32>,
}

fn follow(
    view: &gpui_kit::WeakEntity<ViewTree>,
    follows: &Follows,
    event: &LongPressEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(tree) = view.upgrade() else {
        return;
    };
    if !window.has_long_press_capture(&tree) {
        return;
    }
    tree.update(cx, |this, cx| {
        if this.long_pressed.as_ref() != Some(&follows.at) {
            return;
        }
        match event.phase {
            TouchPhase::Started => {}
            TouchPhase::Moved => {
                if let Some(handler) = follows.moves {
                    let moved = MouseMoveEvent {
                        position: event.position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Default::default(),
                    };
                    cx.emit(wire::Event::MouseMove {
                        handler,
                        phase: wire::DispatchPhase::Bubble,
                        event: (&moved).into(),
                    });
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                this.long_pressed = None;
                let position = match event.phase {
                    TouchPhase::Ended => event.position,
                    _ => event.start_position,
                };
                if let Some(handler) = follows.ups {
                    let up = MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Default::default(),
                        click_count: 1,
                    };
                    cx.emit(wire::Event::MouseUp {
                        handler,
                        phase: wire::DispatchPhase::Bubble,
                        event: (&up).into(),
                    });
                }
            }
        }
    });
}
