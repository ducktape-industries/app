use super::*;

pub(super) struct SensorState {
    pub(super) reset: Option<wire::SurfaceValue>,
    pub(super) size: Option<Size<Pixels>>,
    pub(super) on_hide: Option<u32>,
    pub(super) on_show: Option<u32>,
    pub(super) on_resize: Option<u32>,
    pub(super) pending: Option<(Size<Pixels>, Task<()>)>,
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
        let element = div()
            .refine_style(style)
            .id(id.to_gpui().expect("sanitized resize identity"))
            .relative()
            .cursor(native_cursor(*cursor))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.drags.insert(press_key.clone(), event.position);
                    if let Some(message) = press {
                        cx.emit(wire::Event::Message(message));
                    }
                }),
            )
            .child(self.node(content, window, cx))
            .child(capture);
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
            reset,
            on_show,
            on_resize,
            on_hide,
            anticipate,
            delay,
            child,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        let sensor = self.sensors.entry(path.clone()).or_insert(SensorState {
            reset: reset.clone(),
            size: None,
            on_hide: *on_hide,
            on_show: *on_show,
            on_resize: *on_resize,
            pending: None,
        });
        if sensor.reset != *reset {
            sensor.reset = reset.clone();
            sensor.size = None;
            sensor.pending = None;
        }
        sensor.on_hide = *on_hide;
        sensor.on_show = *on_show;
        sensor.on_resize = *on_resize;
        let route = path;
        let show = *on_show;
        let resize = *on_resize;
        let anticipate = px(anticipate.unwrap_or_default());
        let delay = std::time::Duration::from_secs_f32(delay.unwrap_or_default().max(0.0) / 1000.0);
        let weak = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, window, cx| {
                let viewport = window.content_mask().bounds;
                let visible_bounds = Bounds::new(
                    bounds.origin - point(anticipate, anticipate),
                    bounds.size + size(anticipate * 2.0, anticipate * 2.0),
                );
                let visible = viewport.intersects(&visible_bounds);
                let _ = weak.update(cx, |this, cx| {
                    this.bounds.insert(route.clone(), bounds);
                    let Some(sensor) = this.sensors.get_mut(&route) else {
                        return;
                    };
                    if !visible {
                        sensor.pending = None;
                        if sensor.size.take().is_some()
                            && let Some(message) = sensor.on_hide
                        {
                            cx.emit(wire::Event::Message(message));
                        }
                        return;
                    }
                    let unchanged = sensor.size == Some(bounds.size);
                    if unchanged {
                        sensor.pending = None;
                        return;
                    }
                    if !delay.is_zero() {
                        let waiting = sensor
                            .pending
                            .as_ref()
                            .is_some_and(|(size, _)| *size == bounds.size);
                        if waiting {
                            return;
                        }
                        let route = route.clone();
                        let size = bounds.size;
                        let timer = cx.background_executor().timer(delay);
                        let pending = cx.spawn(async move |this, cx| {
                            timer.await;
                            let _ = this.update(cx, |this, cx| {
                                let Some(sensor) = this.sensors.get_mut(&route) else {
                                    return;
                                };
                                let current = sensor
                                    .pending
                                    .as_ref()
                                    .is_some_and(|(pending, _)| *pending == size);
                                if !current {
                                    return;
                                }
                                let handler = match sensor.size {
                                    None => sensor.on_show,
                                    Some(_) => sensor.on_resize,
                                };
                                sensor.size = Some(size);
                                sensor.pending = None;
                                if let Some(handler) = handler {
                                    cx.emit(wire::Event::Size {
                                        handler,
                                        width: f32::from(size.width),
                                        height: f32::from(size.height),
                                    });
                                }
                            });
                        });
                        sensor.pending = Some((size, pending));
                        return;
                    }
                    let handler = match sensor.size {
                        None => show,
                        Some(previous) if previous != bounds.size => resize,
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
            .id(id.to_gpui().expect("sanitized sensor identity"))
            .relative()
            .refine_style(style)
            .child(self.node(child, window, cx))
            .child(measure)
            .into_any_element()
    }

    pub(super) fn mouse_area(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::MouseArea {
            id,
            content,
            on_press,
            on_release,
            on_double_click,
            on_right_press,
            on_right_release,
            on_middle_press,
            on_middle_release,
            on_enter,
            on_exit,
            on_move,
            on_press_at,
            on_scroll,
            role,
            ..
        } = node
        else {
            unreachable!()
        };
        // A mouse area is layout-transparent, like a sensor: a fill-sized
        // child must not collapse inside an auto-sized wrapper.
        let path = self.authored_path.clone();
        let mut element = div()
            .id(id.to_gpui().expect("sanitized mouse-area identity"))
            .relative();
        for (button, down, up) in [
            (MouseButton::Left, *on_press, *on_release),
            (MouseButton::Right, *on_right_press, *on_right_release),
            (MouseButton::Middle, *on_middle_press, *on_middle_release),
        ] {
            if let Some(message) = down {
                element = element.on_mouse_down(
                    button,
                    cx.listener(move |_, _, _, cx| cx.emit(wire::Event::Message(message))),
                );
            }
            if let Some(message) = up {
                element = element.on_mouse_up(
                    button,
                    cx.listener(move |_, _, _, cx| cx.emit(wire::Event::Message(message))),
                );
            }
        }
        if let Some(message) = on_double_click {
            let message = *message;
            element = element.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                    if event.click_count == 2 {
                        cx.emit(wire::Event::Message(message));
                    }
                }),
            );
        }
        let enter = *on_enter;
        let exit = *on_exit;
        element = element.on_hover(cx.listener(move |_, hovered, _, cx| {
            let message = match hovered {
                true => enter,
                false => exit,
            };
            if let Some(message) = message {
                cx.emit(wire::Event::Message(message));
            }
        }));
        if let Some(handler) = on_move {
            let handler = *handler;
            let route = path.clone();
            element =
                element.on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    let origin = this
                        .bounds
                        .get(&route)
                        .map_or(Point::default(), |bounds| bounds.origin);
                    let local = event.position - origin;
                    cx.emit(wire::Event::Pointer {
                        handler,
                        x: f32::from(local.x),
                        y: f32::from(local.y),
                    });
                }));
        }
        if let Some(handler) = on_press_at {
            let handler = *handler;
            let route = path.clone();
            element = element.capture_any_mouse_down(cx.listener(
                move |this, event: &MouseDownEvent, _, cx| {
                    // A right press reports its position too: a context menu
                    // opens where the pointer is.
                    let reported = matches!(event.button, MouseButton::Left | MouseButton::Right);
                    if !reported {
                        return;
                    }
                    let Some(bounds) = this.bounds.get(&route) else {
                        return;
                    };
                    if !bounds.contains(&event.position) {
                        return;
                    }
                    let local = event.position - bounds.origin;
                    cx.emit(wire::Event::Pointer {
                        handler,
                        x: f32::from(local.x),
                        y: f32::from(local.y),
                    });
                },
            ));
        }
        if let Some(handler) = on_scroll {
            let handler = *handler;
            element =
                element.on_scroll_wheel(cx.listener(move |_, event: &ScrollWheelEvent, _, cx| {
                    let (delta, pixels) = match event.delta {
                        ScrollDelta::Pixels(delta) => {
                            (point(f32::from(delta.x), f32::from(delta.y)), true)
                        }
                        ScrollDelta::Lines(delta) => (delta, false),
                    };
                    cx.emit(wire::Event::Scroll {
                        handler,
                        dx: delta.x,
                        dy: delta.y,
                        pixels,
                    });
                }));
        }
        // An area with no role is plumbing assistive technology skips. One
        // with a role is a control: Tab reaches it, and Enter, Space or a
        // screen reader's press is the pointer's press and release. A
        // pointer's own click is already both.
        if role.is_some() {
            let (press, release) = (*on_press, *on_release);
            if press.is_some() || release.is_some() {
                element = crate::a11y::keyboard(element).on_click(cx.listener(
                    move |_, event: &gpui_kit::ClickEvent, _, cx| {
                        if !event.is_keyboard() {
                            return;
                        }
                        for message in [press, release].into_iter().flatten() {
                            cx.emit(wire::Event::Message(message));
                        }
                    },
                ));
            }
            element = announce(element, accessible(node));
        }
        element
            .child(self.node(content, window, cx))
            .child(self.measure(&path, cx))
            .into_any_element()
    }
}
