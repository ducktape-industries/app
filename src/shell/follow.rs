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

const UNFOLLOWED_NODE_RETRY_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) struct Follower {
    session: Entity<Session>,
    account: Entity<Account>,
    running: BTreeMap<String, Followed>,
    asked: BTreeSet<String>,
    resting: BTreeSet<String>,
    chain: String,
    out: Option<Task<()>>,
    rest: Option<Task<()>>,
    locked: bool,
    _observing: [Subscription; 2],
}

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

    pub(crate) fn follow_console(
        &mut self,
        session: &Entity<Session>,
        account: &Entity<Account>,
        cx: &mut Context<Self>,
    ) {
        self.session = session.clone();
        self.account = account.clone();
        self._observing = [
            cx.observe(session, |this, _, cx| this.sync(cx)),
            cx.observe(account, |this, _, cx| this.lock(cx)),
        ];
        self.lock(cx);
        self.sync(cx);
    }

    pub(crate) fn asking(&self, cx: &App) -> bool {
        let state = self.session.read(cx).get();
        wanted(&state.recent_endpoints, &state.connected_rpc)
            .iter()
            .any(|node| !self.asked.contains(node))
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let state = self.session.read(cx).get().clone();
        if state.chain != self.chain {
            self.chain = state.chain.clone();
            self.resting.clear();
        }
        let wanted = wanted(&state.recent_endpoints, &state.connected_rpc);
        let before = self.running.len();
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

    fn rest_then_ask(&mut self, cx: &mut Context<Self>) {
        if self.rest.is_some() {
            return;
        }
        self.rest = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(UNFOLLOWED_NODE_RETRY_INTERVAL)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.rest = None;
                this.resting.clear();
                this.sync(cx);
            });
        }));
    }

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

    const THERE: &str = "http://127.0.0.1:1";
    const HERE: &str = "http://127.0.0.1:2";

    fn listed(url: &str) -> RecentEndpoint {
        RecentEndpoint {
            url: url.into(),
            network: "testkit".into(),
            ..Default::default()
        }
    }

    fn follower(
        state: SessionState,
        cx: &mut TestAppContext,
    ) -> (Entity<Follower>, Entity<Session>, Entity<Account>) {
        cx.executor().allow_parking();
        let session = cx.update(ducktape_host::session::tests::session);
        let (_, account, _) = session.read_with(cx, |session, _| session.parts());
        session.update(cx, |session, cx| session.seed(state, cx));
        let follower = cx.new(|cx| Follower::new(&session, &account, cx));
        (follower, session, account)
    }

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
