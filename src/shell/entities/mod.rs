//! The shell's state as entities the layers observe: the app-wide ones
//! every shell shares (`ducktape_host::session::Shared`: `Session`,
//! `Chain`, `Account`, `Screen`, `Rail`, `Notifications`, `Toast`,
//! `Prefs`, `Seats`) and the desktop's own, one set per window (`Windows`
//! holds those: `Desk`, `Overlays`, `Spotlight`, `Front`, `DotSlot`), each
//! written by its own methods and by nothing else. `DeskSeats` tells the
//! shared `Seats` which panes the windows' desks hold.

mod desk;
mod dot;
mod front;
mod overlays;
mod seats;
mod spotlight;
mod windows;

pub(crate) use desk::Desk;
pub(crate) use dot::DotSlot;
#[cfg(test)]
pub(crate) use ducktape_host::session::AccountStep;
#[cfg(test)]
pub(crate) use ducktape_host::session::STATUS_EVERY;
pub(crate) use ducktape_host::session::{
    Account, AccountEvent, AccountState, Chain, Notifications, Observed, Prefs, Rail, Screen,
    Seats, Secret, Session, SessionEvent, SessionState, Shared, Slice, Toast, permission,
    spawn_on_runtime,
};
pub(crate) use front::{Front, Listed, Popped};
pub(crate) use overlays::{Overlay, Overlays, Popover, SettingsPage, Spot, SpotRow};
pub(crate) use seats::DeskSeats;
pub(crate) use spotlight::Spotlight;
pub(crate) use windows::{Windows, open_banner, posted};
#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod desk_tests;
#[cfg(test)]
mod windows_tests;

use gpui_kit::{App, AppContext as _, Entity};

/// Every app-wide entity. Each window's own are `Windows`' (`own`).
#[derive(Clone)]
pub(crate) struct Entities {
    shared: Shared,
    pub(crate) windows: Entity<Windows>,
}

impl std::ops::Deref for Entities {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.shared
    }
}

impl Entities {
    /// The desktop's windows over the shared entities the host made
    /// (`ducktape_host::start`), the seats following them.
    pub(crate) fn new(shared: Shared, cx: &mut App) -> Self {
        let windows = cx.new(|cx| Windows::new(shared.clone(), cx));
        let desk_seats = cx.new(|cx| DeskSeats::new(&shared.seats, &windows, cx));
        windows.update(cx, |windows, _| windows.followed_by(desk_seats));
        Self { shared, windows }
    }

    /// As `new`, over shared entities on a roster and a centre of the
    /// test's own, with a rail no loader thread wakes.
    #[cfg(test)]
    pub(crate) fn for_test(
        roster: crate::runtime::Roster,
        center: crate::runtime::notify::CenterHandle,
        cx: &mut App,
    ) -> Self {
        Self::new(Shared::for_test(roster, center, cx), cx)
    }
}

/// One OS window's own entities.
#[derive(Clone)]
pub(crate) struct WindowEntities {
    /// The console (a desk of framed windows under a bar) or a window one
    /// view fills: what its panes' bodies are measured from (`DeskSeats`),
    /// as its layer draws them.
    pub(crate) kind: crate::shell::WindowKind,
    pub(crate) desk: Entity<Desk>,
    /// What is open over it (only the console's opens anything), and ⌘K's
    /// text: their methods write them.
    pub(crate) overlays: Entity<Overlays>,
    pub(crate) spotlight: Entity<Spotlight>,
    /// Derived from `desk` (`Front::of_desk`); never written by hand.
    pub(crate) front: Entity<Slice<Front>>,
    /// Where the bar's status well is: Chrome commits it after each frame.
    pub(crate) dot: Entity<DotSlot>,
}

/// The entities on stores of the test's own, for the entities' own tests.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) use ducktape_host::session::tests::{notifies, session, source, status};

    /// Every entity off a roster and a centre of the test's own, on the
    /// desk with no window open.
    pub(crate) fn entities(cx: &mut App) -> Entities {
        let entities = Entities::for_test(Default::default(), Default::default(), cx);
        entities.screen.update(cx, |screen, cx| {
            screen.set(Screen::Desk, cx);
        });
        entities
    }

    /// Window `key`'s desk.
    pub(crate) fn desk_of(
        app: &Entities,
        key: crate::runtime::WindowKey,
        cx: &gpui_kit::TestAppContext,
    ) -> Entity<Desk> {
        app.windows.read_with(cx, |windows, _| {
            windows.own(key).expect("its window").desk.clone()
        })
    }

    /// The programs window `key`'s desk shows, pane by pane.
    pub(crate) fn programs(
        app: &Entities,
        key: crate::runtime::WindowKey,
        cx: &gpui_kit::TestAppContext,
    ) -> Vec<&'static str> {
        desk_of(app, key, cx).read_with(cx, |desk, _| {
            desk.get().panes.iter().map(|pane| pane.program).collect()
        })
    }

    /// The active program, if any.
    pub(crate) fn active(app: &Entities, cx: &gpui_kit::TestAppContext) -> Option<&'static str> {
        app.windows.read_with(cx, |windows, _| windows.active())
    }
}
