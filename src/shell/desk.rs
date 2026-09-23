//! The desk: a menu bar across the top, the open programs' windows under
//! it. The bar holds, left to right: the network (its menu switches), the
//! network's programs as tabs, then Search (⌘K), the node's breath (its
//! status on a click), who is signed in (their menu), and Settings.

use super::*;
use crate::{Popover, Spot};
use launcher::mono;
use screens::{Facts, pulse};

/// The menu bar's height.
const BAR: f32 = 36.;

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
        let console = self.kind == crate::shell::WindowKind::Console;
        let bar = console.then(|| self.menubar(&state, &rail, narrow, window, cx));
        let seat = self.pane_stage(window, cx);
        let overlay = match console {
            false => None,
            true if state.spotlight => Some(self.spotlight(&state, window, cx)),
            true if state.network_menu => Some(self.network_menu(&state, narrow, cx)),
            true => state.popover.map(|popover| match popover {
                Popover::Node => self.node_menu(&state, cx),
                Popover::Account => self.account_menu(&state, cx),
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

    fn menubar(
        &self,
        state: &Facts,
        rail: &[crate::runtime::RailRow],
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let fg = hsla_of(palette.foreground);
        let muted = hsla_of(palette.muted);
        let raised = hsla_of(palette.surface_raised);
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
            // Before the manifest lands (or if it never does) the label is
            // the program's own id, prettified.
            let shown = match row.note {
                Some(_) => screens::prettify(&row.label),
                None => row.label.clone(),
            };
            let name = match row.note {
                Some(note) => format!("{shown} · {note}"),
                None => shown.clone(),
            };
            let shown = match narrow {
                true => shown.chars().next().map(String::from).unwrap_or_default(),
                false => shown,
            };
            div()
                .id(SharedString::from(format!("rail/{module}")))
                .control(Role::Tab, SharedString::from(name))
                .aria_selected(selected)
                .focusable()
                .tab_stop(true)
                .h(px(BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1p5()
                .px_2p5()
                .cursor_pointer()
                .text_color(match selected || open.contains(&module) {
                    true => fg,
                    false => muted,
                })
                .when(selected, |tab| tab.font_weight(FontWeight::MEDIUM))
                .hover(move |style| style.text_color(fg))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let message = match event.modifiers().shift {
                        true => Message::SplitView(module),
                        false => Message::SelectView(module),
                    };
                    this.pane_message(message, window, cx);
                }))
                .child(shown)
                .when(row.note == Some("Failed"), |tab| {
                    tab.child(
                        div()
                            .size(px(5.))
                            .rounded_full()
                            .bg(hsla_of(palette.danger)),
                    )
                })
                .when(badge > 0, |tab| {
                    tab.child(mono(badge.to_string(), muted).text_size(px(11.)))
                })
        });
        let item = |id: &'static str, name: SharedString, message: fn() -> Message| {
            let model = self.model.clone();
            crate::a11y::keyboard(
                div()
                    .id(id)
                    .control(Role::Button, name)
                    .h(px(BAR))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2p5()
                    .cursor_pointer()
                    .hover(move |style| style.bg(raised))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        model.update(cx, |model, cx| model.dispatch(message(), cx));
                    }),
            )
        };
        let network = item(
            "network-switcher",
            SharedString::from(format!("Network: {}", state.network)),
            || Message::ToggleNetworkMenu,
        )
        .aria_expanded(state.network_menu)
        .when(state.network_menu, |item| item.bg(raised))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(state.network.clone()),
        )
        .child(
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::ChevronDown)
                .size(px(13.))
                .text_color(muted),
        );
        let chord = match cfg!(target_os = "macos") {
            true => "⌘K",
            false => "Ctrl K",
        };
        let search = item("rail-search", "Search".into(), || Message::OpenSpotlight)
            .when(!narrow, |item| {
                item.child(div().text_color(muted).child("Search"))
            })
            .child(
                mono(chord, muted)
                    .text_size(px(11.))
                    .px_1()
                    .border_1()
                    .border_color(hsla_of(palette.border)),
            );
        let (breath, said) = match (state.connecting, state.reconnecting) {
            (true, _) => (true, "Node: switching".to_owned()),
            (_, true) => (false, "Node: not answering".to_owned()),
            (false, false) => (true, format!("Node: in sync, block {}", state.height)),
        };
        let node = item("rail-connection", said.into(), || {
            Message::TogglePopover(Popover::Node)
        })
        .aria_expanded(state.popover == Some(Popover::Node))
        .when(state.popover == Some(Popover::Node), |item| item.bg(raised))
        .child(pulse(breath, state.motion, palette))
        .when(!breath && !narrow, |item| {
            item.child(mono("reconnecting", muted))
        });
        let unlocked = !state.signer_key.is_empty();
        let who = match (&state.account, unlocked) {
            (_, false) => item("sign-in", "Sign in".into(), || Message::SignIn)
                .child(div().underline().child("Sign in")),
            (Some(None), true) => item(
                "rail-account",
                "Account: no account yet — create one".into(),
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
                    || Message::TogglePopover(Popover::Account),
                )
                .aria_expanded(state.popover == Some(Popover::Account))
                .when(state.popover == Some(Popover::Account), |item| {
                    item.bg(raised)
                })
                .child(shown)
            }
        };
        // Settings are the app's, not the account's: their own spot at the
        // edge.
        let gear = item("settings", "Ducktape settings".into(), || {
            Message::OpenSettings
        })
        .px_2()
        .child(
            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                .size(px(15.))
                .text_color(muted),
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
            .pl(px(if titlebar { 78. } else { 6. }))
            .pr_1()
            .border_b_1()
            .border_color(hsla_of(palette.border))
            .bg(hsla_of(palette.background))
            .text_size(px(13.))
            .child(network)
            .child(
                div()
                    .w(px(1.))
                    .h(px(16.))
                    .mx_1p5()
                    .bg(hsla_of(palette.border)),
            )
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
                            div()
                                .px_2()
                                .text_color(muted)
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
                    div()
                        .id("capture-indicator")
                        .role(Role::Status)
                        .mx_1()
                        .px_1p5()
                        .bg(hsla_of(palette.danger))
                        .text_size(px(11.))
                        .text_color(hsla_of(palette.background))
                        .child(recording),
                )
            })
            .child(search)
            .child(node)
            .child(who)
            .child(gear)
            .into_any_element()
    }

    /// A menu hanging from the bar's right side, over a backdrop that
    /// closes it; Escape closes it too.
    fn hanging(
        &self,
        id: &'static str,
        name: &'static str,
        right: f32,
        width: f32,
        body: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let palette = design::palette(self.model.read(cx).state.dark());
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
                    .bg(hsla_of(palette.background))
                    .border(px(1.5))
                    .border_color(hsla_of(palette.foreground))
                    .shadow_lg()
                    .text_size(px(13.))
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

    /// A row in a hanging menu that does one thing.
    fn menu_row(
        &self,
        id: &'static str,
        label: &'static str,
        run: impl Fn(&mut gpui_kit::App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::*;
        let raised = hsla_of(design::palette(self.model.read(cx).state.dark()).surface_raised);
        crate::a11y::keyboard(
            div()
                .id(id)
                .control(Role::MenuItem, label)
                .px_4()
                .py_2()
                .cursor_pointer()
                .hover(move |style| style.bg(raised))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    run(cx)
                })
                .child(label),
        )
    }

    fn dispatching(&self, message: fn() -> Message) -> impl Fn(&mut gpui_kit::App) + 'static {
        let model = self.model.clone();
        move |cx| model.update(cx, |model, cx| model.dispatch(message(), cx))
    }

    /// What the breath means: in sync or not, and the node's own numbers.
    fn node_menu(&self, state: &Facts, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let muted = hsla_of(palette.muted);
        let border = hsla_of(palette.border);
        let ok = !state.reconnecting;
        let host = state
            .connected_rpc
            .split_once("://")
            .map_or(state.connected_rpc.as_str(), |(_, host)| host)
            .to_owned();
        let row = |key: &'static str, value: String| {
            div()
                .flex()
                .justify_between()
                .gap_4()
                .px_4()
                .py(px(5.))
                .child(div().text_color(muted).child(key))
                .child(mono(value, hsla_of(palette.foreground)))
        };
        let mut rows = vec![];
        if let Some(node) = &state.node {
            rows.push(row("Height", grouped(node.height)));
            rows.push(row(
                "Last block",
                match state.block_age {
                    ..=0 => "just now".to_owned(),
                    age => format!("{age} s ago"),
                },
            ));
            rows.push(row("Block time", format!("{} ms", node.block_time_ms)));
            let into = match node.epoch_length {
                0 => 0.,
                length => (node.height % length) as f32 / length as f32,
            };
            rows.push(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap_4()
                    .px_4()
                    .py(px(5.))
                    .child(div().text_color(muted).child("Epoch"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div().w(px(60.)).h(px(2.)).bg(border).child(
                                    div()
                                        .h_full()
                                        .w(px(60. * into))
                                        .bg(hsla_of(palette.foreground)),
                                ),
                            )
                            .child(mono(node.epoch.to_string(), hsla_of(palette.foreground))),
                    ),
            );
            rows.push(row("Tip", short_hex(&node.tip)));
            rows.push(row("State root", short_hex(&node.root.0)));
            rows.push(row("Node key", short_hex(&node.identity)));
            rows.push(row("Contract", format!("v{}", node.contract)));
            rows.push(row("Founded", founded(node.time)));
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
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .child(pulse(ok, state.motion, palette))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(FontWeight::MEDIUM).child(match ok {
                                true => "In sync",
                                false => "Not answering",
                            }))
                            .child(mono(format!("{} · {host}", state.network), muted)),
                    ),
            )
            .child(div().py_2().flex().flex_col().children(rows))
            .child(
                div()
                    .py_1()
                    .border_t_1()
                    .border_color(border)
                    .child(copy)
                    .child(self.menu_row(
                        "node-switch",
                        "Switch node…",
                        self.dispatching(|| Message::ToggleNetworkMenu),
                        cx,
                    )),
            );
        self.hanging("node-status", "Node status", 120., 340., body, cx)
    }

    /// Who is signed in, and the ways out.
    fn account_menu(&self, state: &Facts, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let muted = hsla_of(palette.muted);
        let border = hsla_of(palette.border);
        let (name, number) = match &state.account {
            Some(Some((number, name))) => (name.clone(), Some(*number)),
            _ => ("Signed in".to_owned(), None),
        };
        let detail = match number {
            Some(number) => format!("account {number} · {}", state.network),
            None => state.network.clone(),
        };
        let mut rows = div().py_1().flex().flex_col();
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
        }
        let body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(mono(detail, muted))
                    .child(
                        div()
                            .pt_1()
                            .text_size(px(12.5))
                            .text_color(muted)
                            .child("Key on this device, locked with a password"),
                    ),
            )
            .child(rows)
            .child(
                div()
                    .py_1()
                    .border_t_1()
                    .border_color(border)
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

    /// ⌘K: one field, and what it finds among the programs, the networks
    /// and the things to do. ↑↓ pick, Enter runs, Escape closes.
    fn spotlight(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let muted = hsla_of(palette.muted);
        let border = hsla_of(palette.border);
        let field = self.input(
            "spotlight",
            "Search programs, networks, actions",
            false,
            |state| &state.spotlight_query,
            Message::SpotlightTyped,
            || Message::SpotlightSubmit,
            Some("Search".into()),
            false,
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
            .py_1();
        let mut group = "";
        for (nth, row) in rows.into_iter().enumerate() {
            if row.group != group {
                group = row.group;
                list = list.child(mono(group, muted).px_4().pt_2().pb_1());
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
                div()
                    .id(SharedString::from(format!("spotlight/{nth}")))
                    .control(Role::MenuItem, SharedString::from(row.title.clone()))
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .when(picked, |row| row.bg(hsla_of(palette.surface_raised)))
                    .on_click(move |_, _, cx| {
                        let spot = spot.clone();
                        model.update(cx, |model, cx| model.dispatch(Message::Spot(spot), cx));
                    })
                    .child(div().text_size(px(14.)).child(row.title))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(row.meta),
                    )
                    .child(mono(hint, muted)),
            );
        }
        if count == 0 {
            list = list.child(
                div()
                    .px_4()
                    .py_3()
                    .text_color(muted)
                    .child("Nothing here by that name."),
            );
        }
        let keys = self.model.clone();
        let close = self.model.clone();
        let scrim = hsla_of(palette.background).opacity(0.6);
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
                    .mt(px(60.))
                    .w(px(600.))
                    .max_w_full()
                    .h_auto()
                    .self_start()
                    .flex()
                    .flex_col()
                    .bg(hsla_of(palette.background))
                    .border(px(1.5))
                    .border_color(hsla_of(palette.foreground))
                    .shadow_lg()
                    .text_size(px(13.))
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
                    .child(div().p_2().border_b_1().border_color(border).child(field))
                    .child(list)
                    .child(
                        mono("↑↓ move   ↵ open   esc close", muted)
                            .px_4()
                            .py_2()
                            .border_t_1()
                            .border_color(border),
                    ),
            )
            .into_any_element()
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
