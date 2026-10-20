//! Tab is the page's while it has the keys. The kit's window root binds
//! Tab and Shift+Tab to moving gpui's focus, and a binding runs before a
//! key reaches the page, so Tab never got to CEF. In the page's own key
//! context (deeper than the root's, so it wins) they are these actions,
//! which hand the key to CEF; when the page's focus runs off its end, CEF
//! says so (`on_take_focus`) and gpui's focus moves on.

use gpui_kit::{App, KeyBinding, Keystroke};

gpui_kit::actions!(web_page, [Tab, TabPrev]);

pub(crate) const CONTEXT: &str = "WebPage";

pub(crate) fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", Tab, Some(CONTEXT)),
        KeyBinding::new("shift-tab", TabPrev, Some(CONTEXT)),
    ]);
}

pub(crate) fn tab(back: bool) -> Keystroke {
    Keystroke::parse(if back { "shift-tab" } else { "tab" }).expect("a keystroke")
}
