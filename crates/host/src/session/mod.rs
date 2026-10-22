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
mod hand;
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
pub use hand::Hand;
#[cfg(any(test, feature = "test-support"))]
pub use node::{HeadsSource, StatusSource};
pub use node::{Session, SessionEvent, SessionState};
pub use notifications::{Notifications, permission};
pub use prefs::{DYNAMIC_TYPE_BODY, Prefs, TextSize, text_scale};
pub use rail::{Opening, Rail};
pub use screen::{AccountStep, Screen};
pub use seats::{Place, Seats, SeatsEvent};
pub use slice::{Observed, Slice, on_runtime, spawn_on_runtime};
pub use toast::Toast;
#[cfg(test)]
mod node_tests;
#[cfg(test)]
mod shared_tests;

use crate::runtime::notify::CenterHandle;
use crate::runtime::{Network, Roster};
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, Entity};
use std::sync::Arc;

/// What every window draws from, whichever network it shows: the toast,
/// the prefs, and the network in hand (`Hand`).
#[derive(Clone)]
pub struct Common {
    pub toast: Entity<Toast>,
    pub prefs: Entity<Slice<Prefs>>,
    pub hand: Entity<Hand>,
    center: CenterHandle,
}

impl Common {
    pub fn new(center: CenterHandle, changes: mpsc::UnboundedReceiver<()>, cx: &mut App) -> Self {
        let prefs = cx.new(|_| Slice::new(Prefs::load()));
        let text_size = prefs.read(cx).get().text_size;
        cx.set_global(text_size);
        let toast = cx.new(|_| Toast::new());
        let hand = cx.new(|cx| Hand::new(changes, cx));
        Self {
            toast,
            prefs,
            hand,
            center,
        }
    }
}

/// One network's entities: its runtime `Network`, the node in hand on it
/// (`Session`), its chain, account, screen, rail, notifications and seats.
#[derive(Clone)]
pub struct Net {
    pub network: Arc<Network>,
    pub session: Entity<Session>,
    pub chain: Entity<Chain>,
    pub account: Entity<Account>,
    pub screen: Entity<Slice<Screen>>,
    pub rail: Entity<Rail>,
    pub notifications: Entity<Notifications>,
    pub seats: Entity<Seats>,
}

impl PartialEq for Net {
    fn eq(&self, other: &Self) -> bool {
        self.session == other.session
    }
}

impl Net {
    pub fn new(common: &Common, network: Arc<Network>, in_hand: bool, cx: &mut App) -> Self {
        let roster = network.roster().clone();
        Self::over(common, network, roster, in_hand, cx)
    }

    fn over(
        common: &Common,
        network: Arc<Network>,
        roster: Roster,
        in_hand: bool,
        cx: &mut App,
    ) -> Self {
        let chain = cx.new(|_| Chain::default());
        let screen = cx.new(|_| Slice::new(Screen::Connect));
        let account = cx.new(|_| Account::new(screen.clone(), common.prefs.clone()));
        let session = {
            let (chain, account, screen) = (chain.clone(), account.clone(), screen.clone());
            let (center, network, roster) =
                (common.center.clone(), network.clone(), roster.clone());
            cx.new(|_| {
                let mut session = Session::new(chain, account, screen, center, network, roster);
                session.hand_notifiers(in_hand);
                session
            })
        };
        let wake = common.hand.update(cx, |hand, _| hand.wake());
        let rail = cx.new(|cx| Rail::new(network.clone(), roster, wake, &session, cx));
        let notifications = cx.new(|_| Notifications::new(common.center.clone()));
        notifications.update(cx, |notifications, cx| notifications.follow(&session, cx));
        common
            .toast
            .update(cx, |toast, cx| toast.follow(&session, &account, cx));
        let seats = {
            let network = network.clone();
            cx.new(|cx| {
                Seats::new(
                    &session,
                    &account,
                    &rail,
                    &notifications,
                    network,
                    in_hand,
                    cx,
                )
            })
        };
        if in_hand {
            common
                .hand
                .update(cx, |hand, cx| hand.take(&session, &seats, cx));
        }
        Self {
            network,
            session,
            chain,
            account,
            screen,
            rail,
            notifications,
            seats,
        }
    }
}

/// What one window draws from: the app-wide entities (`Common`) and its
/// network's (`Net`), each under the name it had while the app held one
/// network.
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
    pub common: Common,
    pub net: Net,
}

impl Shared {
    pub fn of(common: &Common, net: &Net) -> Self {
        Self {
            session: net.session.clone(),
            chain: net.chain.clone(),
            account: net.account.clone(),
            screen: net.screen.clone(),
            rail: net.rail.clone(),
            notifications: net.notifications.clone(),
            toast: common.toast.clone(),
            prefs: common.prefs.clone(),
            seats: net.seats.clone(),
            common: common.clone(),
            net: net.clone(),
        }
    }

    pub fn new(
        network: Arc<Network>,
        center: CenterHandle,
        changes: mpsc::UnboundedReceiver<()>,
        cx: &mut App,
    ) -> Self {
        let common = Common::new(center, changes, cx);
        let net = Net::new(&common, network, true, cx);
        Self::of(&common, &net)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(roster: Roster, center: CenterHandle, cx: &mut App) -> Self {
        let common = Common::new(center, mpsc::unbounded().1, cx);
        let net = Net::over(&common, crate::runtime::in_hand().clone(), roster, true, cx);
        net.session
            .update(cx, |session, _| session.heads_through(tests::quiet()));
        Self::of(&common, &net)
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
                crate::runtime::in_hand().clone(),
                Roster::default(),
            );
            session.apart_from_runtime();
            session.heads_through(quiet());
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
        answer: impl Fn(usize) -> Result<crate::backend::NodeStatus, noded_client::Error> + 'static,
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

    pub(crate) fn quiet() -> HeadsSource {
        use futures::StreamExt as _;
        std::rc::Rc::new(|| futures::stream::pending().boxed_local())
    }

    type Feed = futures::channel::mpsc::UnboundedSender<
        Result<noded_client::wire::Head, noded_client::Error>,
    >;

    /// A test's heads socket: each head it is told, and its end when cut.
    pub struct Heads {
        feed: std::rc::Rc<std::cell::RefCell<Option<Feed>>>,
        opens: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl Heads {
        pub fn send(&self, height: u64) {
            let head = noded_client::wire::Head {
                height,
                id: [0; 32],
                time: 0,
            };
            if let Some(feed) = self.feed.borrow().as_ref() {
                let _ = feed.unbounded_send(Ok(head));
            }
        }

        pub fn cut(&self) {
            self.feed.borrow_mut().take();
        }

        pub fn lose(&self) {
            let lost = noded_client::Error::Transport("the socket closed".into());
            if let Some(feed) = self.feed.borrow().as_ref() {
                let _ = feed.unbounded_send(Err(lost));
            }
        }

        pub fn opens(&self) -> usize {
            self.opens.get()
        }
    }

    pub fn heads() -> (HeadsSource, Heads) {
        use futures::StreamExt as _;
        let feed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let opens = std::rc::Rc::new(std::cell::Cell::new(0));
        let (fed, opened) = (feed.clone(), opens.clone());
        let source: HeadsSource = std::rc::Rc::new(move || {
            let (tell, heard) = futures::channel::mpsc::unbounded();
            *fed.borrow_mut() = Some(tell);
            opened.set(opened.get() + 1);
            heard.boxed_local()
        });
        (source, Heads { feed, opens })
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
