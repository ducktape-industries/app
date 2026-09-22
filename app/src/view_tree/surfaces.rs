use super::*;

impl ViewTree {
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
            style,
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
            .refine_style(style)
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
            key: _,
            content,
            x,
            y,
            scale: _,
            style,
        } = node
        else {
            unreachable!()
        };
        let mut element = div()
            .absolute()
            .bg(gpui_kit::component::Theme::global(cx)
                .color_tokens()
                .surface)
            .text_color(
                gpui_kit::component::Theme::global(cx)
                    .color_tokens()
                    .surface_foreground,
            )
            .refine_style(style)
            .left(px(*x))
            .top(px(*y));
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
            .child(self.measure(&self.authored_path, cx))
            .into_any_element()
    }

    pub(super) fn overlay(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Overlay {
            id,
            label,
            children,
            style,
            on_dismiss,
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        let mut element = div()
            .id(id.to_gpui().expect("sanitized overlay identity"))
            .relative()
            .size_full();
        if let Some(base) = children.first() {
            element = element.child(self.node(base, window, cx));
        }
        if let Some(modal) = children.get(1) {
            let named = named_overlay(label, children);
            let nested = has_named_overlay(modal);
            if named && nested {
                // The focus-trap registry keeps weak handles until another
                // trap registers; drop an obscured ancestor before that pass.
                self.dialogs.remove(&path);
            }
            let shade = div()
                .id("backdrop")
                .absolute()
                .inset_0()
                .bg(rgb(0x000000))
                .opacity(0.5);
            let opened = named && !self.dialogs.contains_key(&path);
            let entry = (named && !nested).then(|| {
                self.dialogs
                    .entry(path.clone())
                    .or_insert_with(|| cx.focus_handle())
                    .clone()
            });
            let is_float = matches!(modal, wire::Node::Float { .. });
            let mut layer_style = style.clone();
            if is_float {
                layer_style.padding = Default::default();
            }
            let mut layer = div()
                .id("layer")
                .absolute()
                .inset_0()
                .refine_style(&layer_style);
            if !is_float {
                layer = layer.flex();
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
                        .focus_trap("focus-trap-container", entry)
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
