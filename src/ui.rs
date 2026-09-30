//! The app's state and what is left of its reducer (the wall clock's
//! beat), and the pure pane geometry (`layout`). The desk, its panes, the
//! windows, the notification centre, reaching a node and signing in are
//! `shell::entities`.

mod app;
pub(crate) mod layout;
pub(crate) mod task;
mod update;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use app::*;
pub(crate) use update::BeatFace;
