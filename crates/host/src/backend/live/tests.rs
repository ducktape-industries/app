use super::*;

#[test]
fn a_build_without_the_live_client_refuses_every_request_live_unavailable() {
    let rpc = crate::backend::rpc_client("http://127.0.0.1:9");
    let ask = Ask::Watch {
        namespace: "room".into(),
    };
    let Err(refused) = futures::executor::block_on(session(&rpc).open(ask)) else {
        panic!("a request opened without a live client");
    };
    assert_eq!(refused.code, refusal::LIVE_UNAVAILABLE);
}
