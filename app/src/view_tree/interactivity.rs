use super::ViewTree;
use gpui_kit::{
    AppContext as _, Context, Div, FocusHandle, InteractiveElement as _, KeyDownEvent, KeyUpEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MousePressureEvent, MouseUpEvent, PinchEvent, ScrollWheelEvent, Stateful,
    StatefulInteractiveElement as _, WindowControlArea,
};
use std::time::Duration;
use view_wire as wire;

pub(super) fn apply(
    mut element: Stateful<Div>,
    interactivity: &wire::Interactivity,
    focus_handle: Option<FocusHandle>,
    cx: &mut Context<ViewTree>,
) -> Stateful<Div> {
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
    if let Some(style) = &interactivity.focus {
        let style = style.clone();
        element = element.focus(move |_| style);
    }
    if let Some(style) = &interactivity.in_focus {
        let style = style.clone();
        element = element.in_focus(move |_| style);
    }
    if let Some(style) = &interactivity.focus_visible {
        let style = style.clone();
        element = element.focus_visible(move |_| style);
    }
    if let Some(context) = &interactivity.key_context {
        element = element.key_context(context.as_ref());
    }
    if interactivity.occlude {
        element = element.occlude();
    }
    if interactivity.block_mouse_except_scroll {
        element = element.block_mouse_except_scroll();
    }
    if let Some(area) = interactivity.window_control_area {
        element = element.window_control_area(match area {
            wire::WindowControlArea::Drag => WindowControlArea::Drag,
            wire::WindowControlArea::Close => WindowControlArea::Close,
            wire::WindowControlArea::Max => WindowControlArea::Max,
            wire::WindowControlArea::Min => WindowControlArea::Min,
        });
    }
    element = apply_mouse(element, interactivity, cx);
    element = apply_keyboard(element, interactivity, cx);
    element = apply_misc(element, interactivity, cx);
    if let Some(tooltip) = &interactivity.tooltip
        && let Some(content) = &tooltip.content
    {
        let content = content.clone();
        let delay = Duration::from_millis(tooltip.delay_ms);
        element = element.tooltip_show_delay(delay);
        if tooltip.hoverable {
            element = element.hoverable_tooltip(move |_, cx| {
                let content = content.clone();
                cx.new(|_| ViewTree::new(*content)).into()
            });
        } else {
            element = element.tooltip(move |_, cx| {
                let content = content.clone();
                cx.new(|_| ViewTree::new(*content)).into()
            });
        }
    }
    element
}

fn apply_mouse(
    mut element: Stateful<Div>,
    interactivity: &wire::Interactivity,
    cx: &mut Context<ViewTree>,
) -> Stateful<Div> {
    if let Some(handler) = interactivity.on_mouse_down {
        element = element.on_any_mouse_down(cx.listener(move |_, event: &MouseDownEvent, _, cx| {
            cx.emit(wire::Event::MouseDown {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.capture_mouse_down {
        element = element.capture_any_mouse_down(cx.listener(
            move |_, event: &MouseDownEvent, _, cx| {
                cx.emit(wire::Event::MouseDown {
                    handler,
                    phase: wire::DispatchPhase::Capture,
                    event: event.into(),
                });
            },
        ));
    }
    if let Some(handler) = interactivity.on_mouse_down_out {
        element = element.on_mouse_down_out(cx.listener(move |_, event: &MouseDownEvent, _, cx| {
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
        element = element.capture_any_mouse_up(cx.listener(move |_, event: &MouseUpEvent, _, cx| {
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
        element = element.on_mouse_pressure(cx.listener(
            move |_, event: &MousePressureEvent, _, cx| {
                cx.emit(wire::Event::MousePressure {
                    handler,
                    phase: wire::DispatchPhase::Bubble,
                    event: event.into(),
                });
            },
        ));
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
        element = element.on_scroll_wheel(cx.listener(
            move |_, event: &ScrollWheelEvent, _, cx| {
                cx.emit(wire::Event::ScrollWheel {
                    handler,
                    phase: wire::DispatchPhase::Bubble,
                    event: event.into(),
                });
            },
        ));
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

fn apply_keyboard(
    mut element: Stateful<Div>,
    interactivity: &wire::Interactivity,
    cx: &mut Context<ViewTree>,
) -> Stateful<Div> {
    if let Some(handler) = interactivity.on_key_down {
        element = element.on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
            cx.emit(wire::Event::KeyDown {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.capture_key_down {
        element = element.capture_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
            cx.emit(wire::Event::KeyDown {
                handler,
                phase: wire::DispatchPhase::Capture,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.on_key_up {
        element = element.on_key_up(cx.listener(move |_, event: &KeyUpEvent, _, cx| {
            cx.emit(wire::Event::KeyUp {
                handler,
                phase: wire::DispatchPhase::Bubble,
                event: event.into(),
            });
        }));
    }
    if let Some(handler) = interactivity.capture_key_up {
        element = element.capture_key_up(cx.listener(move |_, event: &KeyUpEvent, _, cx| {
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
    element
}

fn apply_misc(
    mut element: Stateful<Div>,
    interactivity: &wire::Interactivity,
    cx: &mut Context<ViewTree>,
) -> Stateful<Div> {
    let tooltip_request = interactivity.tooltip.as_ref().map(|tooltip| tooltip.request);
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
                cx.emit(wire::Event::TooltipRequest { request });
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
    if let Some(handler) = interactivity.on_aux_click {
        element = element.on_aux_click(cx.listener(
            move |this, event: &gpui_kit::ClickEvent, _, cx| {
                this.user_activation.set(Some(handler));
                cx.emit(wire::Event::AuxClick {
                    handler,
                    event: event.into(),
                });
            },
        ));
    }
    element
}
