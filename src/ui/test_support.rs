//! The node's answers, for the tests that feed the session one.

use crate::backend;

pub(crate) fn status(height: u64) -> backend::NodeStatus {
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
