//! The other listed networks' notices, while the app runs: one followed
//! set per node the Recent list names, never the node in hand
//! (`runtime::follow`, which also refuses the chain in hand and a chain
//! already followed), started one after another and none while a connect
//! is out. A set stops as its node is forgotten or its chain is taken up
//! in hand, and Lock hands every set no key and no account. What a set
//! runs, and how, is the host's; which nodes, and when, is here.

use crate::backend::RecentEndpoint;
use crate::shell::entities::{Account, Session, spawn_on_runtime};
use ducktape_host::runtime::Followed;
use gpui_kit::{App, Context, Entity, Subscription, Task};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// How long a node not followed when asked rests before it is asked again:
/// each ask is a client of its own, so far slower than a set's poll.
// ponytail: one cadence for "down" and "its chain is taken"; a backoff,
// or a client kept per node, if a long Recent list makes it felt
const REST: Duration = Duration::from_secs(30);

pub(crate) struct Follower {
    session: Entity<Session>,
    account: Entity<Account>,
    /// The sets running, by the node each follows.
    running: BTreeMap<String, Followed>,
    /// Nodes asked at least once this run, followed or not.
    asked: BTreeSet<String>,
    /// Nodes not followed when last asked (not answering, or their chain
    /// in hand or followed already): asked again after [`REST`], or as
    /// the chain in hand moves.
    resting: BTreeSet<String>,
    /// The chain in hand as last seen.
    chain: String,
    /// The one follow out.
    out: Option<Task<()>>,
    /// The wait before the resting nodes are asked again.
    rest: Option<Task<()>>,
    /// The lock as last handed to the host's sets (`lock_followed`).
    locked: bool,
    _observing: [Subscription; 2],
}

/// The nodes to follow: the Recent list's, most recent first, each once,
/// never the node in hand.
pub(crate) fn wanted(recent: &[RecentEndpoint], in_hand: &str) -> Vec<String> {
    let mut wanted: Vec<String> = Vec::new();
    for entry in recent {
        if entry.url != in_hand && !wanted.contains(&entry.url) {
            wanted.push(entry.url.clone());
        }
    }
    wanted
}

