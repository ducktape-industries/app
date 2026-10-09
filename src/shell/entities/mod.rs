//! The shell's state as entities the layers observe: app-wide ones and
//! one set per window (`Windows` holds those), each written by its own
//! methods and by nothing else. `Session`, `Chain`, `Account` and
//! `Screen` are written by `Session`'s and `Account`'s flows; `Desk`,
//! `Windows`, `Toast`, `Prefs`, `Notifications` and `Rail` by the calls
//! the layers, the keys, the tray and the door make (and `Prefs`' layout
//! by the layout step's Continue, `Account::layout_chosen`).

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
pub(crate) use notifications::{Notifications, permission};
pub(crate) use overlays::{Overlay, Overlays, Popover, SettingsPage, Spot, SpotRow};
pub(crate) use prefs::Prefs;
pub(crate) use rail::Rail;
pub(crate) use screen::{AccountStep, Screen};
pub(crate) use seats::Seats;
#[cfg(test)]
pub(crate) use session::{STATUS_EVERY, StatusSource};
pub(crate) use session::{Session, SessionEvent, SessionState};
#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod desk_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod windows_tests;
pub(crate) use slice::{Observed, Slice, on_runtime, spawn_on_runtime};
pub(crate) use spotlight::Spotlight;
pub(crate) use toast::Toast;
pub(crate) use windows::{Windows, posted};

use crate::runtime::Roster;
use crate::runtime::notify::CenterHandle;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, Entity};

/// Every app-wide entity but the windows: what `Windows` is made over,
/// and what every window's root reads (`Entities` derefs to it).
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) session: Entity<Session>,
    pub(crate) chain: Entity<Chain>,
    pub(crate) account: Entity<Account>,
    pub(crate) screen: Entity<Slice<Screen>>,
    pub(crate) rail: Entity<Rail>,
    pub(crate) notifications: Entity<Notifications>,
    pub(crate) toast: Entity<Toast>,
    pub(crate) prefs: Entity<Slice<Prefs>>,
    pub(crate) seats: Entity<Seats>,
}

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
    /// Every app-wide entity, off a roster, a notification centre and this
    /// device's prefs; the rail's rows read now and again on every message
    /// `changes` brings.
    pub(crate) fn new(
        roster: Roster,
        center: CenterHandle,
        changes: mpsc::UnboundedReceiver<()>,
        cx: &mut App,
    ) -> Self {
        let chain = cx.new(|_| Chain::default());
        let screen = cx.new(|_| Slice::new(Screen::Connect));
        let prefs = cx.new(|_| Slice::new(Prefs::load()));
        let account = cx.new(|_| Account::new(screen.clone(), prefs.clone()));
        let session = {
            let (chain, account, screen) = (chain.clone(), account.clone(), screen.clone());
            cx.new(|_| Session::new(chain, account, screen, center.clone()))
        };
        let rail = cx.new(|cx| Rail::new(roster, changes, &session, cx));
        let notifications = cx.new(|_| Notifications::new(center));
        let toast = cx.new(|cx| Toast::new(&session, &account, cx));
        let seats = cx.new(|cx| Seats::new(&session, &account, &rail, &notifications, cx));
        let shared = Shared {
            session,
            chain,
            account,
            screen,
            rail,
            notifications,
            toast,
            prefs,
            seats,
        };
        let windows = cx.new(|cx| Windows::new(shared.clone(), cx));
        shared
            .seats
            .update(cx, |seats, cx| seats.follow(&windows, cx));
        Self { shared, windows }
    }

    /// As `new`, with a rail no loader thread wakes: a test refreshes it.
    #[cfg(test)]
    pub(crate) fn for_test(roster: Roster, center: CenterHandle, cx: &mut App) -> Self {
        Self::new(roster, center, mpsc::unbounded().1, cx)
    }
}

/// One OS window's own entities.
#[derive(Clone)]
pub(crate) struct WindowEntities {
    pub(crate) desk: Entity<Desk>,
    /// What is open over it (only the console's opens anything), and ⌘K's
    /// text: their methods write them.
    pub(crate) overlays: Entity<Overlays>,
    pub(crate) spotlight: Entity<Slice<Spotlight>>,
    /// Derived from `desk` (`Front::of_desk`); never written by hand.
    pub(crate) front: Entity<Slice<Front>>,
    /// Where the bar's status well is: Chrome commits it after each frame.
    pub(crate) dot: Entity<DotSlot>,
}

/// The app-wide entities on stores of the test's own, for the entities'
/// own tests.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A session over its own chain, account and screen, on a notification
    /// centre of its own.
    pub(crate) fn session(cx: &mut App) -> Entity<Session> {
        let chain = cx.new(|_| Chain::default());
        let screen = cx.new(|_| Slice::new(Screen::Connect));
        let prefs = cx.new(|_| Slice::new(Prefs::load()));
        let account = cx.new(|_| Account::new(screen.clone(), prefs));
        cx.new(|_| Session::new(chain, account, screen, Default::default()))
    }

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

    /// The modules window `key`'s desk shows, pane by pane.
    pub(crate) fn modules(
        app: &Entities,
        key: crate::runtime::WindowKey,
        cx: &gpui_kit::TestAppContext,
    ) -> Vec<&'static str> {
        desk_of(app, key, cx).read_with(cx, |desk, _| {
            desk.get().panes.iter().map(|pane| pane.module).collect()
        })
    }

    /// The active program, if any.
    pub(crate) fn active(app: &Entities, cx: &gpui_kit::TestAppContext) -> Option<&'static str> {
        app.windows.read_with(cx, |windows, _| windows.active())
    }

    /// Counts `entity`'s notifies while the subscription lives.
    pub(crate) fn notifies<T: 'static>(
        entity: &Entity<T>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (std::rc::Rc<std::cell::Cell<usize>>, gpui_kit::Subscription) {
        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = seen.clone();
        let observing = cx.update(|cx| cx.observe(entity, move |_, _| count.set(count.get() + 1)));
        (seen, observing)
    }

    /// A status source answering `answer(n)` to its `n`th ask, counting them.
    pub(crate) fn source(
        answer: impl Fn(usize) -> Result<crate::backend::NodeStatus, String> + 'static,
    ) -> (StatusSource, std::rc::Rc<std::cell::Cell<usize>>) {
        use futures::FutureExt as _;
        let asked = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = asked.clone();
        let source: StatusSource = std::rc::Rc::new(move || {
            let n = count.get();
            count.set(n + 1);
            std::future::ready(answer(n)).boxed_local()
        });
        (source, asked)
    }

    /// The node's answer at `height`, for the tests that feed the session
    /// one.
    pub(crate) fn status(height: u64) -> crate::backend::NodeStatus {
        crate::backend::NodeStatus {
            network: "testkit".into(),
            time: 0,
            block_time_ms: 0,
            epoch_length: 0,
            height,
            tip: [0; 32],
            root: abi::Root([0; 32]),
            epoch: 0,
            identity: Vec::new(),
            contract: crate::backend::noded::NODE_CONTRACT,
            genesis: [0; 32],
        }
    }
}
