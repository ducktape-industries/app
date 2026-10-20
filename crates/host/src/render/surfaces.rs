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
        let style = self.style(*style);
        let path = self.authored_path.clone();
        let mut element = div().id(native_id(id)).relative().size_full();
        if let Some(base) = children.first() {
            element = element.child(self.node(base, window, cx));
        }
        if let Some(modal) = children.get(1) {
            let named = named_overlay(label, children);
            let nested = has_named_overlay(modal);
            let shown = match modal {
                wire::Node::Deferred { content, .. } => &**content,
                modal => modal,
            };
            let anchored = matches!(shown, wire::Node::Anchored { .. });
            let shade = shades(anchored, &style).then(|| {
                div()
                    .id(host_id("backdrop"))
                    .absolute()
                    .inset_0()
                    .bg(rgb(0x000000))
                    .opacity(0.5)
            });
            let mut layer = div()
                .id(host_id("layer"))
                .absolute()
                .inset_0()
                .refine_style(&style)
                .flex()
                .occlude();
            if let Some(message) = on_dismiss {
                let message = *message;
                layer = layer.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.activate();
                        cx.emit(wire::Event::Message(message))
                    }),
                );
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
            let layer = self.dialog(
                path,
                named,
                nested,
                layer.child(content),
                *on_dismiss,
                node,
                window,
                cx,
            );
            element = element.children(shade).child(layer);
        }
        element.into_any_element()
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one dialog for the overlay and the sheet: each passes its own layer and node"
    )]
    pub(super) fn dialog(
        &mut self,
        path: AuthoredPath,
        named: bool,
        nested: bool,
        layer: Stateful<Div>,
        dismiss: Option<u32>,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if named && nested {
            self.dialogs.remove(&path);
        }
        let opened = named && !self.dialogs.contains_key(&path);
        let entry = (named && !nested).then(|| {
            self.dialogs
                .entry(path)
                .or_insert_with(|| {
                    let opener = window.focused(cx).map(|focus| focus.downgrade());
                    (cx.focus_handle(), opener)
                })
                .0
                .clone()
        });
        if let Some(entry) = entry.as_ref().filter(|_| opened && self.keys_grant) {
            dialog_entry(entry, window, cx);
        }
        if !named {
            return escape(layer, dismiss, cx).into_any_element();
        }
        match (nested, entry.as_ref()) {
            (false, Some(entry)) => escape(div().absolute().inset_0(), dismiss, cx)
                .focus_trap(host_id("focus-trap-container"), entry)
                .child(crate::a11y::modal(announce(layer, accessible(node))))
                .into_any_element(),
            _ => crate::a11y::modal(announce(escape(layer, dismiss, cx), accessible(node)))
                .into_any_element(),
        }
    }
}

fn escape<E: gpui_kit::InteractiveElement>(
    element: E,
    dismiss: Option<u32>,
    cx: &mut Context<ViewTree>,
) -> E {
    match dismiss {
        Some(message) => element.on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                cx.emit(wire::Event::Message(message));
            }
        })),
        None => element,
    }
}

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
