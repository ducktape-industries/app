//! The shell's state as entities the layers observe: app-wide ones and
//! one set per window, each written by its own methods (until then by the
//! bridge in `Desktop::dispatch`, one writer per slice). `Session`,
//! `Chain`, `Account` and `Screen` are written by `Session`'s and
//! `Account`'s own flows; the rest are bridged until s11.

mod account;
mod chain;
mod desk;
mod dot;
mod front;
mod notifications;
mod overlays;
mod prefs;
mod rail;
mod screen;
mod seats;
mod session;
mod slice;
mod spotlight;
mod toast;
mod windows;

pub(crate) use account::{Account, AccountEvent, AccountState, Secret};
pub(crate) use chain::Chain;
pub(crate) use desk::Desk;
pub(crate) use dot::DotSlot;
pub(crate) use front::Front;
pub(crate) use notifications::Notifications;
pub(crate) use overlays::{Overlay, Overlays, Popover, SettingsPage, Spot, SpotRow};
pub(crate) use prefs::Prefs;
pub(crate) use rail::Rail;
pub(crate) use screen::{AccountStep, Screen};
pub(crate) use seats::Seats;
#[cfg(test)]
pub(crate) use session::{LOST_AFTER, STATUS_EVERY, StatusSource};
pub(crate) use session::{Session, SessionEvent, SessionState};
#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod session_tests;
pub(crate) use slice::{Observed, Slice, on_runtime, spawn_on_runtime};
pub(crate) use spotlight::Spotlight;
pub(crate) use toast::Toast;
pub(crate) use windows::Windows;

use crate::runtime::WindowKey;
use gpui_kit::Entity;
use std::collections::BTreeMap;

/// Every app-wide entity, and each window's own, as the `Desktop` holds
/// them.
#[derive(Clone)]
pub(crate) struct Entities {
    pub(crate) session: Entity<Session>,
    pub(crate) chain: Entity<Chain>,
    pub(crate) account: Entity<Account>,
    pub(crate) screen: Entity<Slice<Screen>>,
    pub(crate) rail: Entity<Rail>,
    pub(crate) notifications: Entity<Notifications>,
    pub(crate) toast: Entity<Toast>,
    pub(crate) prefs: Entity<Slice<Prefs>>,
    pub(crate) windows: Entity<Windows>,
    /// Each OS window's own, by its key, made with its window. A closed
    /// window's stay until s11 (`Windows` holds them then).
    pub(crate) by_window: BTreeMap<WindowKey, WindowEntities>,
}

/// One OS window's own entities.
#[derive(Clone)]
pub(crate) struct WindowEntities {
    pub(crate) desk: Entity<Desk>,
    /// What is open over it (only the console's opens anything), and ⌘K's
    /// text: their methods write them.
    pub(crate) overlays: Entity<Overlays>,
    pub(crate) spotlight: Entity<Slice<Spotlight>>,
    /// Derived from `desk` (`Front::of_desk`); never bridged.
    pub(crate) front: Entity<Slice<Front>>,
    /// Where the bar's status well is: Chrome commits it after each frame.
    pub(crate) dot: Entity<DotSlot>,
}

/// The app-wide entities off no reducer, for the entities' own tests.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gpui_kit::{App, AppContext as _};

    /// A session over its own chain, account and screen, on a notification
    /// centre of its own.
    pub(crate) fn session(cx: &mut App) -> Entity<Session> {
        let chain = cx.new(|_| Chain::default());
        let screen = cx.new(|_| Slice::new(Screen::Connect));
        let account = cx.new(|_| Account::new(screen.clone()));
        cx.new(|_| Session::new(chain, account, screen, Default::default()))
    }
}
