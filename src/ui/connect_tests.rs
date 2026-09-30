//! Reaching a node, its polls, a switch, and leaving (connect.rs).

use super::test_support::{on_testkit, signing_in, status};
use super::{AppMessage as Message, Ducktape, Stage};
use crate::backend;

#[test]
fn two_missed_polls_read_reconnecting_and_one_answer_recovers() {
    let (mut state, _) = Ducktape::boot();
    state.connected = true;
    state.status = "Connected · block 7".into();
    let _ = state.update(Message::StatusMissed);
    assert!(!state.reconnecting(), "one miss is a hiccup");
    assert_eq!(state.status, "Connected · block 7");
    let _ = state.update(Message::StatusMissed);
    assert!(state.reconnecting());
    assert_eq!(state.status, "Reconnecting…");
    assert!(state.connected, "the poll keeps running");
    // same height: no block moved, so nothing else is asked for
    state.height = 9;
    let _ = state.update(Message::StatusPushed(status(9)));
    assert!(!state.reconnecting());
    assert_eq!(state.status, "Connected · block 9");
}

#[test]
fn a_new_try_clears_the_last_connect_failure() {
    let mut state = signing_in();
    state.error = "Can't reach http://a.".into();
    let _ = state.update(Message::EndpointTyped("not a url at all".into()));
    assert!(state.error.is_empty());
    state.error = "Can't reach http://a.".into();
    let _ = state.update(Message::ConnectSubmit);
    assert!(state.error.is_empty(), "the stale failure hid the refusal");
    assert_eq!(state.endpoint_error, backend::ENDPOINT_REFUSAL);
}

#[test]
fn switching_node_leaves_no_key_seated() {
    let mut state = signing_in();
    state.signer_key = "ab".into();
    state.stage = Stage::Account(super::Account {
        name: "ada".into(),
        link_code: "ABCD-EFGH".into(),
        ..Default::default()
    });
    state.sign_in.unlock_error = "wrong".into();
    let _ = state.update(Message::Disconnect);
    assert!(state.signer_key.is_empty());
    assert_eq!(state.stage.step(), "Connect");
    assert!(state.sign_in.unlock_error.is_empty());
}

#[test]
fn a_switch_keeps_the_network_in_hand_until_the_other_answers() {
    let mut state = on_testkit();
    let _ = state.update(Message::SwitchNetwork("http://b".into()));
    assert!(state.connecting);
    assert_eq!(state.status, "Reaching http://b…");
    assert_eq!(state.signer_key, "ab", "still signed in while reaching");
    // A's poll lands mid-switch: the status keeps saying where it is going
    let _ = state.update(Message::StatusPushed(status(8)));
    assert_eq!(state.status, "Reaching http://b…");
    let _ = state.update(Message::ConnectFailed {
        generation: state.connect_generation,
        error: "error sending request for url (http://b/v1/status)".into(),
    });
    assert!(state.connected && !state.connecting);
    assert_eq!(state.status, "Connected · block 8");
    assert_eq!(state.endpoint, "http://a");
    assert!(state.error.is_empty() && state.toast.contains("http://b"));
    assert_eq!(state.account, Some(Some((7, "Grace Hopper".into()))));
}

#[test]
fn taking_up_another_chain_resets_the_last_ones_state() {
    let mut state = on_testkit();
    let _ = state.take_up(backend::Keyring {
        dir: "testkit".into(),
        other_chain: false,
    });
    assert_eq!(
        state.signer_key, "ab",
        "a second node of one network keeps the key"
    );
    assert_eq!(state.active, Some("chat"));
    let _ = state.take_up(backend::Keyring {
        dir: "testkit+200".into(),
        other_chain: true,
    });
    assert!(
        state.signer_key.is_empty(),
        "the seat is locked on a switch"
    );
    assert_eq!(state.account, None, "the account is asked again");
    assert_eq!(state.active, None);
    assert!(state.badges.is_empty());
    assert!(state.other_chain);
    assert_eq!(state.keyring, "testkit+200");
}

#[test]
fn an_account_answer_from_before_a_switch_is_dropped() {
    let mut state = on_testkit();
    state.account = None;
    let _ = state.update(Message::AccountResolved {
        node: "http://old".into(),
        key: "ab".into(),
        account: Some((1, "Stale".into())),
    });
    assert_eq!(state.account, None);
    let _ = state.update(Message::AccountResolved {
        node: "http://a".into(),
        key: "ab".into(),
        account: None,
    });
    assert_eq!(state.account, Some(None));
}
