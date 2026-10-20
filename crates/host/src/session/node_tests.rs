//! `Session` polls the node it is on through a status source of its own,
//! on the test clock, and moves only what moved: the poll's misses and
//! answers onto `Chain`; a connect that fails; a switch; leaving.
//!
//! A node taken up for real (`connect_answered`) binds a keyring and
//! notes the node on this device and hands the runtime its roster: the
//! machine's, not a unit test's. The sessions here are seeded on a node
//! already reached (`Session::seed_connected`); the take-up itself is
//! `Account::take_up`'s row in account_tests.rs and the kit's.
use super::tests::{notifies, session, source, status};
use super::{STATUS_EVERY, Screen, Session, SessionEvent, SessionState};
use crate::backend::NodeStatus;
use gpui_kit::{Entity, Subscription, TestAppContext};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const ORIGIN: &str = "http://127.0.0.1:1";

const OTHER: &str = "http://127.0.0.1:2";

fn testnet(height: u64) -> NodeStatus {
    NodeStatus {
        network: "session-test".into(),
        ..status(height)
    }
}

fn events(
    session: &Entity<Session>,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<SessionEvent>>>, Subscription) {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let keep = heard.clone();
    let observing = cx.update(|cx| {
        cx.subscribe(session, move |_, event: &SessionEvent, _| {
            keep.borrow_mut().push(event.clone())
        })
    });
    (heard, observing)
}

fn chain(session: &Entity<Session>, cx: &mut TestAppContext) -> Entity<super::Chain> {
    session.read_with(cx, |session, _| session.parts().0)
}

fn screen(session: &Entity<Session>, cx: &mut TestAppContext) -> Screen {
    let (_, _, screen) = session.read_with(cx, |session, _| session.parts());
    screen.read_with(cx, |screen, _| *screen.get())
}

fn state(session: &Entity<Session>, cx: &mut TestAppContext) -> SessionState {
    session.read_with(cx, |session, _| session.get().clone())
}

fn poll(cx: &mut TestAppContext) {
    cx.executor().advance_clock(STATUS_EVERY);
    cx.run_until_parked();
}

fn connected(
    answer: impl Fn(usize) -> Result<NodeStatus, String> + 'static,
    cx: &mut TestAppContext,
) -> (Entity<Session>, Rc<Cell<usize>>) {
    let session = cx.update(session);
    let (source, asked) = source(answer);
    session.update(cx, |session, cx| {
        let on_node = SessionState {
            connected: true,
            connected_rpc: ORIGIN.into(),
            endpoint: ORIGIN.into(),
            network: "session-test".into(),
            chain: "session-test".into(),
            ..SessionState::booted()
        };
        session.seed_connected(on_node, source, cx);
    });
    poll(cx);
    assert_eq!(asked.get(), 1, "the first poll did not land");
    (session, asked)
}

#[gpui_kit::test]
fn a_node_not_reached_is_said_and_a_new_try_clears_it(cx: &mut TestAppContext) {
    let session = cx.update(session);
    let (source, asked) = source(|_| Err("error sending request for url".into()));
    session.update(cx, |session, cx| {
        session.connect_through(ORIGIN.into(), source, cx)
    });
    assert!(state(&session, cx).connecting);
    cx.run_until_parked();
    let failed = state(&session, cx);
    assert!(!failed.connected && !failed.connecting);
    assert_eq!(asked.get(), 1);
    assert_eq!(
        failed.error,
        "Nothing answered at 127.0.0.1:1. Check the address, and that the node is up and \
         reachable from this computer's network."
    );
    session.update(cx, |session, cx| {
        session.set_endpoint("not a url at all".into(), cx)
    });
    assert!(state(&session, cx).error.is_empty());
    session.update(cx, |session, cx| session.submit(cx));
    let refused = state(&session, cx);
    assert_eq!(
        refused.endpoint_error,
        noded_client::endpoint::ENDPOINT_REFUSAL
    );
    assert!(!refused.connecting, "a refused address was tried");
}

