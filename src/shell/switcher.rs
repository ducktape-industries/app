//! The rail's network switcher: the network's name at the rail's top is a
//! button, and it opens the networks this device has reached — the old
//! app's "Switch network" block (before #216), without its detour through
//! the Connect screen and a second password prompt for the network already
//! in hand.

use super::*;
use screens::Facts;

impl DesktopWindow {
    /// The network in hand, how it is doing, and the way to another: a
    /// live dot, the name and the status line ("Reaching …" while a switch
    /// is in flight). Compact, it keeps the dot and the name's first letter.
    pub(super) fn network_switcher(
        &self,
        state: &Facts,
        narrow: bool,
        dot: gpui_kit::Hsla,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let ink_fg = hsla_of(palette.sidebar_foreground);
        let ink_muted = hsla_of(palette.sidebar_muted);
        let ink_raised = hsla_of(palette.sidebar_raised);
        let model = self.model.clone();
        let live = div()
            .id("rail-connection")
            .control(
                Role::Status,
                SharedString::from(
                    match (state.connecting, state.connected, state.reconnecting) {
                        (true, _, _) => "Switching",
                        (_, true, false) => "Connected",
                        (_, true, true) => "Reconnecting",
                        (_, false, _) => "Not connected",
                    },
                ),
            )
            .size(px(6.))
            .flex_shrink_0()
            .rounded_full()
            .bg(dot);
        crate::a11y::keyboard(
            div()
                .id("network-switcher")
                .control(
                    Role::Button,
                    SharedString::from(format!("Network: {}", state.network)),
                )
                .aria_expanded(state.network_menu)
                .flex()
                .items_center()
                .gap_2()
                .when(narrow, |button| button.flex_col().gap_1())
                .px_2()
                .py_1p5()
                .rounded(px(design::radius::CONTROL as f32))
                .cursor_pointer()
                .when(state.network_menu, |button| button.bg(ink_raised))
                .hover(move |style| style.bg(ink_raised))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    model.update(cx, |model, cx| {
                        model.dispatch(Message::ToggleNetworkMenu, cx)
                    });
                })
                .map(|button| match narrow {
                    true => button
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ink_fg)
                                .child(
                                    state
                                        .network
                                        .chars()
                                        .next()
                                        .map(|first| first.to_uppercase().to_string())
                                        .unwrap_or_default(),
                                ),
                        )
                        .child(live),
                    false => button
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ink_fg)
                                        .truncate()
                                        .child(state.network.clone()),
                                )
                                .child(
                                    div().flex().items_center().gap_1p5().child(live).child(
                                        div()
                                            .id("rail-status")
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(ink_muted)
                                            .truncate()
                                            .child(state.status.clone()),
                                    ),
                                ),
                        )
                        .child(
                            gpui_kit::component::Icon::new(
                                gpui_kit::assets::IconName::ChevronsUpDown,
                            )
                            .size(px(14.))
                            .text_color(ink_muted),
                        ),
                }),
        )
    }

    /// The switcher's menu: each node this device reached (its network, its
    /// host, a check on the one in hand, and which one is a different chain
    /// sharing a name), then "Add a network…" for the Connect screen. A
    /// click anywhere outside, or Escape, closes it.
    pub(super) fn network_menu(
        &self,
        state: &Facts,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::assets::IconName;
        use gpui_kit::*;
        let theme = gpui_kit::component::Theme::global(cx);
        let colors = theme.color_tokens();
        let (popover, accent_bg) = (theme.popover, theme.accent);
        let item = |id: SharedString, role: Role, name: String, message: Message| {
            let model = self.model.clone();
            let message = std::cell::Cell::new(Some(message));
            crate::a11y::keyboard(
                div()
                    .id(id)
                    .control(role, SharedString::from(name))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(px(design::radius::CONTROL as f32))
                    .cursor_pointer()
                    .hover(move |style| style.bg(accent_bg))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        // a click after this one finds the menu closed
                        if let Some(message) = message.take() {
                            model.update(cx, |model, cx| model.dispatch(message, cx));
                        }
                    }),
            )
        };
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
            .child(div().size(px(14.)).flex_shrink_0().when(current, |mark| {
                mark.child(gpui_kit::component::Icon::new(IconName::Check).size(px(14.)))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(match current {
                                true => FontWeight::MEDIUM,
                                false => FontWeight::NORMAL,
                            })
                            .truncate()
                            .child(network),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .truncate()
                            .child(entry.host().to_owned()),
                    ),
            )
            .when(entry.other_chain, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(colors.muted_foreground)
                        .child("different network"),
                )
            })
        });
        let add = item(
            "network-menu/add".into(),
            Role::MenuItem,
            "Add a network…".into(),
            Message::Disconnect,
        )
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .child(gpui_kit::component::Icon::new(IconName::Plus).size(px(14.))),
        )
        .child(div().text_size(px(12.5)).child("Add a network…"));
        let close = self.model.clone();
        let escape = self.model.clone();
        let menu = div()
            .id("network-menu")
            .control(Role::Menu, "Networks")
            .occlude()
            .absolute()
            .map(|menu| match narrow {
                // beside the compact rail, not over it
                true => menu.top(px(8.)).left(px(RAIL_COMPACT_WIDTH + 4.)),
                false => menu.top(px(58.)).left(px(8.)),
            })
            .w(px(300.))
            .max_w(px(RAIL_WIDTH + 100.))
            .p_1()
            .flex()
            .flex_col()
            .rounded(px(design::radius::CARD as f32))
            .border_1()
            .border_color(colors.border)
            .bg(popover)
            .shadow_md()
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
                div()
                    .px_2()
                    .pt_1()
                    .pb_0p5()
                    .text_size(px(11.))
                    .text_color(colors.muted_foreground)
                    .child("Networks"),
            )
            .children(rows)
            .child(div().my_1().h(px(1.)).bg(colors.border))
            .child(add);
        div()
            .id("network-menu-backdrop")
            .absolute()
            .inset_0()
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
