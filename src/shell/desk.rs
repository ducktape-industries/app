//! The desk: a menu bar across the top, the open programs' windows under
//! it. The bar holds, left to right: the network (its menu switches), the
//! network's programs as tabs, then Search (⌘K), the node's breath (its
//! status on a click), who is signed in (their menu), and Settings.

use super::*;
use crate::{Popover, Spot};
use screens::{Facts, pulse};

/// The menu bar's height.
pub(super) const BAR: f32 = 36.;

/// The program whose view holds the account's settings, which the account
/// menu opens.
const ACCOUNT_SETTINGS: &str = "module-registry";

impl DesktopWindow {
    /// Inside a node. Reached with an unlocked key, or by choosing to read
    /// without one.
    pub(super) fn console(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let rail = crate::runtime::rail();
        if rail.iter().any(|row| row.note == Some("Loading")) {
            window.request_animation_frame();
        }
        let narrow = window.viewport_size().width < px(NARROW_WINDOW_WIDTH);
        self.initialize_panes(
            state
                .active
                .or_else(|| rail.iter().find(|row| !row.empty).map(|row| row.module)),
            cx,
        );
        // the policy's "in front": this window, if it is, and the view
        // focused in it
        let focused = self
            .layout
            .panes
            .get(self.layout.focused)
            .map_or(super::layout::EMPTY, |pane| pane.module);
        crate::runtime::notify::center().set_front(self.key, window.is_window_active(), focused);
        let console = self.kind == crate::shell::WindowKind::Console;
        let bar = console.then(|| self.menubar(&state, &rail, narrow, window, cx));
        let seat = self.pane_stage(window, cx);
        let overlay = match console {
            false => None,
            true if state.spotlight => Some(self.spotlight(&state, window, cx)),
            true if state.approving => Some(self.approve(&state, window, cx)),
            true if state.settings => Some(self.settings(&state)),
            true if state.network_menu => Some(self.network_menu(&state, narrow)),
            true => state.popover.map(|popover| match popover {
                Popover::Node => self.node_menu(&state, cx),
                Popover::Account => self.account_menu(&state, cx),
                Popover::Notifications => self.notifications(&state, window, cx),
            }),
        };
        if !state.spotlight {
            self.spotlight_focused = false;
        }
        div()
            .id("console")
            .size_full()
            .flex()
            .flex_col()
            .children(bar)
            .child(div().id("seat").flex_1().min_h_0().w_full().child(seat))
            .children(overlay)
            .children(self.footer(cx))
            .into_any_element()
    }

