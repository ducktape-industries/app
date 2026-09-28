//! The desk with no window on it: a figure, and either why the network
//! has nothing to open or the ways to open something.
use super::*;

/// What an empty desk says when it has nothing to offer: `None` when there
/// is something to open, and the desk shows its buttons instead.
fn empty_panes_message(rail: &[crate::runtime::RailRow]) -> Option<&'static str> {
    (!rail.iter().any(|row| !row.empty)).then_some("This network runs no program with a view.")
}

/// An empty desk's way out: the chord and what it does, and a press does
/// what the chord would.
fn desk_button(
    id: &'static str,
    key: &str,
    name: &'static str,
    ink: &super::ink::Ink,
    action: fn() -> Box<dyn gpui_kit::Action>,
) -> gpui_kit::AnyElement {
    use super::ink::*;
    use gpui_kit::*;
    let hover = ink.surface;
    let button = sans(500, 14.)
        .id(id)
        .control(Role::Button, SharedString::from(name))
        .h(px(tall(34.)))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .border_1()
        .border_color(ink.line)
        .text_color(ink.ink)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            window.dispatch_action(action(), cx);
        })
        .child(mono(400, 12.).text_color(ink.muted).child(chord_label(key)))
        .child(name);
    // its chord, as Help lists it (AX-114)
    crate::a11y::keyboard(button)
        .aria_keyshortcuts(chord_label(key))
        .into_any_element()
}

impl DesktopWindow {
    /// The desk with no window on it: a figure, and either the reason the
    /// network has nothing to open or the ways to open something. The
    /// window itself takes the keys once the last pane leaves.
    pub(super) fn empty_desk(
        &mut self,
        moved: bool,
        ink: &super::ink::Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        if moved {
            self.focus.focus(window, cx);
        }
        let moving = self.model.read(cx).state.motion;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(20.))
            .child(super::spin::drawing(
                "empty-desk-figure",
                super::figure::Figure::Roll,
                moving,
                ink.figure,
                window,
                cx,
            ))
            .child(match empty_panes_message(&crate::runtime::rail()) {
                Some(message) => super::ink::mono(400, 12.)
                    .text_color(ink.muted)
                    .child(super::ink::words("empty-desk/message", message))
                    .into_any_element(),
                None => div()
                    .flex()
                    .gap(px(12.))
                    .child(desk_button(
                        "empty-desk/new",
                        "N",
                        "New window",
                        ink,
                        || Box::new(super::keys::NewWindow),
                    ))
                    .child(desk_button("empty-desk/search", "K", "Search", ink, || {
                        Box::new(super::keys::ToggleSpotlight)
                    }))
                    .child(desk_button("empty-desk/help", "/", "Help", ink, || {
                        Box::new(super::keys::OpenHelp)
                    }))
                    .into_any_element(),
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod empty_panes_message_tests {
    use super::empty_panes_message;
    use crate::runtime::RailRow;

    fn row(empty: bool) -> RailRow {
        RailRow {
            module: "chat",
            label: "Chat".into(),
            note: None,
            empty,
        }
    }

    #[test]
    fn names_the_network_only_when_the_rail_itself_is_empty() {
        assert_eq!(
            empty_panes_message(&[]),
            Some("This network runs no program with a view.")
        );
        assert_eq!(
            empty_panes_message(&[row(true)]),
            Some("This network runs no program with a view."),
            "a rail of empty-slot rows still has nothing to open"
        );
        assert_eq!(empty_panes_message(&[row(true), row(false)]), None);
    }
}
