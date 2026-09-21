use super::*;

impl ViewTree {
    pub(super) fn hover(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Hover {
            key,
            children,
            open,
            width,
            height,
            padding,
            background,
            border,
            tint,
            radius,
        } = node
        else {
            unreachable!()
        };
        let route = key.clone();
        let mut element = decoration(
            pad(dimensions(div().relative(), *width, *height), *padding),
            *background,
            *border,
        )
        .id(key.clone());
        /// How far above the row's top edge the float's box starts: half a
        /// message bar, so the bar straddles the edge.
        const HOVER_FLOAT_LIFT: f32 = 14.;
        let reveal = *open || self.hovered.contains(key);
        // The tint is the row's own ground while the pointer is on it — it
        // sits UNDER the content, never over the text.
        if reveal && let Some(color) = tint {
            element = element.bg(rgba(*color)).rounded(px(*radius));
        }
        if let Some(base) = children.first() {
            element = element.child(self.node(base, window, cx));
        }
        if reveal && let Some(child) = children.get(1) {
            // The float straddles the row's top edge, half above it, the way
            // a message bar does; nothing here clips, so it draws over the
            // row above.
            let layer = div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(-HOVER_FLOAT_LIFT))
                .bottom_0();
            element = element.child(layer.child(self.node(child, window, cx)));
        }
        let element = element.on_hover(cx.listener(move |this, hovered, _, cx| {
            match hovered {
                true => {
                    this.hovered.insert(route.clone());
                }
                false => {
                    this.hovered.remove(&route);
                }
            }
            cx.notify();
        }));
        #[cfg(test)]
        let element = {
            use gpui_kit::test::TestSupportExt as _;
            element.test_support()
        };
        element.into_any_element()
    }

    pub(super) fn tooltip(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Tooltip {
            key,
            children,
            delay_ms,
            ..
        } = node
        else {
            unreachable!()
        };
        let Some(content) = children.first() else {
            return div().into_any_element();
        };
        let mut element = div()
            .id(key.clone())
            .tooltip_show_delay(std::time::Duration::from_millis(*delay_ms))
            .child(self.node(content, window, cx));
        if let Some(tip) = children.get(1) {
            let tip = tip.clone();
            element = element.tooltip(move |_, cx| cx.new(|_| ViewTree::new(tip.clone())).into());
        }
        element.into_any_element()
    }

    pub(super) fn float(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Float {
            key,
            content,
            x,
            y,
            scale: _,
            shadow,
            radius,
        } = node
        else {
            unreachable!()
        };
        let mut element = shadows(
            div()
                .absolute()
                .bg(gpui_kit::component::Theme::global(cx)
                    .color_tokens()
                    .surface)
                .text_color(
                    gpui_kit::component::Theme::global(cx)
                        .color_tokens()
                        .surface_foreground,
                )
                .left(px(*x))
                .top(px(*y)),
            *shadow,
        );
        if let Some(radius) = radius {
            element = decoration(
                element,
                None,
                Some(wire::Border {
                    color: None,
                    width: None,
                    radius: Some(*radius),
                }),
            );
        }
        // A press inside a floated card is the card's: it never
        // reaches what the card floats over (a dismissing backdrop,
        // the document under a comment card).
        element = element
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        // Authored floating rails use unit scale; their measurement is
        // outside the translated child to avoid positional feedback.
        div()
            .relative()
            .child(element.child(self.node(content, window, cx)))
            .child(self.measure(key, cx))
            .into_any_element()
    }

    pub(super) fn overlay(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Overlay {
            key,
            label,
            children,
            backdrop,
            padding,
            align_x,
            align_y,
            on_dismiss,
        } = node
        else {
            unreachable!()
        };
        let mut element = div().relative().size_full();
        if let Some(base) = children.first() {
            element = element.child(self.node(base, window, cx));
        }
        if let Some(modal) = children.get(1) {
            let named = named_overlay(label, children);
            let nested = has_named_overlay(modal);
            if named && nested {
                // The focus-trap registry keeps weak handles until another
                // trap registers; drop an obscured ancestor before that pass.
                self.dialogs.remove(key);
            }
            let shade = div()
                .id(format!("{key}/backdrop"))
                .absolute()
                .inset_0()
                .bg(rgba(*backdrop));
            let opened = named && !self.dialogs.contains_key(key);
            let entry = (named && !nested).then(|| {
                self.dialogs
                    .entry(key.clone())
                    .or_insert_with(|| cx.focus_handle())
                    .clone()
            });
            let is_float = matches!(modal, wire::Node::Float { .. });
            let mut layer = div().id(format!("{key}/layer")).absolute().inset_0();
            if !is_float {
                layer = layer.flex().p(px(*padding));
            }
            if let Some(message) = on_dismiss {
                let message = *message;
                layer = layer
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _, _, cx| cx.emit(wire::Event::Message(message))),
                    )
                    .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            cx.emit(wire::Event::Message(message));
                        }
                    }));
            }
            if !is_float {
                layer = match align_x {
                    wire::AlignX::Left => layer.justify_start(),
                    wire::AlignX::Center => layer.justify_center(),
                    wire::AlignX::Right => layer.justify_end(),
                };
                layer = match align_y {
                    wire::AlignY::Top => layer.items_start(),
                    wire::AlignY::Center => layer.items_center(),
                    wire::AlignY::Bottom => layer.items_end(),
                };
            }
            let content = if is_float {
                self.node(modal, window, cx)
            } else {
                div()
                    .bg(gpui_kit::component::Theme::global(cx)
                        .color_tokens()
                        .surface)
                    .text_color(
                        gpui_kit::component::Theme::global(cx)
                            .color_tokens()
                            .surface_foreground,
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // a dialog takes the keyboard when it opens; a
                    // popup's view moves focus itself (widget commands)
                    .children(
                        entry
                            .as_ref()
                            .map(|entry| dialog_entry(entry, opened, window, cx)),
                    )
                    .child(self.node(modal, window, cx))
                    .into_any_element()
            };
            if is_float {
                layer = layer.children(
                    entry
                        .as_ref()
                        .map(|entry| dialog_entry(entry, opened, window, cx)),
                );
            }
            layer = layer.child(content);
            let layer = if named {
                let layer = crate::a11y::modal(announce(layer, accessible(node)));
                match (nested, entry.as_ref()) {
                    (false, Some(entry)) => div()
                        .absolute()
                        .inset_0()
                        .focus_trap(format!("{key}/focus-trap-container"), entry)
                        .child(layer)
                        .into_any_element(),
                    _ => layer.into_any_element(),
                }
            } else {
                layer.into_any_element()
            };
            element = element.child(shade).child(layer);
        }
        element.into_any_element()
    }
}
