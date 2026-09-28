//! Fixtures the ui tests share: states built the way the app reaches them,
//! and the node's answers.

use super::{AppMessage as Message, Ducktape, Stage};
use crate::backend;

/// A fresh boot moved straight to the key step. Not connected: for a
/// node reached, `on_testkit()`.
pub(super) fn signing_in() -> Ducktape {
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Unlock(Default::default());
    state
}

pub(super) fn resolved(state: &mut Ducktape, account: Option<(u64, String)>) {
    let key = state.signer_key.clone();
    let node = state.connected_rpc.clone();
    let _ = state.update(Message::AccountResolved { node, key, account });
}

pub(super) fn on_testkit() -> Ducktape {
    let mut state = signing_in();
    state.stage = Stage::Desk;
    state.connected = true;
    state.connected_rpc = "http://a".into();
    state.network = "testkit".into();
    state.keyring = "testkit".into();
    state.height = 7;
    state.status = "Connected · block 7".into();
    state.signer_key = "ab".into();
    state.account = Some(Some((7, "Grace Hopper".into())));
    state.active = Some("chat");
    state.badges.insert("chat", 3);
    state
}

pub(super) fn status(height: u64) -> backend::NodeStatus {
    backend::NodeStatus {
        network: "testkit".into(),
        time: 0,
        block_time_ms: 0,
        epoch_length: 0,
        height,
        tip: [0; 32],
        root: abi::Root([0; 32]),
        epoch: 0,
        identity: Vec::new(),
        contract: backend::noded::NODE_CONTRACT,
        genesis: [0; 32],
    }
}