    /// The menu bar (the Menubar board): `height: 36px; padding: 0 8px;
    /// border-bottom: 1px solid line`; every item `height: 36px; padding: 0
    /// 10px; gap: 8px; font: 400 13px`, on `surface` while its menu is open.
    fn menubar(
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
        let open: Vec<&'static str> = self.layout.panes.iter().map(|pane| pane.module).collect();
        let focused = self
            .layout
            .panes
            .get(self.layout.focused)
            .map(|pane| pane.module);
        let tabs = rail.iter().filter(|row| !row.empty).map(|row| {
            let module = row.module;
            let selected = focused == Some(module);
            let badge = state.badges.get(module).copied().unwrap_or(0);
            let shown = tab_label(row);
            let name = match row.note {
                Some(note) => format!("{shown} · {note}"),
                None => shown.clone(),
            };
            let shown = match narrow {
                true => shown.chars().next().map(String::from).unwrap_or_default(),
                false => shown,
            };
            let hover = ink.ink;
            sans(400, 13.)
                .id(SharedString::from(format!("rail/{module}")))
                .control(Role::Tab, SharedString::from(name))
                .aria_selected(selected)
                .focusable()
                .tab_stop(true)
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
                        true => this.pane_message(Message::SelectView(module), window, cx),
                        false => this.open_view(module, window, cx),
                    }
                }))
                .child(shown)
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
            state.network_menu,
            || Message::ToggleNetworkMenu,
        )
        .aria_expanded(state.network_menu)
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
        let bell_open = state.popover == Some(Popover::Notifications);
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
        .child(
            div()
                .relative()
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Bell)
                        .size(px(16.))
                        .text_color(ink.muted),
                )
                .when(unread > 0, |bell| {
                    bell.child(
                        mono(500, 9.)
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
        let (breath, said) = match (state.connecting, state.reconnecting) {
            (true, _) => (true, "Node: switching".to_owned()),
            (_, true) => (false, "Node: not answering".to_owned()),
            (false, false) => (true, format!("Node: in sync, block {}", state.height)),
        };
        let node_open = state.popover == Some(Popover::Node);
        let node = item("rail-connection", said.into(), node_open, || {
            Message::TogglePopover(Popover::Node)
        })
        .aria_expanded(node_open)
        .px(px(12.))
        .child(pulse(breath, state.motion, &ink));
        let unlocked = !state.signer_key.is_empty();
        let account_open = state.popover == Some(Popover::Account);
        let who = match (&state.account, unlocked) {
            (_, false) => item("sign-in", "Sign in".into(), false, || Message::SignIn)
                .child(div().underline().child("Sign in")),
            (Some(None), true) => item(
                "rail-account",
                "Account: no account yet — create one".into(),
                false,
                || Message::ShowCreateAccount,
            )
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
        let titlebar = cfg!(target_os = "macos") && !window.is_fullscreen();
        let handle = div()
            .id("menubar-handle")
            .flex_1()
            .min_w(px(8.))
            .h_full()
            .when(titlebar, |strip| {
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
            .pl(px(if titlebar { 78. } else { 8. }))
            .pr(px(8.))
            .border_b_1()
            .border_color(ink.line)
            .bg(ink.bg)
            .child(network)
            .child(div().w(px(1.)).h(px(16.)).mx(px(6.)).bg(ink.line))
            .child(
                div()
                    .id("rail-rows")
                    .role(Role::TabList)
                    .min_w_0()
                    .flex()
                    .overflow_hidden()
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
            // THE HOST'S OWN WORD ON WHAT IS RECORDING: a seated view draws
            // inside its seat and can neither paint here nor decline to be
            // listed.
            .when_some(crate::runtime::capturing(), |bar, recording| {
                bar.child(
                    mono(400, 11.)
                        .id("capture-indicator")
                        .role(Role::Status)
                        .mx_1()
                        .px_1p5()
                        .bg(ink.danger)
                        .text_color(ink.bg)
                        .child(recording),
                )
            })
            .child(search)
            .child(bell)
            .child(node)
            .child(who)
            .child(gear)
            .into_any_element()
    }

    /// A menu hanging below the bar (the canvas's menus: `top: 40px;
    /// border: 1.5px solid ink; box-shadow: 0 10px 30px`), over a backdrop
    /// that closes it; Escape closes it too. `right` places it from the
    /// window's right edge.
    fn hanging(
        &self,
        id: &'static str,
        name: &'static str,
        right: f32,
        width: f32,
        body: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let close = self.model.clone();
        let escape = self.model.clone();
        div()
            .id(SharedString::from(format!("{id}-backdrop")))
            // below the bar: its items stay live, and one menu gives way to
            // the next in one click
            .absolute()
            .top(px(BAR))
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .on_click(move |_, _, cx| {
                close.update(cx, |model, cx| model.dispatch(Message::ClosePopover, cx));
            })
            .child(
                div()
                    .id(id)
                    .control(Role::Dialog, name)
                    .occlude()
                    .absolute()
                    .top(px(4.))
                    .right(px(right))
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .bg(ink.bg)
                    .text_color(ink.ink)
                    .border(px(1.5))
                    .border_color(ink.ink)
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_key_down(move |event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            escape
                                .update(cx, |model, cx| model.dispatch(Message::ClosePopover, cx));
                        }
                    })
                    .child(body),
            )
            .into_any_element()
    }

    /// A menu item: `height: 36px; padding: 0 16px; font: 400 14px`, a mono
    /// hint at its right end.
    fn menu_row(
        &self,
        id: &'static str,
        label: &'static str,
        run: impl Fn(&mut gpui_kit::App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        self.menu_row_hint(id, label, "", run, cx)
    }

    fn menu_row_hint(
        &self,
        id: &'static str,
        label: &'static str,
        hint: &'static str,
        run: impl Fn(&mut gpui_kit::App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use super::ink::{Ink, mono, sans};
        use gpui_kit::*;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let surface = ink.surface;
        crate::a11y::keyboard(
            sans(400, 14.)
                .id(id)
                .control(Role::MenuItem, label)
                .h(px(36.))
                .px(px(16.))
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(move |style| style.bg(surface))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    run(cx)
                })
                .child(label)
                .child(mono(400, 12.).text_color(ink.muted).child(hint)),
        )
    }

    fn dispatching(&self, message: fn() -> Message) -> impl Fn(&mut gpui_kit::App) + 'static {
        let model = self.model.clone();
        move |cx| model.update(cx, |model, cx| model.dispatch(message(), cx))
    }

    /// What the breath means (the NodeStatus board): in sync or not, and
    /// the node's own numbers, `padding: 7px 16px` each.
    fn node_menu(&self, state: &Facts, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let ok = !state.reconnecting;
        let host = crate::backend::host_of(&state.connected_rpc).to_owned();
        let row = |key: &'static str, value: String, code: bool| {
            div()
                .flex()
                .justify_between()
                .items_baseline()
                .px(px(16.))
                .py(px(7.))
                .child(sans(400, 13.).text_color(ink.muted).child(key))
                .child(
                    match code {
                        true => mono(400, 13.),
                        false => sans(400, 13.),
                    }
                    .text_color(ink.ink)
                    .child(value),
                )
        };
        let mut rows = vec![];
        if let Some(node) = &state.node {
            rows.push(row("Height", grouped(node.height), true));
            rows.push(row(
                "Last block",
                match state.block_age {
                    ..=0 => "just now".to_owned(),
                    age => format!("{age} s ago"),
                },
                false,
            ));
            rows.push(row(
                "Block time",
                format!("{:.1} s", node.block_time_ms as f64 / 1000.),
                false,
            ));
            let (into, of) = match node.epoch_length {
                0 => (0, 0),
                length => (node.height % length, length),
            };
            rows.push(row(
                "Epoch",
                format!("{} · {into} / {of}", node.epoch),
                true,
            ));
            let share = match of {
                0 => 0.,
                of => into as f32 / of as f32,
            };
            rows.push(
                div()
                    .mx(px(16.))
                    .mt(px(2.))
                    .mb(px(8.))
                    .h(px(2.))
                    .bg(ink.line)
                    .child(div().h_full().w(relative(share)).bg(ink.ink)),
            );
            rows.push(row("Tip", short_hex(&node.tip), true));
            rows.push(row("State root", short_hex(&node.root.0), true));
            rows.push(row("Node key", short_hex(&node.identity), true));
            rows.push(row("Contract", format!("v{}", node.contract), true));
            rows.push(row("Chain founded", founded(node.time), false));
        }
        let copy = {
            let url = state.connected_rpc.clone();
            self.menu_row(
                "copy-node",
                "Copy node address",
                move |cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url.clone())),
                cx,
            )
        };
        let body = div()
            .pb(px(6.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pt(px(14.))
                    .px(px(16.))
                    .pb(px(12.))
                    .child(pulse(ok, state.motion, &ink))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(sans(500, 15.).child(match ok {
                                true => "In sync",
                                false => "Not answering",
                            }))
                            .child(
                                mono(400, 12.)
                                    .text_color(ink.muted)
                                    .child(format!("{} · {host}", state.network)),
                            ),
                    ),
            )
            .child(div().h(px(1.)).mb(px(6.)).bg(ink.line))
            .children(rows)
            .child(div().h(px(1.)).my(px(6.)).bg(ink.line))
            .child(copy)
            .child(self.menu_row(
                "node-switch",
                "Switch node…",
                self.dispatching(|| Message::ToggleNetworkMenu),
                cx,
            ));
        self.hanging("node-status", "Node status", 120., 340., body, cx)
    }

    /// The bell's panel (the NotifCenter board): 400 wide under the bell,
    /// kept inside the window; the rows, newest first, under Today and
    /// Earlier.
    fn notifications(
        &self,
        state: &Facts,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        const WIDTH: f32 = 400.;
        let ink = Ink::of(state.dark);
        let viewport = window.viewport_size();
        let (wide, high) = (f32::from(viewport.width), f32::from(viewport.height));
        let width = WIDTH.min(wide - 16.).max(0.);
        // under the bell, else as far right as keeps it on screen
        let right = 150_f32.min(wide - width - 8.).max(8.);
        let now = crate::runtime::notify::wall();
        let midnight = crate::runtime::notify::local_midnight(now);
        let center = crate::runtime::notify::center();
        let unread = center.unread();
        let entries: Vec<_> = center.entries().cloned().collect();
        drop(center);
        let small_link = |id: &'static str, text: &'static str, message: fn() -> Message| {
            let model = self.model.clone();
            let hover = ink.ink;
            crate::a11y::keyboard(
                sans(400, 13.)
                    .id(id)
                    .control(Role::Button, text)
                    .text_color(ink.muted)
                    .cursor_pointer()
                    .hover(move |style| style.text_color(hover))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        model.update(cx, |model, cx| model.dispatch(message(), cx))
                    }),
            )
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .h(px(46.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(ink.line)
            .child(sans(500, 14.).child("Notifications"))
            .child(
                mono(400, 12.)
                    .flex_1()
                    .text_color(ink.muted)
                    .child(format!("{unread} unread")),
            )
            .when(unread > 0, |header| {
                header.child(
                    small_link("notif-mark-all", "Mark all read", || {
                        Message::NotifyMarkAllRead
                    })
                    .child("Mark all read")
                    .text_color(ink.ink)
                    .underline(),
                )
            });
        let mut list = div()
            .id("notif-rows")
            .role(Role::Menu)
            .aria_label("Notifications")
            .flex()
            .flex_col()
            .max_h(px((high - BAR - 4. - 46. - 38. - 8.).max(80.)))
            .overflow_y_scroll();
        let mut section = None;
        for entry in &entries {
            let today = entry.at >= midnight;
            if section != Some(today) {
                section = Some(today);
                list = list.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .px(px(16.))
                        .pt(px(12.))
                        .pb(px(6.))
                        .border_b_1()
                        .border_color(ink.line)
                        .child(if today { "Today" } else { "Earlier" }),
                );
            }
            let model = self.model.clone();
            let id = entry.id;
            let surface = ink.surface;
            let initial: String = entry
                .title
                .chars()
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .collect();
            let source = match entry.tag.is_empty() {
                true => entry.module.clone(),
                false => format!("{} · {}", entry.module, entry.tag),
            };
            let said = format!(
                "{}{}: {}",
                if entry.read { "" } else { "Unread. " },
                entry.title,
                entry.body
            );
            let line = |text: String, color: Hsla| {
                sans(400, 13.)
                    .text_color(color)
                    .whitespace_nowrap()
                    .truncate()
                    .child(text)
            };
            list = list.child(crate::a11y::keyboard(
                div()
                    .id(SharedString::from(format!("notif/{id}")))
                    .control(Role::MenuItem, SharedString::from(said))
                    .flex()
                    .gap(px(10.))
                    .pl(px(8.))
                    .pr(px(16.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(ink.line)
                    .cursor_pointer()
                    .hover(move |style| style.bg(surface))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        model.update(cx, |model, cx| model.dispatch(Message::NotifyOpen(id), cx))
                    })
                    .child(
                        div()
                            .w(px(8.))
                            .h(px(28.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .when(!entry.read, |dot| {
                                dot.child(div().size(px(5.)).rounded_full().bg(ink.ink))
                            }),
                    )
                    .child(
                        sans(500, 12.)
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(ink.surface)
                            .text_color(ink.muted)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        sans(if entry.read { 400 } else { 500 }, 14.)
                                            .min_w_0()
                                            .truncate()
                                            .text_color(match entry.read {
                                                true => ink.muted,
                                                false => ink.ink,
                                            })
                                            .child(entry.title.clone()),
                                    )
                                    .when(entry.count > 1, |row| {
                                        row.child(
                                            mono(400, 11.)
                                                .px(px(5.))
                                                .border_1()
                                                .border_color(ink.line)
                                                .text_color(ink.muted)
                                                .child(entry.count.to_string()),
                                        )
                                    })
                                    .child(div().flex_1())
                                    .child(
                                        mono(400, 12.)
                                            .flex_shrink_0()
                                            .text_color(ink.muted)
                                            .child(crate::runtime::notify::ago(entry.at, now)),
                                    ),
                            )
                            .when(!entry.body.is_empty(), |text| {
                                text.child(line(
                                    entry.body.replace('\n', " "),
                                    match entry.read {
                                        true => ink.muted,
                                        false => ink.figure,
                                    },
                                ))
                            })
                            .child(mono(400, 11.).text_color(ink.muted).child(source)),
                    ),
            ));
        }
        if entries.is_empty() {
            list = list.child(
                div()
                    .id("notif-empty")
                    .role(Role::Status)
                    .aria_label("You\u{2019}re all caught up")
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .py(px(44.))
                    .child(sans(500, 14.).child("You\u{2019}re all caught up"))
                    .child(
                        sans(400, 13.)
                            .text_color(ink.muted)
                            .child("Mentions and messages land here."),
                    ),
            );
        }
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(16.))
            .h(px(38.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(ink.line)
            .child(
                small_link("notif-clear-read", "Clear read", || {
                    Message::NotifyClearRead
                })
                .child("Clear read"),
            )
            .child(
                small_link("notif-settings", "Notification settings", || {
                    Message::NotifySettings
                })
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                        .size(px(13.)),
                )
                .child("Notification settings"),
            );
        let body = div()
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .child(footer);
        self.hanging("notifications", "Notifications", right, width, body, cx)
    }

    /// Who is signed in, and the ways out.
    fn account_menu(&self, state: &Facts, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let (name, number) = match &state.account {
            Some(Some((number, name))) => (name.clone(), Some(*number)),
            _ => ("Signed in".to_owned(), None),
        };
        let detail = match number {
            Some(number) => format!("account {number} · {}", state.network),
            None => state.network.clone(),
        };
        let mut rows = div().flex().flex_col();
        if number.is_some() {
            rows = rows.child(self.menu_row(
                "account-settings",
                "Account settings…",
                self.dispatching(|| Message::SelectView(crate::runtime::intern(ACCOUNT_SETTINGS))),
                cx,
            ));
        }
        if let Some(number) = number {
            rows = rows.child(self.menu_row(
                "copy-account",
                "Copy account number",
                move |cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(number.to_string()))
                },
                cx,
            ));
            rows = rows
                .child(self.menu_row(
                    "add-device",
                    "Add a device…",
                    self.dispatching(|| Message::ApproveOpen),
                    cx,
                ))
                .child(self.menu_row(
                    "recovery-key",
                    "Make a recovery key…",
                    self.dispatching(|| Message::RecoveryKeyStart),
                    cx,
                ));
        }
        let body = div()
            .flex()
            .flex_col()
            .py(px(6.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .pt(px(10.))
                    .px(px(16.))
                    .pb(px(12.))
                    .child(sans(500, 16.).child(name))
                    .child(mono(400, 12.).text_color(ink.muted).child(detail))
                    .child(
                        sans(400, 13.)
                            .text_color(ink.muted)
                            .child("This device's key, kept by the system"),
                    ),
            )
            .child(div().h(px(1.)).mb(px(6.)).bg(ink.line))
            .child(rows)
            .child(div().h(px(1.)).my(px(6.)).bg(ink.line))
            .child(
                div()
                    .child(self.menu_row("lock", "Lock", self.dispatching(|| Message::Lock), cx))
                    .child(self.menu_row(
                        "disconnect",
                        "Switch node…",
                        self.dispatching(|| Message::Disconnect),
                        cx,
                    )),
            );
        self.hanging("account-menu", "Account", 44., 300., body, cx)
    }

    /// "Add a device…": the code a new device shows, then its fingerprint
    /// to compare, then this device's yes — consent it signs for the
    /// account and hands over the relay. Dressed as the canvas's dialogs:
    /// `border: 1.5px solid ink`, a soft shadow, on a scrim below the bar.
    fn approve(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::{self, *};
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let error = (!state.unlock_error.is_empty())
            .then(|| self.alert("approve-error", state.unlock_error.clone(), &ink));
        let (said, fields) = match &state.approve_fingerprint {
            None => {
                let code = self.input(
                    "approve-code",
                    "XXXX-XXXX",
                    false,
                    |state| &state.approve_code,
                    Message::ApproveCodeTyped,
                    || Message::ApproveFind,
                    Some("Code".into()),
                    false,
                    15.,
                    window,
                    cx,
                );
                (
                    "On the new device, choose \"Add this device from another device\". Type the code it shows.",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(
                            self.field(
                                "Code",
                                field_box(code, ink.strong, 44., &ink)
                                    .font_family(super::theme::FAMILY_MONO)
                                    .into_any_element(),
                                error,
                                &ink,
                            ),
                        )
                        .child(div().flex().child(self.button(
                            "approve-find",
                            "Next",
                            Kind::Primary,
                            || Message::ApproveFind,
                            state.unlock_busy,
                            &ink,
                        ))),
                )
            }
            Some(fingerprint) => (
                "Approve only if the new device shows these same characters. It can then write as your account.",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(crate::a11y::whole(
                        mono(400, 28.)
                            .id("approve-fingerprint")
                            .role(Role::Label)
                            .aria_label(fingerprint.clone())
                            .child(fingerprint.clone()),
                    ))
                    .children(error)
                    .child(div().flex().child(self.button(
                        "approve-confirm",
                        "Approve",
                        Kind::Primary,
                        || Message::ApproveConfirm,
                        state.unlock_busy,
                        &ink,
                    ))),
            ),
        };
        let close = self.model.clone();
        div()
            .id("approve-backdrop")
            .absolute()
            .top(px(BAR))
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .bg(ink.bg.opacity(0.6))
            .flex()
            .justify_center()
            .on_click(move |_, _, cx| {
                close.update(cx, |model, cx| model.dispatch(Message::ApproveClose, cx));
            })
            .child(
                div()
                    .id("approve")
                    .control(Role::Dialog, "Add a device")
                    .occlude()
                    .mt(px(84.))
                    .w(px(440.))
                    .max_w_full()
                    .self_start()
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .p(px(24.))
                    .bg(ink.bg)
                    .border(px(1.5))
                    .border_color(ink.ink)
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(tag("Add a device", &ink))
                    .child(ink::note(said, ink.muted))
                    .child(fields)
                    .child(self.link(
                        "approve-cancel",
                        "Cancel",
                        || Message::ApproveClose,
                        false,
                        &ink,
                    )),
            )
            .into_any_element()
    }

    /// ⌘K: one field, and what it finds among the programs, the networks
    /// and the things to do. ↑↓ pick, Enter runs, Escape closes.
    fn spotlight(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let field = self.input(
            "spotlight",
            "Search programs, networks, actions",
            false,
            |state| &state.spotlight_query,
            Message::SpotlightTyped,
            || Message::SpotlightSubmit,
            Some("Search".into()),
            false,
            20.,
            window,
            cx,
        );
        if !self.spotlight_focused {
            self.spotlight_focused = true;
            if let Some(input) = self.inputs.get("spotlight") {
                let state = input.state.clone();
                window.defer(cx, move |window, cx| {
                    state.update(cx, |state, cx| state.focus(window, cx));
                });
            }
        }
        let rows = self.model.read(cx).state.spotlight_rows();
        let count = rows.len();
        let pick = state.spotlight_pick.min(count.saturating_sub(1));
        let mut list = div()
            .id("spotlight-rows")
            .role(Role::Menu)
            .max_h(px(380.))
            .overflow_y_scroll()
            .py(px(6.));
        let mut group = "";
        for (nth, row) in rows.into_iter().enumerate() {
            if row.group != group {
                group = row.group;
                // groups `padding: 6px 0`, a hairline between them
                if nth > 0 {
                    list = list.child(div().mt(px(6.)).mb(px(6.)).h(px(1.)).bg(ink.line));
                }
                list = list.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .px(px(16.))
                        .py(px(6.))
                        .child(group),
                );
            }
            let model = self.model.clone();
            let spot = row.spot.clone();
            let picked = nth == pick;
            let hint = match (&row.spot, picked) {
                (_, false) => "",
                (Spot::Switch(_), true) => "switch",
                (_, true) => "↵",
            };
            list = list.child(
                sans(400, 13.)
                    .id(SharedString::from(format!("spotlight/{nth}")))
                    .control(Role::MenuItem, SharedString::from(row.title.clone()))
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(10.))
                    .cursor_pointer()
                    .when(picked, |row| row.bg(ink.surface))
                    .on_click(move |_, _, cx| {
                        let spot = spot.clone();
                        model.update(cx, |model, cx| model.dispatch(Message::Spot(spot), cx));
                    })
                    .child(
                        sans(if picked { 500 } else { 400 }, 15.)
                            .text_color(ink.ink)
                            .child(row.title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(ink.muted)
                            .child(row.meta),
                    )
                    .child(mono(400, 12.).text_color(ink.muted).child(hint)),
            );
        }
        if count == 0 {
            list = list.child(
                sans(400, 14.)
                    .px(px(16.))
                    .py(px(12.))
                    .text_color(ink.muted)
                    .child("Nothing here by that name."),
            );
        }
        let keys = self.model.clone();
        let close = self.model.clone();
        let scrim = ink.bg.opacity(0.6);
        div()
            .id("spotlight-backdrop")
            .absolute()
            .top(px(BAR))
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .bg(scrim)
            .flex()
            .justify_center()
            .on_click(move |_, _, cx| {
                close.update(cx, |model, cx| model.dispatch(Message::CloseSpotlight, cx));
            })
            .child(
                div()
                    .id("spotlight")
                    .control(Role::Dialog, "Search")
                    .occlude()
                    .mt(px(84.))
                    .w(px(600.))
                    .max_w_full()
                    .h_auto()
                    .self_start()
                    .flex()
                    .flex_col()
                    .bg(ink.bg)
                    .text_color(ink.ink)
                    .border(px(1.5))
                    .border_color(ink.ink)
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .capture_key_down(move |event: &KeyDownEvent, _, cx| {
                        let message = match event.keystroke.key.as_str() {
                            "up" => Message::SpotlightMove {
                                down: false,
                                rows: count,
                            },
                            "down" => Message::SpotlightMove {
                                down: true,
                                rows: count,
                            },
                            "escape" => Message::CloseSpotlight,
                            _ => return,
                        };
                        cx.stop_propagation();
                        keys.update(cx, |model, cx| model.dispatch(message, cx));
                    })
                    .child(
                        div()
                            .h(px(56.))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(ink.line)
                            .gap(px(12.))
                            .child(div().flex_1().child(field))
                            .child(mono(400, 12.).text_color(ink.muted).child("esc")),
                    )
                    .child(list)
                    .child(
                        mono(400, 12.)
                            .flex()
                            .gap(px(20.))
                            .px(px(16.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(ink.line)
                            .text_color(ink.muted)
                            .child("↑↓ move")
                            .child("↵ open")
                            .child("esc close"),
                    ),
            )
            .into_any_element()
    }
}

