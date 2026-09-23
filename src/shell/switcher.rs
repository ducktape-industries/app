//! The network menu: the network's name at the menu bar's left is a
//! button, and it opens the networks this device has reached — the old
//! app's "Switch network" block (before #216), without its detour through
//! the Connect screen and a second password prompt for the network already
//! in hand.

use super::*;
use screens::Facts;

impl DesktopWindow {
    /// The switcher's menu: each node this device reached (its network, its
    /// host, a check on the one in hand, and which one is a different chain
    /// sharing a name), then "Add a network" for the Connect screen. A
    /// click anywhere outside, or Escape, closes it.
    pub(super) fn network_menu(&self, state: &Facts, narrow: bool) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let surface = ink.surface;
        let item = |id: SharedString, role: Role, name: String, message: Message| {
            let model = self.model.clone();
            let message = std::cell::Cell::new(Some(message));
            crate::a11y::keyboard(
                div()
                    .id(id)
                    .control(role, SharedString::from(name))
                    .cursor_pointer()
                    .hover(move |style| style.bg(surface))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        // a click after this one finds the menu closed
                        if let Some(message) = message.take() {
                            model.update(cx, |model, cx| model.dispatch(message, cx));
                        }
                    }),
            )
        };
        // The NetworkSwitcher board: `padding: 12px 16px; gap: 12px`, a
        // hairline above each row; the name `500 15px` in a `110px` column,
        // the host in mono.
        let rows = state.recent_endpoints.iter().map(|entry| {
            let current = entry.url == state.connected_rpc;
            let name = match current {
                true => format!("{} (current)", entry.label()),
                false => entry.label(),
            };
            let network = match entry.network.is_empty() {
                true => entry.host().to_owned(),
                false => entry.network.clone(),
            };
            item(
                SharedString::from(format!("network-menu/{}", entry.url)),
                Role::MenuItemRadio,
                name,
                Message::SwitchNetwork(entry.url.clone()),
            )
            .aria_toggled(match current {
                true => gpui_kit::accesskit::Toggled::True,
                false => gpui_kit::accesskit::Toggled::False,
            })
            .flex()
            .items_baseline()
            .gap(px(12.))
            .px(px(16.))
            .py(px(12.))
            .border_t_1()
            .border_color(ink.line)
            .child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(sans(500, 15.).text_color(ink.ink).truncate().child(network))
                    .when(entry.other_chain, |column| {
                        column.child(sans(400, 12.).text_color(ink.muted).child("Other chain"))
                    }),
            )
            .child(
                mono(400, 13.)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(ink.muted)
                    .child(entry.host().to_owned()),
            )
            .child(
                mono(400, 12.)
                    .text_color(ink.muted)
                    .child(if current { "Current" } else { "" }),
            )
        });
        let add = item(
            "network-menu/add".into(),
            Role::MenuItem,
            "Add a network".into(),
            Message::Disconnect,
        )
        .child(
            sans(500, 14.)
                .h(px(36.))
                .px(px(18.))
                .flex()
                .items_center()
                .border(px(1.5))
                .border_color(ink.ink)
                .text_color(ink.ink)
                .child("Add a network"),
        );
        let count = state.recent_endpoints.len();
        let close = self.model.clone();
        let escape = self.model.clone();
        let menu = div()
            .id("network-menu")
            .control(Role::Menu, "Networks")
            .occlude()
            .absolute()
            .top(px(4.))
            .left(px(if cfg!(target_os = "macos") { 78. } else { 8. }))
            .w(px(if narrow { 300. } else { 400. }))
            .flex()
            .flex_col()
            .border(px(1.5))
            .border_color(ink.ink)
            .bg(ink.bg)
            .shadow_lg()
            // a click on the menu's own padding is not a click outside it
            .on_click(|_, _, cx| cx.stop_propagation())
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    escape.update(cx, |model, cx| {
                        model.dispatch(Message::CloseNetworkMenu, cx)
                    });
                }
            })
            .child(
                mono(400, 12.)
                    .px(px(16.))
                    .py(px(12.))
                    .text_color(ink.muted)
                    .child(format!("Networks · {count}")),
            )
            .children(rows)
            .child(
                div()
                    .flex()
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(ink.line)
                    .child(add),
            );
        div()
            .id("network-menu-backdrop")
            .absolute()
            .top(px(36.))
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .on_click(move |_, _, cx| {
                close.update(cx, |model, cx| {
                    model.dispatch(Message::CloseNetworkMenu, cx)
                });
            })
            .child(menu)
            .into_any_element()
    }
}
