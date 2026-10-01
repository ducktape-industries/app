//! The Overlay node: a base plus an optional modal layer (dialog, popover,
//! backdrop, focus trap).
use super::*;
use crate::render::native_id;

impl ViewTree {
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
        let mut element = div().id(native_id(id)).relative().size_full();
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
            let anchored = matches!(modal, wire::Node::Anchored { .. });
            let shade = shades(anchored, style).then(|| {
                div()
                    .id(host_id("backdrop"))
                    .absolute()
                    .inset_0()
                    .bg(rgb(0x000000))
                    .opacity(0.5)
            });
            let opened = named && !self.dialogs.contains_key(&path);
            let entry = (named && !nested).then(|| {
                self.dialogs
                    .entry(path.clone())
                    .or_insert_with(|| {
                        let opener = window.focused(cx).map(|focus| focus.downgrade());
                        (cx.focus_handle(), opener)
                    })
                    .0
                    .clone()
            });
            let mut layer = div()
                .id(host_id("layer"))
                .absolute()
                .inset_0()
                .refine_style(style)
                .flex();
            if let Some(message) = on_dismiss {
                let message = *message;
                layer = layer
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // the press outside is a press: a gesture
                            this.activate();
                            cx.emit(wire::Event::Message(message))
                        }),
                    )
                    .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                        // a dismissal is no gesture, as Escape is not on the web
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            cx.emit(wire::Event::Message(message));
                        }
                    }));
            }
            // a dialog takes the keyboard when it opens, if its guest may
            // move the keys (`Seat::keys_free`); a popup's view moves
            // focus itself (widget commands)
            if let Some(entry) = entry.as_ref().filter(|_| opened && self.keys_grant) {
                dialog_entry(entry, window, cx);
            }
            let content = div()
                .bg(gpui_kit::component::Theme::global(cx)
                    .color_tokens()
                    .surface)
                .text_color(
                    gpui_kit::component::Theme::global(cx)
                        .color_tokens()
                        .surface_foreground,
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(self.node(modal, window, cx));
            layer = layer.child(content);
            let layer = if named {
                let layer = crate::a11y::modal(announce(layer, accessible(node)));
                match (nested, entry.as_ref()) {
                    (false, Some(entry)) => div()
                        .absolute()
                        .inset_0()
                        .focus_trap(host_id("focus-trap-container"), entry)
                        .child(layer)
                        .into_any_element(),
                    _ => layer.into_any_element(),
                }
            } else {
                layer.into_any_element()
            };
            element = element.children(shade).child(layer);
        }
        element.into_any_element()
    }
}

/// Whether the host dims what an overlay covers. A popover anchored to the
/// view (an `Anchored`) covers nothing, and an overlay whose view asked for a
/// backdrop (its style's background, drawn on the layer) has one already;
/// only a bare dialog gets the host's shade.
pub(super) fn shades(anchored: bool, style: &gpui_kit::StyleRefinement) -> bool {
    let asked = style
        .background
        .as_ref()
        .and_then(|fill| fill.color())
        .and_then(|background| background.as_solid())
        .is_some_and(|color| color.a > 0.);
    !anchored && !asked
}

#[cfg(test)]
mod shade_tests {
    use gpui_kit::{Styled as _, hsla};

    /// A popover anchored to the view is not shaded; a bare dialog is; a
    /// view's own backdrop replaces the host's shade.
    #[test]
    fn only_a_bare_dialog_is_shaded() {
        let bare = gpui_kit::StyleRefinement::default();
        let clear = gpui_kit::StyleRefinement::default().bg(hsla(0., 0., 0., 0.));
        let dim = gpui_kit::StyleRefinement::default().bg(hsla(0., 0., 0., 0.55));
        assert!(!super::shades(true, &bare));
        assert!(!super::shades(true, &clear));
        assert!(super::shades(false, &bare));
        assert!(super::shades(false, &clear));
        assert!(!super::shades(false, &dim));
    }
}