#[gpui_kit::test]
fn a_connect_out_reaches_for_the_address_it_was_started_with(cx: &mut TestAppContext) {
    let session = cx.update(session);
    let (source, _) = source(|_| Err("error sending request for url".into()));
    session.update(cx, |session, cx| {
        session.connect_through(ORIGIN.into(), source, cx)
    });
    assert_eq!(state(&session, cx).reaching, ORIGIN);
    let typed = format!("{ORIGIN}9");
    session.update(cx, |session, cx| session.set_endpoint(typed.clone(), cx));
    let out = state(&session, cx);
    assert!(out.connecting);
    assert_eq!(out.endpoint, typed);
    assert_eq!(out.reaching, ORIGIN, "the attempt out followed the typing");
    cx.run_until_parked();
    let failed = state(&session, cx);
    assert!(failed.reaching.is_empty(), "no attempt is out");
    assert!(
        failed.error.contains("127.0.0.1:1.") && !failed.error.contains("127.0.0.1:19"),
        "the failure named an address that was not tried: {}",
        failed.error
    );
}

#[gpui_kit::test]
fn an_unchanged_status_poll_notifies_nothing(cx: &mut TestAppContext) {
    let (session, asked) = connected(|_| Ok(testnet(7)), cx);
    let chain = chain(&session, cx);
    let (sessions, _a) = notifies(&session, cx);
    let (chains, _b) = notifies(&chain, cx);
    poll(cx);
    assert_eq!(asked.get(), 2, "two seconds on the clock: one poll");
    poll(cx);
    assert_eq!(asked.get(), 3);
    assert_eq!(sessions.get(), 0, "a still chain moved the session");
    assert_eq!(chains.get(), 0, "a still chain moved the chain");
}

#[gpui_kit::test]
fn a_moved_height_notifies_the_chain_and_not_the_session(cx: &mut TestAppContext) {
    let (session, _) = connected(|n| Ok(testnet(7 + n as u64)), cx);
    let chain = chain(&session, cx);
    assert_eq!(chain.read_with(cx, |chain, _| chain.height), 7);
    chain.update(cx, |chain, _| chain.block_seen = 0);
    let (sessions, _a) = notifies(&session, cx);
    let (chains, _b) = notifies(&chain, cx);
    let before = crate::runtime::notify::wall();
    poll(cx);
    let (height, block_seen) = chain.read_with(cx, |chain, _| (chain.height, chain.block_seen));
    assert_eq!(height, 8);
    assert!(
        block_seen >= before,
        "a new height is a block seen now: {block_seen} < {before}"
    );
    assert_eq!(chains.get(), 1, "a block did not move the chain once");
    assert_eq!(sessions.get(), 0, "a block moved the session");
}

#[gpui_kit::test]
fn heard_moves_without_a_notify(cx: &mut TestAppContext) {
    let (session, _) = connected(|_| Ok(testnet(7)), cx);
    let chain = chain(&session, cx);
    chain.update(cx, |chain, _| {
        chain.heard = 0;
        chain.block_seen = 0;
    });
    let (chains, _b) = notifies(&chain, cx);
    let before = crate::runtime::notify::wall();
    poll(cx);
    let (heard, block_seen) = chain.read_with(cx, |chain, _| (chain.heard, chain.block_seen));
    assert!(
        heard >= before,
        "heard is the wall clock: {heard} < {before}"
    );
    assert_eq!(block_seen, 0, "the same height moved no block");
    assert_eq!(
        chains.get(),
        0,
        "a poll that only moved heard notified the chain"
    );
}

#[gpui_kit::test]
fn a_missed_poll_reads_reconnecting_and_one_answer_recovers(cx: &mut TestAppContext) {
    let (session, _) = connected(
        |n| match n {
            1 | 2 => Err("no".into()),
            _ => Ok(testnet(7)),
        },
        cx,
    );
    let (sessions, _a) = notifies(&session, cx);
    poll(cx);
    assert!(
        state(&session, cx).reconnecting,
        "one miss is the node lost"
    );
    poll(cx);
    let lost = state(&session, cx);
    assert!(lost.reconnecting);
    assert!(lost.connected, "a lost node is still the node in hand");
    poll(cx);
    assert!(!state(&session, cx).reconnecting);
    assert_eq!(
        sessions.get(),
        2,
        "reconnecting moved other than there and back"
    );
}

