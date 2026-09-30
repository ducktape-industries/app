//! The shell's state as entities the layers observe: app-wide ones and
//! one set per window, each written by its own methods (until then by the
//! bridge in `Desktop::dispatch`, one writer per slice).

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

pub(crate) use account::Account;
pub(crate) use chain::Chain;
pub(crate) use desk::Desk;
pub(crate) use dot::DotSlot;
pub(crate) use front::Front;
pub(crate) use notifications::Notifications;
pub(crate) use overlays::{Overlay, Overlays};
pub(crate) use prefs::Prefs;
pub(crate) use rail::Rail;
pub(crate) use screen::{AccountStep, Screen};
pub(crate) use seats::Seats;
pub(crate) use session::Session;
pub(crate) use slice::{Observed, Slice};
pub(crate) use spotlight::Spotlight;
pub(crate) use toast::Toast;
pub(crate) use windows::Windows;

use crate::runtime::WindowKey;
use gpui_kit::Entity;
use std::collections::BTreeMap;

/// Every app-wide entity, and each window's own, as the `Desktop` holds
/// them.
pub(crate) struct Entities {
    pub(crate) session: Entity<Slice<Session>>,
    pub(crate) chain: Entity<Chain>,
    pub(crate) account: Entity<Slice<Account>>,
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
    /// The console: the one window things open over.
    pub(crate) console: bool,
    pub(crate) desk: Entity<Desk>,
    pub(crate) overlays: Entity<Overlays>,
    pub(crate) spotlight: Entity<Slice<Spotlight>>,
    /// Derived from `desk` (`Front::of_desk`); never bridged.
    #[cfg_attr(not(test), expect(dead_code, reason = "Chrome reads it from s6b"))]
    pub(crate) front: Entity<Slice<Front>>,
    #[expect(dead_code, reason = "Chrome's dot probe commits it from s6b")]
    pub(crate) dot: Entity<DotSlot>,
}
