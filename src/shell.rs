//! The native shell: every OS window the app opens, and everything drawn
//! in one that is not a program's view. Nothing a network decides is here.
//!
//! The shell's state is its entities (`entities`): app-wide ones
//! (`Session`, `Account`, `Rail`, `Windows`, …) and one set per window,
//! each written by its own methods. Each OS window's root is a
//! `layers::WindowRoot`, a thin gpui view laying the window's layers out
//! as siblings: the launcher (connect, key, account, recovery screens;
//! `layers::LauncherLayer`) until sign-in, then the desk: the menu bar
//! (`layers::Chrome`), the panes floating under it (`layers::PaneLayer`),
//! the open dialog (⌘K, Settings, "Add a device…": `layers::OverlayLayer`),
//! the node's breath (`layers::StatusDot`) and the footer
//! (`layers::ToastView`). `launch::run` makes the entities and opens the
//! console; from there on, input calls an entity method, the method
//! compares and notifies, and only the layers observing what moved draw.

use crate::ui::layout::{self, PaneMessage};

#[cfg(debug_assertions)]
mod fixtures;
#[cfg(debug_assertions)]
pub(crate) use fixtures::render_tree_fixture;
pub(crate) mod entities;
mod figure;
mod help;
pub(crate) use help::chords;
#[cfg(any(test, feature = "ax-door"))]
pub(crate) use layers::Kept;
mod ink;
mod keys;
mod launch;
mod layers;
pub(in crate::shell) use layers::WindowRoot;
mod pane_drag;
mod pane_hold;
#[cfg(test)]
mod pane_hold_tests;
mod panes;
#[cfg(test)]
pub(in crate::shell) mod panes_tests;
mod screens;
#[cfg(test)]
mod screens_tests;
mod status_bar;
mod windows;

pub(crate) use launch::run;
mod spin;
mod theme;

use crate::fonts::BUNDLED_FACES;
#[cfg(not(target_os = "macos"))]
use crate::fonts::EMOJI_FACE;
use theme::configure_native_theme;

pub(crate) use crate::runtime::WindowKey;

/// What an OS window is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowKind {
    /// The main window: the launcher, then the desk with its menu bar.
    Console,
    /// A pop-out: one pane that left the desk, in a window of its own.
    View { module: &'static str },
}

/// How the platform writes a command chord: "⌘K" on a Mac, "Ctrl K"
/// elsewhere. A key led by ⇧ is with Shift ("⌘⇧M", "Ctrl Shift M"), and ↩
/// is Enter where the key has no such glyph.
pub(crate) fn chord_label(key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let (shift, key) = match key.strip_prefix('⇧') {
        Some(key) => (true, key),
        None => (false, key),
    };
    match (mac, shift) {
        (true, false) => format!("⌘{key}"),
        (true, true) => format!("⌘⇧{key}"),
        (false, false) => format!("Ctrl {key}"),
        (false, true) => format!("Ctrl Shift {}", key.replace('↩', "Enter")),
    }
}
