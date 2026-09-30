//! The app's own state (`Ducktape`) and every message that moves it
//! (`AppMessage`). The reducer is update.rs and the per-area files beside
//! it; the shell draws the state. Everything a person does inside a view is
//! the view's, not here; the node in hand and the sign-in are
//! `shell::entities::{Session, Account}`.

use std::collections::BTreeMap;

use crate::backend;
use crate::runtime::Intent;
use crate::shell::WindowKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    System,
    Light,
    Dark,
}

/// Everything the app itself knows: each native window's panes, the
/// program in front, the badges, the notice up. The reducer (update.rs and
/// the files beside it) moves it; the shell draws it. Nothing in here
/// belongs to a program: a view keeps its own.
pub struct Ducktape {
    pub(crate) appearance: Appearance,
    /// The OS says dark; counts under `Appearance::System`.
    pub(crate) system_dark: bool,
    /// Each native window's panes: which view is where, their frames and
    /// focus. The console's is `console_win`'s.
    pub(crate) layouts: BTreeMap<WindowKey, super::layout::Layout>,
    /// Animations: the figures tumble on their own and the status dot
    /// pulses. Off, a figure turns only by hand and the dot holds still.
    pub(crate) motion: bool,
    /// The program in front: the focused pane's, or the one just opened.
    /// The bar highlights it; an untouched console desk opens it.
    pub(crate) active: Option<&'static str>,
    /// Each view's unread count (`host.badge`), on its tab in the bar;
    /// a count of 0 or less takes it off.
    pub(crate) badges: BTreeMap<&'static str, i64>,
    /// The one-line notice up, empty when none.
    pub(crate) toast: String,
    /// `ToastTick`s since the toast was set; it clears itself past 12.
    pub(crate) toast_age: i64,
    /// The main native window, once opened; the launcher and the desk both
    /// live in it.
    pub(crate) console_win: Option<WindowKey>,
    /// The notification centre the bell draws: the app's one, which its
    /// views post into.
    pub(crate) center: crate::runtime::notify::CenterHandle,
    /// The programs the node lists, which the bar's tabs, the empty
    /// window and Search draw and a link is read against: the app's one,
    /// which the node's reads fill.
    pub(crate) roster: crate::runtime::Roster,
}

impl std::fmt::Debug for Ducktape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ducktape")
    }
}

#[derive(Debug)]
pub(crate) enum AppMessage {
    SetAppearance(Appearance),
    /// A Spotlight row picked (it closed as it was): what it does that the
    /// reducer still owns. s11 turns each into the entity call it means.
    Spot(crate::shell::Spot),
    SetMotion(bool),
    /// Show a program on the desk (Spotlight, a menu), as the bar's click
    /// does: into an empty focused window, else the window it is in, else
    /// one of its own.
    SelectView(&'static str),
    /// The app's help, in a window on the desk.
    OpenHelp,
    /// Something done to a window's panes.
    Pane(WindowKey, super::layout::PaneMessage),
    /// A window's desk measured this size (`layers::PaneLayer`, from the
    /// frame's callback); `seed` is the program an untouched desk opens.
    DeskShown {
        window: WindowKey,
        desk: (f32, f32),
        seed: Option<&'static str>,
    },
    ViewEvent(&'static str, Intent),
    /// A link to open. `Desktop::dispatch` reads it against the chain in
    /// hand (`Session`), which the reducer no longer holds.
    OpenLink(String),
    /// The network in hand was left (`SessionEvent::LeftNetwork`): every
    /// desk's panes go (each keeping its measure), and the badges and the
    /// active program with them.
    LeftNetwork,
    /// A notification centre row picked: read, and its link opened.
    NotifyOpen(u64),
    NotifyMarkAllRead,
    NotifyClearRead,
    /// A view's permission bar, or its row in Settings.
    NotifyPermission(&'static str, crate::runtime::notify::Permission),
    NotifyNotNow(&'static str),
    SetNotifyBanners(bool),
    SetNotifyInFront(bool),
    SetNotifyBurst(u32),
    ShowToast(String),
    DismissToast,
    ToastTick,
    WallTick,
    ConsoleOpened(WindowKey),
    WindowWasClosed(WindowKey),
    WindowFocused,
    WindowUnfocused(WindowKey),
    /// The console brought forward, or opened if there is none (the tray's
    /// Open; a node answering).
    TrayOpen,
    TrayQuit,
}

impl Ducktape {
    /// The state at launch.
    pub(crate) fn boot() -> Self {
        Ducktape {
            appearance: backend::load_appearance(),
            system_dark: false,
            layouts: BTreeMap::new(),
            motion: backend::load_motion(),
            active: None,
            badges: BTreeMap::new(),
            toast: String::new(),
            toast_age: 0,
            console_win: None,
            center: crate::runtime::notify::center().clone(),
            roster: crate::runtime::roster().clone(),
        }
    }
}
