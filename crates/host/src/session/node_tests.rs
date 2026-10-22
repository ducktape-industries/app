//! `Session` polls the node it is on through a status source of its own,
//! on the test clock, and moves only what moved: the poll's misses and
//! answers onto `Chain`; a connect that fails; a switch; leaving.
//!
//! A node taken up for real (`connect_answered`) binds a keyring and
//! notes the node on this device and hands the runtime its roster: the
//! machine's, not a unit test's. The sessions here are seeded on a node
//! already reached (`Session::seed_connected`); the take-up itself is
//! `Account::take_up`'s row in account_tests.rs and the kit's.
use super::tests::{Heads, heads, notifies, session, source, status};
use super::{AccountStep, Screen, Session, SessionEvent, SessionState};
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

fn on_node() -> SessionState {
    SessionState {
        connected: true,
        connected_rpc: ORIGIN.into(),
        endpoint: ORIGIN.into(),
        network: "session-test".into(),
        chain: "session-test".into(),
        ..SessionState::booted()
    }
}

fn following(
    answer: impl Fn(usize) -> Result<NodeStatus, noded_client::Error> + 'static,
    cx: &mut TestAppContext,
) -> (Entity<Session>, Rc<Cell<usize>>, Heads) {
    let session = cx.update(session);
    let (source, asked) = source(answer);
    let (fed, heads) = heads();
    session.update(cx, |session, cx| {
        session.heads_through(fed);
        session.seed_connected(on_node(), source, cx);
    });
    cx.run_until_parked();
    (session, asked, heads)
}

fn height(session: &Entity<Session>, cx: &mut TestAppContext) -> i64 {
    chain(session, cx).read_with(cx, |chain, _| chain.height)
}

