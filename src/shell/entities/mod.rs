//! The shell's state as entities the layers observe: the app-wide ones
//! every shell shares (`ducktape_host::session::Shared`: `Session`,
//! `Chain`, `Account`, `Screen`, `Rail`, `Notifications`, `Toast`,
//! `Prefs`, `Seats`) and the desktop's own, one set per window (`Windows`
//! holds those: `Desk`, `Overlays`, `Spotlight`, `Front`, `DotSlot`), each
//! written by its own methods and by nothing else. `DeskSeats` tells the
//! shared `Seats` which panes the windows' desks hold.

mod arrangement;
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
mod arrangement_tests;
#[cfg(test)]
mod desk_tests;
#[cfg(test)]
mod windows_tests;

use gpui_kit::{App, AppContext as _, Entity};

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
    pub(crate) fn new(shared: Shared, cx: &mut App) -> Self {
        let windows = cx.new(|cx| Windows::new(shared.clone(), cx));
        let desk_seats = cx.new(|cx| DeskSeats::new(&shared.seats, &windows, cx));
        windows.update(cx, |windows, _| windows.followed_by(desk_seats));
        Self { shared, windows }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        roster: crate::runtime::Roster,
        center: crate::runtime::notify::CenterHandle,
        cx: &mut App,
    ) -> Self {
        crate::runtime::shared_for_test();
        Self::new(Shared::for_test(roster, center, cx), cx)
    }
}

#[derive(Clone)]
pub(crate) struct WindowEntities {
    pub(crate) kind: crate::shell::WindowKind,
    pub(crate) desk: Entity<Desk>,
    pub(crate) overlays: Entity<Overlays>,
    pub(crate) spotlight: Entity<Spotlight>,
    pub(crate) front: Entity<Slice<Front>>,
    pub(crate) dot: Entity<DotSlot>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) use ducktape_host::session::tests::{notifies, session, source, status};

    pub(crate) fn entities(cx: &mut App) -> Entities {
        let entities = Entities::for_test(Default::default(), Default::default(), cx);
        entities.screen.update(cx, |screen, cx| {
            screen.set(Screen::Desk, cx);
        });
        entities
    }

    pub(crate) fn desk_of(
        app: &Entities,
        key: crate::runtime::WindowKey,
        cx: &gpui_kit::TestAppContext,
    ) -> Entity<Desk> {
        app.windows.read_with(cx, |windows, _| {
            windows.own(key).expect("its window").desk.clone()
        })
    }

    pub(crate) fn programs(
        app: &Entities,
        key: crate::runtime::WindowKey,
        cx: &gpui_kit::TestAppContext,
    ) -> Vec<&'static str> {
        desk_of(app, key, cx).read_with(cx, |desk, _| {
            desk.get().panes.iter().map(|pane| pane.program).collect()
        })
    }

    pub(crate) fn active(app: &Entities, cx: &gpui_kit::TestAppContext) -> Option<&'static str> {
        app.windows.read_with(cx, |windows, _| windows.active())
    }
}