impl Follower {
    /// Following from now on what `session` lists, for `account`'s lock.
    /// It notifies as each node asked answers or not: a banner's switch
    /// may wait for that (`Windows::open_switched`).
    pub(crate) fn new(
        session: &Entity<Session>,
        account: &Entity<Account>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            session: session.clone(),
            account: account.clone(),
            running: BTreeMap::new(),
            asked: BTreeSet::new(),
            resting: BTreeSet::new(),
            chain: String::new(),
            out: None,
            rest: None,
            // as the host's sets start
            locked: false,
            _observing: [
                cx.observe(session, |this, _, cx| this.sync(cx)),
                cx.observe(account, |this, _, cx| this.lock(cx)),
            ],
        };
        this.lock(cx);
        this.sync(cx);
        this
    }

    /// Whether a node listed is still to be asked: a banner's switch to a
    /// chain no set follows yet waits for it.
    pub(crate) fn asking(&self, cx: &App) -> bool {
        let state = self.session.read(cx).get();
        wanted(&state.recent_endpoints, &state.connected_rpc)
            .iter()
            .any(|node| !self.asked.contains(node))
    }

    /// The sets as the session now lists its nodes: one gone for a node
    /// forgotten or now in hand; then, with no connect and no follow out,
    /// the next node not followed asked.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let state = self.session.read(cx).get().clone();
        // a chain left, or taken up: what was taken may be free now
        if state.chain != self.chain {
            self.chain = state.chain.clone();
            self.resting.clear();
        }
        let wanted = wanted(&state.recent_endpoints, &state.connected_rpc);
        let before = self.running.len();
        // a set whose chain was taken up in hand left as it was
        self.running.retain(|node, followed| {
            wanted.contains(node) && (state.chain.is_empty() || followed.chain() != state.chain)
        });
        if self.running.len() != before {
            cx.notify();
        }
        if state.connecting || self.out.is_some() {
            return;
        }
        let next = wanted
            .into_iter()
            .find(|node| !self.running.contains_key(node) && !self.resting.contains(node));
        let Some(node) = next else {
            return;
        };
        let work = ducktape_host::runtime::follow(node.clone());
        self.out = Some(spawn_on_runtime(cx, work, move |this, followed, cx| {
            this.out = None;
            this.asked.insert(node.clone());
            match followed {
                Some(followed) => {
                    this.running.insert(node, followed);
                }
                None => {
                    this.resting.insert(node);
                    this.rest_then_ask(cx);
                }
            }
            cx.notify();
            this.sync(cx);
        }));
    }

    /// The resting nodes asked again after [`REST`].
    fn rest_then_ask(&mut self, cx: &mut Context<Self>) {
        if self.rest.is_some() {
            return;
        }
        self.rest = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REST).await;
            let _ = this.update(cx, |this, cx| {
                this.rest = None;
                this.resting.clear();
                this.sync(cx);
            });
        }));
    }

    /// Locked, every set runs on with no key and no account, as the
    /// network in hand's notifiers do, and one followed meanwhile starts
    /// so; unlocked, with them again. The host keeps the lock beside its
    /// sets, so a follow out as it moves starts as it is.
    fn lock(&mut self, cx: &mut Context<Self>) {
        let locked = self.account.read(cx).get().locked;
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        ducktape_host::runtime::lock_followed(locked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::entities::{AccountState, SessionState};
    use ducktape_host::runtime::follow_for_test;
    use gpui_kit::{AppContext as _, TestAppContext};

    /// Nodes that refuse at once: a follow of either is out, then rests.
    const THERE: &str = "http://127.0.0.1:1";
    const HERE: &str = "http://127.0.0.1:2";

    fn listed(url: &str) -> RecentEndpoint {
        RecentEndpoint {
            url: url.into(),
            network: "testkit".into(),
            ..Default::default()
        }
    }

    /// A follower over a session and an account of the test's own, on
    /// what `state` lists.
    fn follower(
        state: SessionState,
        cx: &mut TestAppContext,
    ) -> (Entity<Follower>, Entity<Session>, Entity<Account>) {
        // a follow asks its node on the runtime's thread
        cx.executor().allow_parking();
        let session = cx.update(ducktape_host::session::tests::session);
        let (_, account, _) = session.read_with(cx, |session, _| session.parts());
        session.update(cx, |session, cx| session.seed(state, cx));
        let follower = cx.new(|cx| Follower::new(&session, &account, cx));
        (follower, session, account)
    }

    /// Every listed node but the one in hand, most recent first, each
    /// once; and a set whose chain is taken up in hand, through any of its
    /// nodes, stops (a second node of that chain the host refuses,
    /// `follow_refuses_the_chain_in_hand_and_a_second_set`).
    #[gpui_kit::test]
    fn wanted_skips_the_chain_in_hand_and_dedupes(cx: &mut TestAppContext) {
        let recent = [listed("a"), listed(HERE), listed("b"), listed("a")];
        assert_eq!(wanted(&recent, HERE), ["a", "b"]);
        assert_eq!(wanted(&recent, ""), ["a", HERE, "b"]);

        let chain = "follow-skips#1";
        let state = SessionState {
            connected: true,
            connected_rpc: HERE.into(),
            chain: "follow-elsewhere#1".into(),
            recent_endpoints: vec![listed(HERE), listed(THERE)],
            ..SessionState::default()
        };
        let (follower, session, _) = follower(state.clone(), cx);
        let followed = follow_for_test(THERE, chain, "", None).expect("a set of its own");
        follower.update(cx, |follower, _| {
            follower.running.insert(THERE.into(), followed)
        });
        session.update(cx, |session, cx| session.seed(state.clone(), cx));
        assert_eq!(
            follower.read_with(cx, |follower, _| follower.running.len()),
            1
        );
        let taken_up = SessionState {
            chain: chain.into(),
            ..state
        };
        session.update(cx, |session, cx| session.seed(taken_up, cx));
        assert!(
            follower.read_with(cx, |follower, _| follower.running.is_empty()),
            "a set of the chain in hand runs on"
        );
    }

    /// While a connect is out, no follow starts (a switch's node may be the
    /// one asked); once it lands, the next listed node is asked.
    #[gpui_kit::test]
    fn nothing_starts_while_connecting(cx: &mut TestAppContext) {
        let state = SessionState {
            connecting: true,
            recent_endpoints: vec![listed(THERE)],
            ..SessionState::default()
        };
        let (follower, session, _) = follower(state.clone(), cx);
        cx.run_until_parked();
        let out = |cx: &mut TestAppContext| {
            follower.read_with(cx, |follower, _| {
                (follower.out.is_some(), follower.asked.len())
            })
        };
        assert_eq!(out(cx), (false, 0), "a follow started while connecting");
        let landed = SessionState {
            connecting: false,
            ..state
        };
        session.update(cx, |session, cx| session.seed(landed, cx));
        assert_eq!(out(cx), (true, 0), "no follow started once it landed");
    }

    /// Lock hands every set no key and no account, as the network in
    /// hand's notifiers, a set followed while it is locked from its start;
    /// Unlock hands them back.
    #[gpui_kit::test]
    fn lock_drops_the_account(cx: &mut TestAppContext) {
        let state = SessionState {
            recent_endpoints: vec![listed(THERE)],
            ..SessionState::default()
        };
        let (follower, _, account) = follower(state, cx);
        let followed =
            follow_for_test(THERE, "follow-lock#1", "ab", Some(7)).expect("a set of its own");
        follower.update(cx, |follower, _| {
            follower.running.insert(THERE.into(), followed)
        });
        let handed = |cx: &mut TestAppContext| {
            follower.read_with(cx, |follower, _| follower.running[THERE].handed())
        };
        assert_eq!(handed(cx), ("ab".into(), Some(7)));
        let locked = |locked| AccountState {
            locked,
            ..AccountState::default()
        };
        account.update(cx, |account, cx| account.seed(locked(true), None, None, cx));
        assert_eq!(
            handed(cx),
            (String::new(), None),
            "locked, a set kept the account"
        );
        let later =
            follow_for_test(HERE, "follow-lock#2", "cd", Some(8)).expect("a set of its own");
        let started = later.handed();
        account.update(cx, |account, cx| {
            account.seed(locked(false), None, None, cx)
        });
        assert_eq!(
            started,
            (String::new(), None),
            "followed while locked, it had the key"
        );
        assert_eq!(handed(cx), ("ab".into(), Some(7)));
        assert_eq!(later.handed(), ("cd".into(), Some(8)));
    }
}
