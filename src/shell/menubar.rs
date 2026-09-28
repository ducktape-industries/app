//! The menu bar: the network, the programs as tabs, then Search (⌘K), the
//! bell, the node's breath, who is signed in, and Settings.

use super::*;
use crate::{Overlay, Popover};
use desk::BAR;
use facts::Facts;
use status_bar::pulse;

impl DesktopWindow {
    /// The menu bar (the Menubar board): `height: 36px; padding: 0 8px;
    /// border-bottom: 1px solid line`; every item `height: 36px; padding: 0
    /// 10px; gap: 8px; font: 400 13px`, on `surface` while its menu is open.
    pub(super) fn menubar(
        &self,
        state: &Facts,
        rail: &[crate::runtime::RailRow],
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let layout = self.layout(cx);
        let open: Vec<&'static str> = layout.panes.iter().map(|pane| pane.module).collect();
        let focused = layout.panes.get(layout.focused).map(|pane| pane.module);
        let tabs = rail.iter().filter(|row| !row.empty).map(|row| {
            let module = row.module;
            let selected = focused == Some(module);
            let badge = state.badges.get(module).copied().unwrap_or(0);
            let shown = tab_label(row);
            let name = match row.note {
                Some(note) => format!("{shown} · {note}"),
                None => shown.clone(),
            };
            // the count the tab shows is in what it is called
            let name = match badge {
                ..=0 => name,
                count => format!("{name}, {count} unread"),
            };
            // folded: the program's icon, its initial when it has none,
            // and its whole name on hover
            let shown: AnyElement = match (narrow, tab_icon(module)) {
                (false, _) => shown.into_any_element(),
                (true, Some(icon)) => gpui_kit::component::Icon::new(icon)
                    .size(px(16.))
                    .into_any_element(),
                (true, None) => shown
                    .chars()
                    .next()
                    .map(String::from)
                    .unwrap_or_default()
                    .into_any_element(),
            };
            let tip = narrow.then(|| SharedString::from(name.clone()));
            let hover = ink.ink;
            crate::a11y::focus_shown(
                sans(400, 13.)
                    .id(SharedString::from(format!("rail/{module}")))
                    .focusable()
                    .tab_stop(true),
            )
            .control(Role::Tab, SharedString::from(name))
            .aria_selected(selected)
            .h(px(BAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .cursor_pointer()
            .text_color(match selected || open.contains(&module) {
                true => ink.ink,
                false => ink.muted,
            })
            .hover(move |style| style.text_color(hover))
            // a click opens it (into an empty focused window, or its
            // own); shift-click shows it in the focused window instead
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                match event.modifiers().shift {
                    true => this.pane_message(PaneMessage::Select(module), window, cx),
                    false => this.open_view(module, window, cx),
                }
            }))
            .child(shown)
            .when_some(tip, |tab, tip| {
                tab.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
            })
            .when(row.note == Some("Failed"), |tab| {
                tab.child(div().size(px(5.)).rounded_full().bg(ink.danger))
            })
            .when(badge > 0, |tab| {
                tab.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .child(badge.to_string()),
                )
            })
        });
        let item = |id: &'static str, name: SharedString, open: bool, message: fn() -> Message| {
            let model = self.model.clone();
            let surface = ink.surface;
            crate::a11y::keyboard(
                sans(400, 13.)
                    .id(id)
                    .control(Role::Button, name)
                    .h(px(BAR))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .cursor_pointer()
                    .text_color(ink.ink)
                    .when(open, |item| item.bg(surface))
                    .hover(move |style| style.bg(surface))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        model.update(cx, |model, cx| model.dispatch(message(), cx));
                    }),
            )
        };
        let network = item(
            "network-switcher",
            SharedString::from(format!("Network: {}", state.network)),
            state.overlay == Some(Overlay::Network),
            || Message::ToggleNetworkMenu,
        )
        .aria_expanded(state.overlay == Some(Overlay::Network))
        .relative()
        .child(self.button_probe(Overlay::Network, cx))
        .child(sans(500, 13.).child(state.network.clone()))
        .child(div().text_color(ink.muted).child("⌄"));
        let chord = chord_label("K");
        let search = item("rail-search", "Search".into(), false, || {
            Message::OpenSpotlight
        })
        .when(!narrow, |item| {
            item.child(div().text_color(ink.muted).child("Search"))
        })
        .child(
            mono(400, 12.)
                .text_color(ink.muted)
                .px(px(5.))
                .py(px(1.))
                .border_1()
                .border_color(ink.line)
                .child(chord),
        );
        let unread = crate::runtime::notify::center().unread();
        let bell_open = state.overlay == Some(Overlay::Menu(Popover::Notifications));
        let bell = item(
            "rail-notifications",
            SharedString::from(match unread {
                0 => "Notifications".to_owned(),
                count => format!("Notifications, {count} unread"),
            }),
            bell_open,
            || Message::TogglePopover(Popover::Notifications),
        )
        .aria_expanded(bell_open)
        .relative()
        .child(self.button_probe(Overlay::Menu(Popover::Notifications), cx))
        .child(
            div()
                .relative()
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Bell)
                        .size(px(16.))
                        .text_color(ink.muted),
                )
                // the count, announced as notices land
                .when(unread > 0, |bell| {
                    bell.child(
                        crate::a11y::live(
                            mono(500, 9.).id("rail-unread").role(Role::Status),
                            accesskit::Live::Polite,
                            format!("{unread} unread notifications"),
                        )
                        .absolute()
                        .top(px(-5.))
                        .left(px(8.))
                        .min_w(px(14.))
                        .h(px(14.))
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(ink.ink)
                        .text_color(ink.bg)
                        .child(match unread {
                            ..=99 => unread.to_string(),
                            _ => "99+".to_owned(),
                        }),
                    )
                }),
        );
        // the height moves every block: it is the description, so the
        // name holds still
        let (breath, said) = match (state.connecting, state.reconnecting) {
            (true, _) => (true, "Node: switching"),
            (_, true) => (false, "Node: not answering"),
            (false, false) => (true, "Node: in sync"),
        };
        let node_open = state.overlay == Some(Overlay::Menu(Popover::Node));
        let node = item("rail-connection", said.into(), node_open, || {
            Message::TogglePopover(Popover::Node)
        })
        .aria_description(format!("Block {}", state.height))
        .aria_expanded(node_open)
        .relative()
        .child(self.button_probe(Overlay::Menu(Popover::Node), cx))
        .px(px(12.))
        .child(pulse(breath, state.motion, &ink));
        let unlocked = !state.signer_key.is_empty();
        let account_open = state.overlay == Some(Overlay::Menu(Popover::Account));
        let who = match (&state.account, unlocked) {
            (_, false) => item("sign-in", "Sign in".into(), false, || Message::SignIn)
                .child(div().underline().child("Sign in")),
            // named by what it says
            (Some(None), true) => item("rail-account", "Create account".into(), false, || {
                Message::ShowCreateAccount
            })
            .child(div().underline().child("Create account")),
            (account, true) => {
                let name = match account {
                    Some(Some((_, name))) => name.clone(),
                    _ => "Signed in".to_owned(),
                };
                let shown = match narrow {
                    true => screens::initials(&name),
                    false => name.clone(),
                };
                item(
                    "rail-account",
                    SharedString::from(format!("Account: {name}")),
                    account_open,
                    || Message::TogglePopover(Popover::Account),
                )
                .aria_expanded(account_open)
                .relative()
                .child(self.button_probe(Overlay::Menu(Popover::Account), cx))
                .child(shown)
            }
        };
        // Settings are the app's, not the account's: their own spot at the
        // edge.
        let gear = item("settings", "Ducktape settings".into(), false, || {
            Message::OpenSettings
        })
        .px(px(8.))
        .child(
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                .size(px(16.))
                .text_color(ink.muted),
        );
        // macOS draws the traffic lights over the bar's left end, and the
        // bar is the window's handle: its empty middle moves the window.
        let titlebar = theme::traffic_lights(window);
        let handle = div()
            .id("menubar-handle")
            .flex_1()
            .min_w(px(8.))
            .h_full()
            .when(titlebar.is_some(), |strip| {
                strip.on_mouse_down(MouseButton::Left, |event, window, _| {
                    match event.click_count {
                        2 => window.titlebar_double_click(),
                        _ => window.start_window_move(),
                    }
                })
            });
        div()
            .id("menubar")
            .role(Role::MenuBar)
            .aria_label("Ducktape")
            .h(px(BAR))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(px(titlebar.unwrap_or(8.)))
            .pr(px(8.))
            .border_b_1()
            .border_color(ink.line)
            .bg(ink.bg)
            .child(network)
            .child(div().w(px(1.)).h(px(16.)).mx(px(6.)).bg(ink.line))
            .child(
                div()
                    .id("rail-rows")
                    .control(Role::TabList, "Programs")
                    .min_w_0()
                    .flex()
                    // folded and still too many: they scroll
                    .overflow_x_scroll()
                    .track_scroll(&self.rail)
                    .children(tabs)
                    .when(rail.is_empty(), |list| {
                        list.child(
                            sans(400, 13.)
                                .px(px(10.))
                                .text_color(ink.muted)
                                .child("No programs listed yet"),
                        )
                    }),
            )
            .child(handle)
            .child(search)
            .child(bell)
            .child(node)
            .child(who)
            .child(gear)
            .into_any_element()
    }

    /// Keeps where the bar button that opens `overlay` is painted (see
    /// [`Self::under_button`]); its parent is `relative`.
    pub(super) fn button_probe(
        &self,
        overlay: Overlay,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        use gpui_kit::*;
        let weak = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.bar_buttons.insert(overlay, bounds) != Some(bounds) {
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }
}

/// A program's icon on a folded bar, by its id (Members is `identity`,
/// Account `module-registry`, Nodes `valset`); `None` folds to its initial.
fn tab_icon(module: &str) -> Option<gpui_kit::assets::IconName> {
    use gpui_kit::assets::IconName;
    Some(match module {
        "chat" => IconName::MessagesSquare,
        "forge" => IconName::Hammer,
        "identity" => IconName::Users,
        "module-registry" => IconName::CircleUser,
        "valset" => IconName::Server,
        "explorer" => IconName::Compass,
        _ => return None,
    })
}

/// A program's name on the bar. Before the manifest lands (or if it never
/// does) the label is the program's own id, prettified.
pub(super) fn tab_label(row: &crate::runtime::RailRow) -> String {
    match row.note {
        Some(_) => screens::prettify(&row.label),
        None => row.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_folded_tab_is_its_programs_icon_or_its_initial() {
        for module in [
            "chat",
            "forge",
            "identity",
            "module-registry",
            "valset",
            "explorer",
        ] {
            assert!(super::tab_icon(module).is_some(), "{module}");
        }
        assert!(super::tab_icon("ledger").is_none());
    }
}
