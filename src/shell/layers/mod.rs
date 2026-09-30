//! The layers one OS window stacks: each a view of its own over the
//! shell's entities, cached where it can be.

mod cached;
mod chrome;
#[cfg(test)]
mod chrome_tests;
mod dot;
mod empty_pane;
mod help_pane;
mod panes;
mod root;
#[cfg(test)]
mod root_tests;
mod screens;
#[cfg(test)]
pub(super) mod tests;
mod toast;

pub(crate) use cached::cached_unless_a11y;
pub(super) use chrome::{BAR, Chrome, tab_label};
pub(super) use dot::StatusDot;
pub(super) use empty_pane::{CHAT_READY, CONTEXT, EmptyPane};
pub(super) use help_pane::HelpPane;
pub(super) use panes::{PaneLayer, PaneView};
pub(super) use root::WindowRoot;
pub(super) use screens::{Screens, dialog_fit};
pub(super) use toast::ToastView;
