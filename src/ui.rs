//! The app's state and its reducer: the desk, its panes, the notification
//! centre and the windows. Reaching a node and signing in are
//! `shell::entities::{Session, Account}`.

mod app;
mod desk;
pub(crate) mod layout;
mod notify;
mod panes;
pub(crate) mod task;
mod update;

#[cfg(test)]
mod desk_tests;
#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use app::*;
pub(crate) use update::BeatFace;