#[gpui_kit::test]
fn a_node_not_reached_is_said_and_a_new_try_clears_it(cx: &mut TestAppContext) {
    let session = cx.update(session);
    let (source, asked) = source(|_| {
        Err(noded_client::Error::Unreachable(
            "error sending request for url".into(),
        ))
    });
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
    let (source, _) = source(|_| {
        Err(noded_client::Error::Unreachable(
            "error sending request for url".into(),
        ))
    });
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
fn an_unchanged_head_notifies_nothing(cx: &mut TestAppContext) {
    let (session, asked, heads) = following(|_| Ok(testnet(7)), cx);
    heads.send(7);
    cx.run_until_parked();
    let chain = chain(&session, cx);
    let (sessions, _a) = notifies(&session, cx);
    let (chains, _b) = notifies(&chain, cx);
    heads.send(7);
    heads.send(7);
    cx.run_until_parked();
    assert_eq!(sessions.get(), 0, "a still chain moved the session");
    assert_eq!(chains.get(), 0, "a still chain moved the chain");
    assert_eq!(
        asked.get(),
        0,
        "a head asked a status with the node menu shut"
    );
}

#[gpui_kit::test]
fn a_moved_height_notifies_the_chain_and_not_the_session(cx: &mut TestAppContext) {
    let (session, _, heads) = following(|_| Ok(testnet(7)), cx);
    heads.send(7);
    cx.run_until_parked();
    let chain = chain(&session, cx);
    assert_eq!(chain.read_with(cx, |chain, _| chain.height), 7);
    chain.update(cx, |chain, _| chain.block_seen = 0);
    let (sessions, _a) = notifies(&session, cx);
    let (chains, _b) = notifies(&chain, cx);
    let before = crate::runtime::notify::wall();
    heads.send(8);
    cx.run_until_parked();
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
    let (session, _, heads) = following(|_| Ok(testnet(7)), cx);
    heads.send(7);
    cx.run_until_parked();
    let chain = chain(&session, cx);
    chain.update(cx, |chain, _| {
        chain.heard = 0;
        chain.block_seen = 0;
    });
    let (chains, _b) = notifies(&chain, cx);
    let before = crate::runtime::notify::wall();
    heads.send(7);
    cx.run_until_parked();
    let (heard, block_seen) = chain.read_with(cx, |chain, _| (chain.heard, chain.block_seen));
    assert!(
        heard >= before,
        "heard is the wall clock: {heard} < {before}"
    );
    assert_eq!(block_seen, 0, "the same height moved no block");
    assert_eq!(
        chains.get(),
        0,
        "a head that only moved heard notified the chain"
    );
}

#[gpui_kit::test]
fn a_lost_node_reads_answering_at_its_next_head_which_asks_its_status(cx: &mut TestAppContext) {
    let (session, asked, heads) = following(|_| Ok(testnet(7)), cx);
    heads.send(7);
    cx.run_until_parked();
    heads.lose();
    cx.run_until_parked();
    let lost = state(&session, cx);
    assert!(lost.reconnecting, "a lost socket left the node answering");
    assert!(lost.connected, "a lost node is still the node in hand");
    assert_eq!(asked.get(), 0, "the loss asked a status");
    heads.send(8);
    cx.run_until_parked();
    assert!(
        !state(&session, cx).reconnecting,
        "a head did not read the node back"
    );
    assert_eq!(asked.get(), 1, "the head after a loss asked no status");
    assert_eq!(heads.opens(), 1, "a loss its source heals opened it again");
}

#[gpui_kit::test]
fn the_open_node_menu_asks_the_status_on_each_head(cx: &mut TestAppContext) {
    let (session, asked, heads) = following(|n| Ok(testnet(7 + n as u64)), cx);
    heads.send(7);
    cx.run_until_parked();
    assert_eq!(asked.get(), 0, "a head asked a status with the menu shut");
    session.update(cx, |session, cx| session.node_watched(true, cx));
    cx.run_until_parked();
    assert_eq!(asked.get(), 1, "opening the menu asked no status");
    heads.send(8);
    cx.run_until_parked();
    assert_eq!(asked.get(), 2, "a head under the open menu asked no status");
    let shown =
        chain(&session, cx).read_with(cx, |chain, _| chain.node.as_ref().map(|node| node.height));
    assert_eq!(shown, Some(8));
    session.update(cx, |session, cx| session.node_watched(false, cx));
    heads.send(9);
    cx.run_until_parked();
    assert_eq!(asked.get(), 2, "a head asked a status once the menu shut");
}

#[gpui_kit::test]
fn a_switch_that_does_not_land_keeps_the_network_in_hand(cx: &mut TestAppContext) {
    let (session, _, heads) = following(|_| Ok(testnet(7)), cx);
    let (heard, _heard) = events(&session, cx);
    let (other, _) = source(|_| {
        Err(noded_client::Error::Unreachable(
            "error sending request".into(),
        ))
    });
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
    heads.send(9);
    cx.run_until_parked();
    assert_eq!(height(&session, cx), 9, "the heads stopped");
    session.update(cx, |session, cx| session.switch(ORIGIN.into(), cx));
    assert!(!state(&session, cx).connecting);
}

#[gpui_kit::test]
fn disconnecting_leaves_everything_of_the_network(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    let (session, _, heads) = following(|_| Ok(testnet(7)), cx);
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
    heads.send(9);
    cx.run_until_parked();
    assert_ne!(
        height(&session, cx),
        9,
        "the heads were followed after leaving"
    );
}

#[gpui_kit::test]
fn session_methods_record_under_reducer_connect(cx: &mut TestAppContext) {
    let _on = crate::perf::on_for_test();
    let (session, _, _heads) = following(|_| Ok(testnet(7)), cx);
    session.update(cx, |session, cx| session.poll_now(cx));
    cx.run_until_parked();
    let snapshot = crate::perf::snapshot(false);
    assert!(
        snapshot["shell"]["reducer.connect"].is_object(),
        "no reducer.connect timer: {}",
        snapshot["shell"]
    );
}

#[gpui_kit::test]
fn the_heads_stop_in_the_background_and_the_status_is_asked_in_front(cx: &mut TestAppContext) {
    use gpui_kit::AppLifecyclePhase::{Background, Foreground};
    let (session, asked, heads) = following(|n| Ok(testnet(n as u64 + 1)), cx);
    session.update(cx, |session, cx| session.lifecycle(Background, cx));
    heads.send(5);
    cx.run_until_parked();
    assert_eq!(
        height(&session, cx),
        -1,
        "a head was heard in the background"
    );
    assert_eq!(asked.get(), 0, "the node was asked in the background");
    session.update(cx, |session, cx| session.lifecycle(Foreground, cx));
    cx.run_until_parked();
    assert_eq!(asked.get(), 1, "coming to the front did not ask at once");
    assert_eq!(heads.opens(), 2, "the heads socket did not open again");
    heads.send(9);
    cx.run_until_parked();
    assert_eq!(height(&session, cx), 9);
}

fn saved(url: &str, signed_in: bool, genesis: [u8; 32]) {
    crate::backend::note_endpoint(crate::backend::RecentEndpoint {
        url: url.into(),
        network: "testkit".into(),
        genesis: Some(genesis),
        signed_in,
        ..Default::default()
    });
}

fn on_desk(screen: Screen) -> bool {
    matches!(screen, Screen::Desk | Screen::Layout)
}

#[gpui_kit::test]
fn a_saved_network_lands_on_the_desk_before_its_node_answers(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    cx.executor().allow_parking();
    const SAVED: &str = "http://127.0.0.1:11";
    saved(SAVED, true, [0; 32]);
    let session = cx.update(session);
    let (source, _) = source(|n| Ok(status(7 + n as u64)));
    let (fed, heads) = heads();
    session.update(cx, |session, cx| {
        session.heads_through(fed);
        session.connect_through(SAVED.into(), source, cx)
    });
    let presumed = state(&session, cx);
    assert!(presumed.connected && presumed.connecting, "{presumed:?}");
    assert_eq!(
        presumed.chain,
        crate::runtime::chain_named("testkit", &[0; 32])
    );
    let shown = screen(&session, cx);
    assert!(
        on_desk(shown),
        "a saved network waited for its node: {shown:?}"
    );
    cx.run_until_parked();
    let confirmed = state(&session, cx);
    assert!(confirmed.connected && !confirmed.connecting && !confirmed.reconnecting);
    assert_eq!(
        chain(&session, cx).read_with(cx, |chain, _| chain.height),
        7
    );
    assert!(on_desk(screen(&session, cx)));
    heads.send(9);
    cx.run_until_parked();
    assert_eq!(
        height(&session, cx),
        9,
        "the heads were not followed on the confirming answer"
    );
}

#[gpui_kit::test]
fn a_saved_network_whose_node_does_not_answer_stays_on_the_desk(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    cx.executor().allow_parking();
    const SAVED: &str = "http://127.0.0.1:12";
    saved(SAVED, true, [0; 32]);
    let session = cx.update(session);
    let (source, asked) = source(|n| match n {
        0 => Err(noded_client::Error::Unreachable(
            "error sending request for url".into(),
        )),
        _ => Ok(status(7)),
    });
    let (fed, heads) = heads();
    session.update(cx, |session, cx| {
        session.heads_through(fed);
        session.connect_through(SAVED.into(), source, cx)
    });
    cx.run_until_parked();
    let lost = state(&session, cx);
    assert!(lost.connected && lost.reconnecting, "{lost:?}");
    assert!(
        lost.error.is_empty(),
        "a saved network's miss was said as a failure"
    );
    assert!(on_desk(screen(&session, cx)));
    heads.send(7);
    cx.run_until_parked();
    assert!(
        !state(&session, cx).reconnecting,
        "a head did not read the node answering"
    );
    assert_eq!(asked.get(), 2, "the head asked no status to confirm");
    assert_eq!(
        chain(&session, cx).read_with(cx, |chain, _| chain.height),
        7
    );
}

fn one_contract_behind() -> Result<NodeStatus, noded_client::Error> {
    let behind = NodeStatus {
        contract: noded_client::wire::NODE_CONTRACT - 1,
        ..status(7)
    };
    abi::decode(&abi::encode(&behind)).map_err(noded_client::Error::Decode)
}

#[gpui_kit::test]
fn a_saved_node_on_another_contract_is_left_saying_so(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    cx.executor().allow_parking();
    const SAVED: &str = "http://127.0.0.1:16";
    saved(SAVED, true, [0; 32]);
    let session = cx.update(session);
    let (source, asked) = source(|_| one_contract_behind());
    session.update(cx, |session, cx| {
        session.connect_through(SAVED.into(), source, cx)
    });
    cx.run_until_parked();
    let left = state(&session, cx);
    assert!(!left.connected && !left.reconnecting, "{left:?}");
    assert!(
        left.error
            .starts_with("testkit (127.0.0.1:16): this node speaks contract"),
        "{}",
        left.error
    );
    assert_eq!(screen(&session, cx), Screen::Connect);
    cx.run_until_parked();
    assert_eq!(asked.get(), 1, "a node on another contract was asked on");
}

#[gpui_kit::test]
fn a_node_that_moves_to_another_contract_is_left_saying_so(cx: &mut TestAppContext) {
    let (session, asked, heads) = following(
        |n| match n {
            0 => Ok(testnet(7)),
            _ => one_contract_behind(),
        },
        cx,
    );
    session.update(cx, |session, cx| session.node_watched(true, cx));
    cx.run_until_parked();
    heads.send(8);
    cx.run_until_parked();
    let left = state(&session, cx);
    assert!(!left.connected && !left.reconnecting, "{left:?}");
    assert!(
        left.error.contains("this node speaks contract"),
        "{}",
        left.error
    );
    heads.send(9);
    cx.run_until_parked();
    assert_eq!(asked.get(), 2, "a node on another contract was asked on");
}

#[gpui_kit::test]
fn a_saved_node_that_answers_another_chain_is_taken_up_afresh(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    cx.executor().allow_parking();
    const SAVED: &str = "http://127.0.0.1:13";
    saved(SAVED, true, [1; 32]);
    let session = cx.update(session);
    let (source, _) = source(|_| Ok(status(7)));
    session.update(cx, |session, cx| {
        session.connect_through(SAVED.into(), source, cx)
    });
    assert_eq!(
        state(&session, cx).chain,
        crate::runtime::chain_named("testkit", &[1; 32])
    );
    cx.run_until_parked();
    let taken = state(&session, cx);
    assert!(taken.connected && !taken.connecting);
    assert_eq!(
        taken.chain,
        crate::runtime::chain_named("testkit", &[0; 32])
    );
    assert!(
        !on_desk(screen(&session, cx)),
        "the other chain kept the presumed desk"
    );
}

#[gpui_kit::test]
fn a_saved_network_never_signed_in_waits_on_the_key_step(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    cx.executor().allow_parking();
    const SAVED: &str = "http://127.0.0.1:14";
    saved(SAVED, false, [0; 32]);
    let session = cx.update(session);
    let (source, _) = source(|_| Ok(status(7)));
    session.update(cx, |session, cx| {
        session.connect_through(SAVED.into(), source, cx)
    });
    let shown = screen(&session, cx);
    assert_eq!(shown, Screen::Unlock { awaiting: false });
    assert!(
        state(&session, cx).connected,
        "its reads did not start at launch"
    );
}

#[gpui_kit::test]
fn a_presumed_desk_whose_key_holds_no_account_goes_to_the_account_step(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    cx.executor().allow_parking();
    const SAVED: &str = "http://127.0.0.1:15";
    saved(SAVED, true, [0; 32]);
    let session = cx.update(session);
    let (source, _) = source(|_| Ok(status(7)));
    session.update(cx, |session, cx| {
        session.connect_through(SAVED.into(), source, cx)
    });
    cx.run_until_parked();
    let (_, account, _) = session.read_with(cx, |session, _| session.parts());
    let key = account.read_with(cx, |account, _| account.get().signer_key.clone());
    account.update(cx, |account, cx| account.resolved(SAVED, &key, None, cx));
    assert_eq!(
        screen(&session, cx),
        Screen::Account {
            step: AccountStep::Name
        }
    );
}

thread_local! {
    static WATCHED: RefCell<Vec<(String, tokio::sync::mpsc::Sender<()>)>> =
        const { RefCell::new(Vec::new()) };
}

fn watched(_: &crate::backend::RpcClient, program: &str) -> crate::runtime::Writes {
    let (tell, writes) = crate::runtime::Writes::idle();
    WATCHED.with(|watched| watched.borrow_mut().push((program.to_owned(), tell)));
    writes
}

fn watching() -> Vec<(String, bool)> {
    WATCHED.with(|watched| {
        watched
            .borrow()
            .iter()
            .map(|(program, tell)| (program.clone(), !tell.is_closed()))
            .collect()
    })
}

#[gpui_kit::test]
fn a_moved_identity_role_moves_the_watch_the_session_follows(cx: &mut TestAppContext) {
    WATCHED.with(|watched| watched.borrow_mut().clear());
    let binding = |identity: &str, height: u64| NodeStatus {
        roles: abi::Roles::from_iter([
            (abi::role::PROGRAMS, "programs"),
            (abi::role::IDENTITY, identity),
        ]),
        ..testnet(height)
    };
    let session = cx.update(session);
    let (source, _) = source(move |n| match n {
        0 => Ok(binding("identity", 7)),
        _ => Ok(binding("owner/identity", 7 + n as u64)),
    });
    session.update(cx, |session, cx| {
        session.watching_with(crate::backend::rpc_client(ORIGIN), watched);
        session.seed_connected(on_node(), source, cx);
        session.node_watched(true, cx);
        session.node_watched(false, cx);
    });
    cx.run_until_parked();
    let own = |program: &str, open: bool| (program.to_owned(), open);
    assert_eq!(watching(), [own("programs", true), own("identity", true)]);
    written("programs");
    cx.run_until_parked();
    assert_eq!(
        watching(),
        [
            own("programs", false),
            own("identity", false),
            own("programs", true),
            own("owner/identity", true),
        ],
        "the watch did not follow identity's move"
    );
    written("programs");
    cx.run_until_parked();
    assert_eq!(watching().len(), 4, "an unmoved binding re-armed");
}

fn written(program: &str) {
    WATCHED.with(|watched| {
        let watched = watched.borrow();
        let open = watched
            .iter()
            .rev()
            .find(|(name, tell)| name == program && !tell.is_closed());
        if let Some((_, tell)) = open {
            let _ = tell.try_send(());
        }
    });
}

#[gpui_kit::test]
fn a_heads_socket_that_ends_reads_the_node_lost_until_retry_opens_it_again(
    cx: &mut TestAppContext,
) {
    let (session, asked, heads) = following(|_| Ok(testnet(8)), cx);
    heads.send(7);
    cx.run_until_parked();
    assert_eq!(height(&session, cx), 7, "a head moved no height");
    assert!(!state(&session, cx).reconnecting);
    heads.cut();
    cx.run_until_parked();
    assert!(
        state(&session, cx).reconnecting,
        "a socket that ended left the node answering"
    );
    assert_eq!(asked.get(), 0, "the loss asked a status");
    session.update(cx, |session, cx| session.poll_now(cx));
    cx.run_until_parked();
    assert_eq!(asked.get(), 1, "Retry asked no status");
    assert_eq!(
        heads.opens(),
        2,
        "Retry did not open the heads socket again"
    );
    heads.send(9);
    cx.run_until_parked();
    let back = state(&session, cx);
    assert!(back.connected && !back.reconnecting, "{back:?}");
    assert_eq!(height(&session, cx), 9);
}

#[gpui_kit::test]
fn a_status_that_fails_while_heads_arrive_leaves_the_node_answering(cx: &mut TestAppContext) {
    let (session, _, heads) =
        following(|_| Err(noded_client::Error::Unreachable("slow".into())), cx);
    for at in [7, 8] {
        heads.send(at);
        cx.executor().advance_clock(crate::runtime::STATUS_EVERY);
        cx.run_until_parked();
    }
    assert!(
        !state(&session, cx).reconnecting,
        "a status read judged the node lost while its heads came"
    );
    assert_eq!(height(&session, cx), 8);
}
