//! The layers one OS window stacks: each a view of its own over the
//! shell's entities, cached where it can be.

mod chrome;
#[cfg(test)]
mod chrome_tests;
mod dot;
mod empty_pane;
mod fields;
mod help_pane;
mod launcher;
mod overlays;
mod panes;
mod root;
#[cfg(test)]
mod root_tests;
#[cfg(test)]
pub(super) mod tests;
mod toast;

pub(super) use chrome::{BAR, Chrome, tab_label};
pub(super) use dot::StatusDot;
pub(super) use empty_pane::{CHAT_READY, CONTEXT, EmptyPane};
pub(super) use help_pane::HelpPane;
pub(super) use launcher::{LAUNCHER_SIZE, LauncherLayer};
#[cfg(any(test, feature = "ax-door"))]
pub(crate) use overlays::Kept;
pub(super) use overlays::OverlayLayer;
#[cfg(test)]
pub(super) use panes::{BORDER, TITLE};
pub(super) use panes::{PaneLayer, PaneView, view_body};
pub(super) use root::WindowRoot;
pub(super) use toast::ToastView;
