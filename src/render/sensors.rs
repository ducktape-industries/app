//! The pointer-shaped nodes: ResizeHandle (a divider with a `grip` and a
//! drag) and Sensor (reports its child's size when shown or resized).
use super::*;
use crate::render::native_id;

/// A Sensor's retained state, per authored path.
pub(super) struct SensorState {
    /// The child's size as last seen in view; `None` out of view, so the
    /// next sight of it is a show again.
    pub(super) size: Option<Size<Pixels>>,
    pub(super) on_show: Option<u32>,
    pub(super) on_resize: Option<u32>,
}

/// The invisible hit area of a ResizeHandle: its own box widened by
/// [`crate::render::GRAB`] px on both sides of the axis it resizes (both axes
/// for other cursors), the same reach desk window borders have, so a press
/// just beside a 1px divider still starts a resize. Absolutely positioned
/// over the neighbouring panes and occluding them.
pub(super) fn grip(cursor: CursorStyle) -> gpui_kit::Stateful<Div> {
    let (x, y) = grip_reach(cursor);
    div()
        .id(host_id("grip"))
        .absolute()
        .left(px(-x))
        .right(px(-x))
        .top(px(-y))
        .bottom(px(-y))
        .cursor(cursor)
        .occlude()
}

/// How far a grip reaches past its handle, across and along.
pub(super) fn grip_reach(cursor: CursorStyle) -> (f32, f32) {
    let grab = crate::render::GRAB;
    match cursor {
        CursorStyle::ResizeLeftRight
        | CursorStyle::ResizeColumn
        | CursorStyle::ResizeLeft
        | CursorStyle::ResizeRight => (grab, 0.),
        CursorStyle::ResizeUpDown
        | CursorStyle::ResizeRow
        | CursorStyle::ResizeUp
        | CursorStyle::ResizeDown => (0., grab),
        _ => (grab, grab),
    }
}

impl ViewTree {
    pub(super) fn resize_handle(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::ResizeHandle {
            id,
            on_press,
            on_release,
            on_drag,
            content,
            cursor,
            style,
            interactivity,
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        let press_key = path.clone();
        let move_key = path.clone();
        let release_key = path;
        let press = *on_press;
        let release = *on_release;
        let drag = *on_drag;
        let view = cx.entity().downgrade();
        let capture = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moving = view.clone();
                let move_key = move_key.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != gpui_kit::DispatchPhase::Capture {
                        return;
                    }
                    let _ = moving.update(cx, |this, cx| {
                        let Some(previous) = this.drags.get_mut(&move_key) else {
                            return;
                        };
                        if event.pressed_button != Some(MouseButton::Left) {
                            this.drags.remove(&move_key);
                            return;
                        }
                        let delta = event.position - *previous;
                        *previous = event.position;
                        if let Some(handler) = drag {
                            cx.emit(wire::Event::Drag {
                                handler,
                                dx: f32::from(delta.x) as f64,
                                dy: f32::from(delta.y) as f64,
                            });
                        }
                    });
                });
                let releasing = view.clone();
                let release_key = release_key.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != gpui_kit::DispatchPhase::Capture
                        || event.button != MouseButton::Left
                    {
                        return;
                    }
                    let _ = releasing.update(cx, |this, cx| {
                        let was_dragging = this.drags.remove(&release_key).is_some();
                        if was_dragging && let Some(message) = release {
                            cx.emit(wire::Event::Message(message));
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0();
        let cursor = native_cursor(*cursor);
        // capture phase: the grip overlaps the neighbouring panes, so claim
        // the left press here, before a pane underneath turns it into a click
        // or a text selection
        let grip = grip(cursor).capture_any_mouse_down(cx.listener(
            move |this, event: &MouseDownEvent, _, cx| {
                if event.button != MouseButton::Left {
                    return;
                }
                cx.stop_propagation();
                this.drags.insert(press_key.clone(), event.position);
                if let Some(message) = press {
                    cx.emit(wire::Event::Message(message));
                }
            },
        ));
        let element = div()
            .refine_style(&self.styles[*style])
            .id(native_id(id))
            .relative()
            .cursor(cursor)
            .child(self.node(content, window, cx))
            .child(capture)
            .child(grip);
        let interactivity = interactivity
            .as_deref()
            .unwrap_or(wire::Interactivity::none());
        let element = self.guest_aria(element, node, interactivity, cx);
        #[cfg(test)]
        let element = {
            use gpui_kit::test::TestSupportExt as _;
            element.test_support()
        };
        element.into_any_element()
    }

    pub(super) fn sensor(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Sensor {
            id,
            on_show,
            on_resize,
            child,
            style,
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        let sensor = self.sensors.entry(path.clone()).or_insert(SensorState {
            size: None,
            on_show: *on_show,
            on_resize: *on_resize,
        });
        sensor.on_show = *on_show;
        sensor.on_resize = *on_resize;
        let route = path;
        let weak = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, window, cx| {
                let visible = window.content_mask().bounds.intersects(&bounds);
                let _ = weak.update(cx, |this, cx| {
                    this.bounds.insert(route.clone(), bounds);
                    let Some(sensor) = this.sensors.get_mut(&route) else {
                        return;
                    };
                    if !visible {
                        sensor.size = None;
                        return;
                    }
                    let handler = match sensor.size {
                        None => sensor.on_show,
                        Some(previous) if previous != bounds.size => sensor.on_resize,
                        Some(_) => None,
                    };
                    sensor.size = Some(bounds.size);
                    if let Some(handler) = handler {
                        cx.emit(wire::Event::Size {
                            handler,
                            width: f32::from(bounds.size.width),
                            height: f32::from(bounds.size.height),
                        });
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        // A sensor is layout-transparent. In particular, a fill spacer
        // must not collapse inside an auto-sized measurement wrapper.
        div()
            .id(native_id(id))
            .relative()
            .refine_style(&self.style(*style))
            .child(self.node(child, window, cx))
            .child(measure)
            .into_any_element()
    }
}
