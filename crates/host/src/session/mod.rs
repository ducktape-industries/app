//! The app-wide state every shell draws from, as entities the shell's
//! layers observe, each written by its own methods and by nothing else:
//! `Session` (the node in hand), `Chain`, `Account` (this device's key and
//! the account it holds), `Screen` (which launcher step, or the desk),
//! `Rail` (the programs the network lists), `Notifications`, `Toast`,
//! `Prefs` and `Seats` (every placed view pane's seat). A shell adds its
//! own (the desktop: its windows, each one's desk, overlays, spotlight)
//! and drives the desktop-only sign-in steps through `Account`'s
//! callbacks (`src/shell/sign_in.rs`).

mod account;
mod chain;
mod node;
mod notifications;
mod prefs;
mod rail;
mod screen;
mod seats;
mod slice;
mod toast;

pub use account::{Account, AccountEvent, AccountState, Secret};
pub use chain::Chain;
#[cfg(any(test, feature = "test-support"))]
pub use node::{STATUS_EVERY, StatusSource};
pub use node::{Session, SessionEvent, SessionState};
pub use notifications::{Notifications, permission};
pub use prefs::Prefs;
pub use rail::{Opening, Rail};
pub use screen::{AccountStep, Screen};
pub use seats::{Place, Seats, SeatsEvent};
pub use slice::{Observed, Slice, on_runtime, spawn_on_runtime};
pub use toast::Toast;
#[cfg(test)]
mod node_tests;

use crate::runtime::Roster;
use crate::runtime::notify::CenterHandle;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, Entity};

/// Every app-wide entity: what a shell's windows are made over, and what
/// every window's root reads.
#[derive(Clone)]
pub struct Shared {
    pub session: Entity<Session>,
    pub chain: Entity<Chain>,
    pub account: Entity<Account>,
    pub screen: Entity<Slice<Screen>>,
    pub rail: Entity<Rail>,
    pub notifications: Entity<Notifications>,
    pub toast: Entity<Toast>,
    pub prefs: Entity<Slice<Prefs>>,
    pub seats: Entity<Seats>,
}

impl Shared {
    /// Every app-wide entity, off a roster, a notification centre and this
    /// device's prefs; the rail's rows read now and again on every message
    /// `changes` brings.
    pub fn new(
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
            let roster = roster.clone();
            cx.new(|_| Session::new(chain, account, screen, center.clone(), roster))
        };
        let rail = cx.new(|cx| Rail::new(roster, changes, &session, cx));
        let notifications = cx.new(|_| Notifications::new(center));
        notifications.update(cx, |notifications, cx| notifications.follow(&session, cx));
        let toast = cx.new(|cx| Toast::new(&session, &account, cx));
        let seats = cx.new(|cx| Seats::new(&session, &account, &rail, &notifications, cx));
        Self {
            session,
            chain,
            account,
            screen,
            rail,
            notifications,
            toast,
            prefs,
            seats,
        }
    }

    /// As `new`, with a rail no loader thread wakes: a test refreshes it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(roster: Roster, center: CenterHandle, cx: &mut App) -> Self {
        Self::new(roster, center, mpsc::unbounded().1, cx)
    }
}

/// The entities on stores of the test's own, for the entities' own tests
/// and a shell's.
#[cfg(any(test, feature = "test-support"))]
pub mod tests {
    use super::*;

    /// A session over its own chain, account and screen, on a notification
    /// centre of its own.
    pub fn session(cx: &mut App) -> Entity<Session> {
        let chain = cx.new(|_| Chain::default());
        let screen = cx.new(|_| Slice::new(Screen::Connect));
        let prefs = cx.new(|_| Slice::new(Prefs::load()));
        let account = cx.new(|_| Account::new(screen.clone(), prefs));
        cx.new(|_| {
            let mut session = Session::new(
                chain,
                account,
                screen,
                Default::default(),
                Roster::default(),
            );
            session.apart_from_runtime();
            session
        })
    }

    /// Counts `entity`'s notifies while the subscription lives.
    pub fn notifies<T: 'static>(
        entity: &Entity<T>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (std::rc::Rc<std::cell::Cell<usize>>, gpui_kit::Subscription) {
        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = seen.clone();
        let observing = cx.update(|cx| cx.observe(entity, move |_, _| count.set(count.get() + 1)));
        (seen, observing)
    }

    /// A status source answering `answer(n)` to its `n`th ask, counting them.
    pub fn source(
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
    pub fn status(height: u64) -> crate::backend::NodeStatus {
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
            contract: noded_client::wire::NODE_CONTRACT,
            genesis: [0; 32],
            roles: crate::backend::test_roles(),
        }
    }
}
