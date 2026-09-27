//! The app's help, in a window on the desk: what the desk is and the keys
//! that run it. A new account opens on it; ⌘/, ⌘K's Help and the empty
//! desk's Help bring it back.

use super::*;

impl DesktopWindow {
    pub(super) fn help_view(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let section = |title: &'static str| {
            mono(400, 12.)
                .text_color(ink.muted)
                .pt(px(24.))
                .pb(px(8.))
                .child(title)
        };
        let line = |text: &'static str| note(text, ink.ink);
        let keys = [
            (
                chord_label("N"),
                "New window: type to find a program for it",
            ),
            (chord_label("K"), "Search programs, networks and actions"),
            (chord_label("W"), "Close the window"),
            (chord_label("D"), "Split the window, left and right"),
            (chord_label("⇧D"), "Split the window, top and bottom"),
            (chord_label("`"), "Next window"),
            (chord_label("1–9"), "Go to a window by its place"),
            (chord_label("/"), "This help"),
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
                        .w(px(88.))
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
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(560.))
                    .px(px(32.))
                    .py(px(32.))
                    .flex()
                    .flex_col()
                    .child(h1("Welcome to Ducktape", &ink))
                    .child(div().pt(px(12.)).child(lead(
                        "Everything here runs on your network. The bar at the top lists \
                         the programs it runs; each one opens in a window on this desk.",
                        &ink,
                    )))
                    .child(section("THE DESK"))
                    .child(line(
                        "Move a window by its title bar and size it from any edge. \
                         Double-click a title bar to fill the desk, and again to put it back.",
                    ))
                    .child(section("THE BAR"))
                    .child(line(
                        "The network's name, at the left, switches networks. Search, \
                         notifications and your account sit at the right.",
                    ))
                    .child(section("KEYS"))
                    .children(keys),
            )
            .into_any_element()
    }
}
