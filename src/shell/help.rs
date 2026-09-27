//! The app's help, in a window on the desk: how to open a program, how the
//! windows and the bar work, the account, and the keys. A new account
//! opens on it, greeted; ⌘/, ⌘K's Help and the empty desk's Help bring it
//! back.

use super::*;

/// How the platform writes a chord with Shift: "⌘⇧D" on a Mac, "Ctrl
/// Shift D" elsewhere.
fn shift_chord_label(key: &str) -> String {
    match cfg!(target_os = "macos") {
        true => format!("⌘⇧{key}"),
        false => format!("Ctrl Shift {key}"),
    }
}

impl DesktopWindow {
    pub(super) fn help_view(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let welcome = self.model.read(cx).state.welcome;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let section = |title: &'static str| {
            mono(400, 12.)
                .text_color(ink.muted)
                .pt(px(28.))
                .pb(px(8.))
                .child(title)
        };
        let line = |text: String| div().pb(px(6.)).child(note(text, ink.ink));
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
        let (n, k, w, d) = (
            chord_label("N"),
            chord_label("K"),
            chord_label("W"),
            chord_label("D"),
        );
        let open = [
            "Click a program's name in the bar. It opens in the window you are \
             in if that one is empty, brings its window to the front if it is \
             already open, and otherwise opens a window of its own."
                .to_owned(),
            format!(
                "{n} opens an empty window: type part of a program's name, pick it \
                 with ↑↓ and open it with ↵. Tab switches what the field searches, \
                 Module or Chat; Chat, for talking to an agent, is coming soon."
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
                "{d} splits the window you are in, left and right; {} top and \
                 bottom. The new half opens empty.",
                shift_chord_label("D")
            ),
        ];
        let bar = [
            "Left to right: the network's name, to switch networks; the programs; \
             Search; the bell, for notifications; the dot, for how your node is \
             doing; your name, for the account menu; and the gear, for Ducktape's \
             settings (appearance, notifications, networks)."
                .to_owned(),
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
            (d, "Split the window, left and right"),
            (shift_chord_label("D"), "Split the window, top and bottom"),
            (chord_label("`"), "The next window"),
            (chord_label("1–9"), "A window by its place"),
            (chord_label("/"), "This help"),
            ("Esc".to_owned(), "Close search, a menu or settings"),
            (chord_label("Q"), "Quit"),
        ]
        .map(|(chord, what)| {
            div()
                .flex()
                .items_baseline()
                .gap(px(16.))
                .py(px(6.))
                .border_b_1()
                .border_color(ink.line)
                .child(
                    mono(400, 12.)
                        .w(px(104.))
                        .flex_shrink_0()
                        .text_color(ink.ink)
                        .child(chord),
                )
                .child(sans(400, 13.).text_color(ink.ink).child(what))
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
                    .child(h1(heading, &ink))
                    .child(div().pt(px(12.)).child(lead(lead_text, &ink)))
                    .child(section("OPEN A PROGRAM"))
                    .children(open.map(line))
                    .child(section("WINDOWS"))
                    .children(windows.map(line))
                    .child(section("THE BAR"))
                    .children(bar.map(line))
                    .child(section("YOUR ACCOUNT"))
                    .children(account.map(line))
                    .child(section("KEYS"))
                    .children(keys),
            )
            .into_any_element()
    }
}
