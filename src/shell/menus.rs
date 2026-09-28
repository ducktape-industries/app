//! The bar's menus: the node's status, the account, and the networks this
//! device has reached. The network menu is the old app's "Switch network"
//! block (before #216), without its detour through the Connect screen and a
//! second password prompt for the network already in hand.

use super::*;
use screens::{Facts, pulse};

/// The program whose view is Account (the account, its keys, agents and
/// invites), which the account menu opens.
const ACCOUNT_VIEW: &str = "module-registry";

impl DesktopWindow {
    /// What the breath means (the NodeStatus board): in sync or not, and
    /// the node's own numbers, `padding: 7px 16px` each.
    pub(super) fn node_menu(
        &self,
        state: &Facts,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
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
            for (key, value, code) in node_facts(node, state.block_age, ok) {
                rows.push(row(key, value, code));
                if key == "Epoch" {
                    rows.push(
                        div()
                            .mx(px(16.))
                            .mt(px(2.))
                            .mb(px(8.))
                            .h(px(2.))
                            .bg(ink.line)
                            .child(div().h_full().w(relative(epoch_share(node))).bg(ink.ink)),
                    );
                }
            }
        }
        // down: when it was last heard, and that the numbers are from then
        let heard = match (ok, &state.node) {
            (false, Some(node)) => Some(
                div().px(px(16.)).pb(px(10.)).child(
                    sans(400, 13.)
                        .text_color(ink.muted)
                        .child(last_heard(node.height, state.block_age)),
                ),
            ),
            _ => None,
        };
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
            .children(heard)
            .child(div().h(px(1.)).mb(px(6.)).bg(ink.line))
            .children(rows)
            .child(div().h(px(1.)).my(px(6.)).bg(ink.line))
            // the bottom bar's Retry now: the same status poll, now
            .children((!ok).then(|| {
                self.menu_row(
                    "node-retry",
                    "Retry now",
                    self.dispatching(|| Message::Tick),
                    cx,
                )
            }))
            .child(copy)
            .child(self.menu_row(
                "node-switch",
                "Switch node…",
                self.dispatching(|| Message::ToggleNetworkMenu),
                cx,
            ));
        self.hanging(crate::Popover::Node, 340., body, window, cx)
    }

    /// Who is signed in, and the ways out.
    pub(super) fn account_menu(
        &self,
        state: &Facts,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
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
        // Account shows a key with no account too: its key, and Create account
        let mut rows = div().flex().flex_col().child(self.menu_row(
            "account-view",
            "Account",
            self.dispatching(|| Message::SelectView(crate::runtime::intern(ACCOUNT_VIEW))),
            cx,
        ));
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
                        // as the node menu's: the networks this device reached
                        self.dispatching(|| Message::ToggleNetworkMenu),
                        cx,
                    )),
            );
        self.hanging(crate::Popover::Account, 300., body, window, cx)
    }

    /// The switcher's menu: each node this device reached (its network, its
    /// host, a check on the one in hand, and which one is a different chain
    /// sharing a name), then "Add a network" for the Connect screen. A
    /// click anywhere outside, or Escape, closes it.
    pub(super) fn network_menu(
        &self,
        state: &Facts,
        narrow: bool,
        window: &Window,
    ) -> gpui_kit::AnyElement {
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
            let network = entry.name();
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
        let at = self.under_button(crate::Overlay::Network, gpui_kit::Anchor::TopLeft, window);
        self.overlay(
            "network-menu",
            Role::Menu,
            "Networks",
            crate::Overlay::Network,
            false,
            &ink,
            |card| {
                at.child(
                    card.w(px(if narrow { 300. } else { 400. }))
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
                        ),
                )
                .into_any_element()
            },
        )
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

/// The popover's rows in the words and formats Nodes uses: label, value,
/// and whether the value is mono. Last block only while it answers.
fn node_facts(
    node: &crate::backend::NodeStatus,
    block_age: i64,
    answering: bool,
) -> Vec<(&'static str, String, bool)> {
    let mut facts = vec![("Height", grouped(node.height), true)];
    if answering {
        let last = match block_age {
            ..=0 => "just now".to_owned(),
            age => format!("{} ago", ago(age)),
        };
        facts.push(("Last block", last, false));
    }
    facts.push((
        "Block time",
        format!("{:.1} s", node.block_time_ms as f64 / 1000.),
        false,
    ));
    let (into, of) = epoch_into(node);
    facts.extend([
        ("Epoch", format!("{} · {into} / {of}", node.epoch), true),
        ("Tip", short_hex(&node.tip), true),
        ("State root", short_hex(&node.root.0), true),
        ("Node identity", short_hex(&node.identity), true),
        ("Contract version", node.contract.to_string(), true),
        ("Chain founded", founded(node.time), false),
    ]);
    facts
}

