//! The app's help, in a window on the desk: how to open a program, how the
//! windows and the bar work, the account, and the keys. A new account
//! opens on it, greeted; ⌘/, ⌘K's Help and the empty desk's Help bring it
//! back.

use super::*;

/// The controls Help lists with a chord, each with the key the platform's
/// modifier goes with (⇧ leading it: with Shift): each reports its chord
/// (AX-114). "Fill window" and "Move or size window" are Search's rows.
const CHORDED: [(&str, &str); 5] = [
    ("New window", "N"),
    ("Search", "K"),
    ("Help", "/"),
    ("Fill window", "⇧↩"),
    ("Move or size window", "⇧M"),
];

/// `(control name, chord)` for each control Help lists with a chord, as
/// the platform writes the chord: what the audit is told (AX-114).
pub(crate) fn chords() -> Vec<(String, String)> {
    CHORDED
        .iter()
        .map(|(name, key)| ((*name).to_owned(), chord_label(key)))
        .collect()
}

/// Help's page: greeted (`welcome`, a new account's first sight of the
/// desk) or titled "Ducktape help", in the dark or light ink.
pub(super) fn help_view(welcome: bool, dark: bool) -> gpui_kit::AnyElement {
    use super::ink::*;
    use gpui_kit::*;
    let ink = Ink::of(dark);
    let section = |id: &'static str, title: &'static str| {
        mono(400, 12.)
            .text_color(ink.muted)
            .pt(px(28.))
            .pb(px(8.))
            .child(words(id, title))
    };
    // each paragraph its own id: `<section>/<n>`
    let lines = |section: &'static str, texts: Vec<String>| {
        texts.into_iter().enumerate().map(move |(n, text)| {
            let id = SharedString::from(format!("{section}/{n}"));
            div().pb(px(6.)).child(note(id, text, ink.ink))
        })
    };
    let (heading, lead_text) = match welcome {
        true => (
            "Welcome to Ducktape",
            "Your account is ready. The bar across the top lists the programs \
                 your network runs; each one opens in a window on this desk.",
        ),
        false => (
            "Ducktape help",
            "The bar across the top lists the programs your network runs; \
                 each one opens in a window on this desk.",
        ),
    };
    let (n, k, w) = (chord_label("N"), chord_label("K"), chord_label("W"));
    let open = [
        "Click a program's name in the bar. It opens in the window you are \
             in if that one is empty, brings its window to the front if it is \
             already open, and otherwise opens a window of its own."
            .to_owned(),
        format!(
            "{n} opens an empty window: type part of a program's name, pick it \
                 with ↑↓ and open it with ↵.{}",
            match super::layers::CHAT_READY {
                true => " Tab switches what the field searches, Module or Chat.",
                false => "",
            }
        ),
        format!("{k} searches programs, networks and actions from anywhere."),
    ];
    let windows = [
        "Drag a title bar to move a window, and any edge or corner to size it. \
             Double-click a title bar to fill the desk; again to put it back."
            .to_owned(),
        "In a title bar, + opens another window of the same program, the \
             arrow moves the window out into one of its own (its arrow brings it \
             back), and × closes it."
            .to_owned(),
        format!(
            "{} fills the desk with the window you are in; {} lets the arrows \
                 move it ({} sizes it).",
            chord_label("⇧↩"),
            chord_label("⇧M"),
            match cfg!(target_os = "macos") {
                true => "⌥",
                false => "Alt",
            }
        ),
    ];
    let bar = [
        "Left to right: the network's name, to switch networks; the programs; \
             Search; the bell, for notifications; the dot, for how your node is \
             doing; your name, for the account menu; and the gear, for Ducktape's \
             settings (appearance, notifications, networks)."
            .to_owned(),
        format!(
            "Shift-click a program's name, or press Shift+{} on it, to show it \
                 in the window you are in.",
            super::pane_hold::keep_key()
        ),
    ];
    let account = [
        "The key that signs for you is kept by this device's system. The \
             account menu, under your name, holds the rest:"
            .to_owned(),
        "Add a device… signs another device in to this account: type the code \
             it shows, and approve only if both show the same fingerprint."
            .to_owned(),
        "Make a recovery key… writes down words that get you back in if you \
             lose every device. Keep them on paper, somewhere safe."
            .to_owned(),
        "Lock puts this device's key away until you unlock it; Switch node… \
             connects to another node."
            .to_owned(),
    ];
    let keys = [
        (n, "An empty window: type to find a program for it"),
        (k, "Search programs, networks and actions"),
        (w, "Close the window"),
        (chord_label("`"), "The next window"),
        (chord_label("1–9"), "A window by its place"),
        (chord_label("⇧↩"), "Fill the desk with this window"),
        (chord_label("⇧M"), "Move this window with the arrows"),
        (chord_label("/"), "This help"),
        ("Esc".to_owned(), "Close search, a menu or settings"),
        (
            "Esc, then Tab".to_owned(),
            "In a text editor, move on (Tab alone indents)",
        ),
        (chord_label("Q"), "Quit"),
    ]
    .into_iter()
    .enumerate()
    .map(|(n, (chord, what))| {
        div()
            .flex()
            .items_baseline()
            .gap(px(16.))
            .py(px(6.))
            .border_b_1()
            .border_color(ink.line)
            .child(
                mono(400, 12.)
                    .w(px(128.))
                    .flex_shrink_0()
                    .text_color(ink.ink)
                    .child(words(SharedString::from(format!("keys/{n}/chord")), chord)),
            )
            .child(
                sans(400, 13.)
                    .text_color(ink.ink)
                    .child(words(SharedString::from(format!("keys/{n}")), what)),
            )
    });
    div()
        .id("help")
        .role(Role::Document)
        .aria_label("Help")
        .size_full()
        // both ways: a narrow window scrolls to what it cuts off
        .overflow_scroll()
        .child(
            div()
                .id("help/page")
                .role(Role::Group)
                .aria_label(heading)
                // auto margins centre it, and never push it past the
                // left edge where no scroll could reach it
                .mx_auto()
                .w_full()
                .min_w(px(360.))
                .max_w(px(560.))
                .px(px(32.))
                .py(px(32.))
                .flex()
                .flex_col()
                .child(h1("heading", heading, &ink))
                .child(div().pt(px(12.)).child(lead("lead", lead_text, &ink)))
                .child(section("open", "OPEN A PROGRAM"))
                .children(lines("open", open.into()))
                .child(section("windows", "WINDOWS"))
                .children(lines("windows", windows.into()))
                .child(section("bar", "THE BAR"))
                .children(lines("bar", bar.into()))
                .child(section("account", "YOUR ACCOUNT"))
                .children(lines("account", account.into()))
                .child(section("keys", "KEYS"))
                .children(keys),
        )
        .into_any_element()
}