/// A program's name on the bar. Before the manifest lands (or if it never
/// does) the label is the program's own id, prettified.
pub(super) fn tab_label(row: &crate::runtime::RailRow) -> String {
    match row.note {
        Some(_) => screens::prettify(&row.label),
        None => row.label.clone(),
    }
}

/// `6230` → "6,230".
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (nth, digit) in digits.chars().enumerate() {
        if nth > 0 && (digits.len() - nth).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The first and last few bytes of a hash, enough to tell two apart.
fn short_hex(bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    match hex.len() > 14 {
        true => format!("{}…{}", &hex[..8], &hex[hex.len() - 4..]),
        false => hex,
    }
}

/// The chain's founding time as a date. The node's clock reads in
/// milliseconds; a value that small is taken as seconds.
fn founded(time: u64) -> String {
    let seconds = match time > 100_000_000_000 {
        true => time / 1000,
        false => time,
    };
    // days since 1970-01-01 → civil date (Howard Hinnant's algorithm)
    let z = (seconds / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_numbers_read_short() {
        assert_eq!(grouped(6230), "6,230");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000_000), "1,000,000");
        assert_eq!(short_hex(&[0xab; 32]), "abababab…abab");
        assert_eq!(short_hex(&[1, 2]), "0102");
        assert_eq!(founded(0), "1970-01-01");
        assert_eq!(founded(1_790_121_600), "2026-09-23");
        assert_eq!(founded(1_790_121_600_000), "2026-09-23");
    }
}