#[gpui_kit::test]
fn a_switch_that_does_not_land_keeps_the_network_in_hand(cx: &mut TestAppContext) {
    let (session, asked) = connected(|_| Ok(testnet(7)), cx);
    let (heard, _heard) = events(&session, cx);
    let (other, _) = source(|_| Err("error sending request".into()));
    session.update(cx, |session, cx| {
        session.connect_through(OTHER.into(), other, cx)
    });
    let reaching = state(&session, cx);
    assert!(reaching.connected && reaching.connecting);
    assert_eq!(reaching.endpoint, OTHER);
    cx.run_until_parked();
    let kept = state(&session, cx);
    assert!(kept.connected && !kept.connecting);
    assert_eq!(kept.connected_rpc, ORIGIN);
    assert_eq!(
        kept.endpoint, ORIGIN,
        "the address went back to the node in hand"
    );
    assert!(
        kept.error.is_empty(),
        "a failed switch was said under the field"
    );
    assert!(
        matches!(&heard.borrow()[..], [SessionEvent::Toast(said)] if said.contains("127.0.0.1:2.")),
        "{:?}",
        heard.borrow()
    );
    let polls = asked.get();
    poll(cx);
    assert_eq!(asked.get(), polls + 1, "the poll stopped");
    session.update(cx, |session, cx| session.switch(ORIGIN.into(), cx));
    assert!(!state(&session, cx).connecting);
}

#[gpui_kit::test]
fn disconnecting_leaves_everything_of_the_network(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    let (session, asked) = connected(|_| Ok(testnet(7)), cx);
    let (chain, account, _) = session.read_with(cx, |session, _| session.parts());
    account.update(cx, |account, cx| {
        let mut signed_in = account.get().clone();
        signed_in.signer_key = "ab".into();
        signed_in.error = "wrong".into();
        account.seed(signed_in, None, None, cx);
    });
    let (heard, _heard) = events(&session, cx);
    session.update(cx, |session, cx| session.disconnect(cx));
    cx.run_until_parked();
    let left = state(&session, cx);
    assert!(!left.connected && left.network.is_empty() && left.chain.is_empty());
    assert_eq!(*heard.borrow(), [SessionEvent::LeftNetwork]);
    assert_eq!(chain.read_with(cx, |chain, _| chain.node.clone()), None);
    assert_eq!(screen(&session, cx), Screen::Connect);
    let account = account.read_with(cx, |account, _| account.get().clone());
    assert!(account.signer_key.is_empty() && account.error.is_empty());
    let polls = asked.get();
    poll(cx);
    assert_eq!(asked.get(), polls, "the poll went on after leaving");
}

#[gpui_kit::test]
fn session_methods_record_under_reducer_connect(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (_session, _) = connected(|_| Ok(testnet(7)), cx);
    poll(cx);
    let snapshot = crate::perf::snapshot(false);
    assert!(
        snapshot["shell"]["reducer.connect"].is_object(),
        "no reducer.connect timer: {}",
        snapshot["shell"]
    );
}

#[gpui_kit::test]
fn the_poll_stops_in_the_background_and_asks_at_once_in_front(cx: &mut TestAppContext) {
    use gpui_kit::AppLifecyclePhase::{Background, Foreground};
    let (session, asked) = connected(|n| Ok(testnet(n as u64 + 1)), cx);
    session.update(cx, |session, cx| session.lifecycle(Background, cx));
    poll(cx);
    poll(cx);
    assert_eq!(asked.get(), 1, "the node was asked in the background");
    session.update(cx, |session, cx| session.lifecycle(Foreground, cx));
    cx.run_until_parked();
    assert_eq!(asked.get(), 2, "coming to the front did not ask at once");
    poll(cx);
    assert_eq!(asked.get(), 3, "the poll did not go on");
}
