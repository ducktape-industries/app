//! The layers one OS window stacks: each a view of its own over the
//! shell's entities, cached where it can be.

mod cached;
mod chrome;
#[cfg(test)]
mod chrome_tests;
mod empty_pane;
mod help_pane;
mod panes;

pub(crate) use cached::cached_unless_a11y;
pub(super) use chrome::{Chrome, tab_label};
pub(super) use empty_pane::{CHAT_READY, CONTEXT, EmptyPane};
pub(super) use help_pane::HelpPane;
pub(super) use panes::{PaneLayer, PaneView};
