//! A `Shared` made on a network of its own: its session connects that
//! network and lets it go, and its seats seat there, never in hand.
use super::Shared;
use super::tests::{source, status};
use crate::backend::NodeStatus;
use crate::runtime::{Network, Placed, in_hand};
use gpui_kit::TestAppContext;
use std::collections::BTreeMap;

const ORIGIN: &str = "http://127.0.0.1:1";
const NETWORK: &str = "shared-wiring";
const PROGRAM: &str = "shared-wiring-view";
const PANE: u64 = 4501;

#[gpui_kit::test]
fn a_shared_on_its_own_network_connects_seats_and_leaves_there(cx: &mut TestAppContext) {
    let _seat = crate::backend::seat_serial();
    let _chain = crate::runtime::notifier::tests::serial();
    cx.executor().allow_parking();
    let network = Network::apart();
    let shared = cx.update(|cx| {
        gpui_kit::init(cx);
        let changes = futures::channel::mpsc::unbounded().1;
        Shared::new(network.clone(), Default::default(), changes, cx)
    });
    shared.session.update(cx, |session, _| {
        session.heads_through(super::tests::quiet())
    });
    let (source, asked) = source(|_| {
        Ok(NodeStatus {
            network: NETWORK.into(),
            ..status(7)
        })
    });
    shared.session.update(cx, |session, cx| {
        session.connect_through(ORIGIN.into(), source, cx)
    });
    cx.run_until_parked();
    assert_eq!(asked.get(), 1, "the connect asked no status");
    let chain = shared
        .session
        .read_with(cx, |session, _| session.get().chain.clone());
    assert_eq!(
        network.reached(),
        Some((NETWORK.to_owned(), chain)),
        "the session's own network was not connected"
    );
    assert!(
        in_hand().reached().is_none_or(|(name, _)| name != NETWORK),
        "the network in hand was connected"
    );

    let place = (PROGRAM, None, Placed::default());
    shared.seats.update(cx, |seats, cx| {
        seats.place(BTreeMap::from([(PANE, place)]), cx)
    });
    assert!(
        network.seated(PROGRAM, PANE),
        "the pane was not seated on its network"
    );
    assert!(
        !in_hand().seated(PROGRAM, PANE),
        "the pane was seated in hand"
    );

    shared
        .session
        .update(cx, |session, cx| session.disconnect(cx));
    cx.run_until_parked();
    assert_eq!(network.reached(), None, "leaving kept its network's node");
}