/// How many blocks into its epoch the tip is, and the epoch's length. A
/// block closes epoch `e` when `height + 1` reaches `(e + 1) × length`, so
/// right after a close it reads 0.
fn epoch_into(node: &crate::backend::NodeStatus) -> (u64, u64) {
    match node.epoch_length {
        0 => (0, 0),
        length => ((node.height + 1) % length, length),
    }
}

fn epoch_share(node: &crate::backend::NodeStatus) -> f32 {
    match epoch_into(node) {
        (_, 0) => 0.,
        (into, of) => into as f32 / of as f32,
    }
}

/// The down popover's line: `Last heard at block 4,295, 38s ago. The
/// numbers below are what it said then.`
fn last_heard(height: u64, block_age: i64) -> String {
    format!(
        "Last heard at block {}, {} ago. The numbers below are what it said then.",
        grouped(height),
        ago(block_age)
    )
}

/// An age in seconds as Nodes writes it (the view SDK's `design::ago`):
/// `13s`, `3m`, `4h`, `5d`.
fn ago(seconds: i64) -> String {
    match seconds.max(0) {
        seconds @ ..60 => format!("{seconds}s"),
        seconds @ 60..3_600 => format!("{}m", seconds / 60),
        seconds @ 3_600..86_400 => format!("{}h", seconds / 3_600),
        seconds => format!("{}d", seconds / 86_400),
    }
}

/// The chain's founding time as a day: `27 Sep 2026`. The node's clock
/// reads in milliseconds; a value that small is taken as seconds.
fn founded(time: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
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
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
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
        assert_eq!(founded(0), "1 Jan 1970");
        assert_eq!(founded(1_790_121_600), "23 Sep 2026");
        assert_eq!(founded(1_790_121_600_000), "23 Sep 2026");
    }

    fn node() -> crate::backend::NodeStatus {
        crate::backend::NodeStatus {
            network: "testkit".into(),
            time: 1_790_121_600_000,
            block_time_ms: 1000,
            epoch_length: 64,
            height: 4295,
            tip: [0xb2; 32],
            root: abi::Root([0x41; 32]),
            epoch: 67,
            identity: vec![0x24; 32],
            contract: 1,
            genesis: [0; 32],
        }
    }

    /// Nodes' words: Node identity, Contract version and its number, the
    /// day the chain was founded, the epoch by core's rule.
    #[test]
    fn the_popover_says_what_nodes_says() {
        let facts: Vec<(&str, String)> = node_facts(&node(), 0, true)
            .into_iter()
            .map(|(key, value, _)| (key, value))
            .collect();
        let expected = [
            ("Height", "4,295"),
            ("Last block", "just now"),
            ("Block time", "1.0 s"),
            ("Epoch", "67 · 8 / 64"),
            ("Tip", "b2b2b2b2…b2b2"),
            ("State root", "41414141…4141"),
            ("Node identity", "24242424…2424"),
            ("Contract version", "1"),
            ("Chain founded", "23 Sep 2026"),
        ];
        let expected: Vec<(&str, String)> = expected
            .iter()
            .map(|(key, value)| (*key, value.to_string()))
            .collect();
        assert_eq!(facts, expected);
        // the block that closes epoch 67 leaves the tip 0 into epoch 68
        let closed = crate::backend::NodeStatus {
            height: 4351,
            epoch: 68,
            ..node()
        };
        assert_eq!(epoch_into(&closed), (0, 64));
    }

    /// Down: no Last block, and the line that says when it was last heard.
    #[test]
    fn a_node_not_answering_says_when_it_was_last_heard() {
        let keys: Vec<&str> = node_facts(&node(), 38, false)
            .into_iter()
            .map(|(key, _, _)| key)
            .collect();
        assert!(!keys.contains(&"Last block"), "{keys:?}");
        assert_eq!(
            last_heard(4295, 38),
            "Last heard at block 4,295, 38s ago. The numbers below are what it said then."
        );
        // the same age format as Nodes: `13s ago`, then minutes
        assert_eq!(node_facts(&node(), 13, true)[1].1, "13s ago");
        assert!(last_heard(4295, 150).starts_with("Last heard at block 4,295, 2m ago."));
        assert_eq!(ago(-1), "0s");
    }
}
