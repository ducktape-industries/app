//! The node methods against a fake node: what each hands the node and what
//! it makes of the answer.
use super::*;

fn call(target: &str, body: &[u8]) -> Vec<u8> {
    methods::encode(&methods::Call {
        target: target.into(),
        body: body.to_vec(),
    })
}

/// A node on a local socket that answers `responses` in order, one
/// connection each, checking every request line against `expected_path`;
/// joined, it hands back the request bodies it read.
fn node_server(
    expected_path: &'static str,
    responses: Vec<(&str, Vec<u8>)>,
) -> (node::Node, std::thread::JoinHandle<Vec<Vec<u8>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let responses: Vec<(String, Vec<u8>)> = responses
        .into_iter()
        .map(|(status, body)| (status.to_owned(), body))
        .collect();
    let server = std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let (line, received) = read_request(&mut stream);
            assert_eq!(line.trim(), expected_path);
            bodies.push(received);
            respond(&mut stream, &status, &body);
        }
        bodies
    });
    (node, server)
}

#[tokio::test]
async fn binary_queries_preserve_signed_payloads_raw_replies_and_node_refusals() {
    let payload = vec![0, 255, 128, 3];
    let (node, server) = node_server(
        "POST /v1/query HTTP/1.1",
        vec![
            ("200 OK", abi::encode(&vec![255u8, 0, 129])),
            (
                "400 Bad Request",
                abi::encode(&abi::Refusal::new("query_denied", "no read")),
            ),
        ],
    );
    let ask = call("registry", &payload);
    assert_eq!(
        query(node.clone(), ask.clone()).await.unwrap(),
        [255, 0, 129]
    );
    let refused = query(node.clone(), ask).await.unwrap_err();
    assert_eq!(refused.code, "query_denied");
    assert_eq!(refused.message, "no read");
    let bodies = server.join().unwrap();
    assert_eq!(bodies.len(), 2);
    for body in bodies {
        let query: backend::noded::Query = abi::decode(&body).unwrap();
        assert_eq!(query.layer, backend::Layer::Preconfirmed);
        let frame: backend::noded::Frame = abi::decode(&query.frame).unwrap();
        assert_eq!(frame.body.target, "registry");
        assert_eq!(frame.body.network, b"test-network");
        assert_eq!(frame.body.payload, payload);
        assert_eq!(frame.proof.len(), 64);
    }

    for ask in [
        b"registry".to_vec(),
        Vec::new(),
        call("", b""),
        call("../registry", b""),
    ] {
        assert_eq!(
            query(node.clone(), ask).await.unwrap_err().code,
            "malformed_request"
        );
    }
}

/// A node from before `/v1/network` answers a bare 404: `unknown_request`
/// at once, which the retry loop takes as the node's word, not a transport
/// failure to ask again.
#[tokio::test]
async fn a_node_without_the_network_route_is_refused_at_once() {
    let (node, server) = node_server(
        "GET /v1/network HTTP/1.1",
        vec![("404 Not Found", Vec::new())],
    );
    let refused = network(node, Vec::new()).await.unwrap_err();
    server.join().unwrap();
    assert_eq!(refused.code, "unknown_request");
    assert_eq!(
        refused.message,
        "This node doesn't report its validators' votes. Update the node."
    );
    assert!(!node::transport_failed(&refused), "not retried");
}

#[tokio::test]
async fn system_status_preserves_borsh_and_refusals() {
    let expected = backend::noded::Status {
        network: "test-network".into(),
        time: 12,
        block_time_ms: 500,
        epoch_length: 100,
        height: 201,
        tip: [2; 32],
        root: abi::Root([3; 32]),
        epoch: 2,
        identity: vec![4; 32],
        contract: 1,
        genesis: [5; 32],
    };
    let (node, server) = node_server(
        "GET /v1/status HTTP/1.1",
        vec![("200 OK", abi::encode(&expected))],
    );
    let answer = status(node.clone(), Vec::new()).await.unwrap();
    let decoded: methods::NodeStatus = methods::decode(&answer).unwrap();
    assert_eq!(
        (
            decoded.chain_id.as_str(),
            decoded.height,
            decoded.root,
            decoded.contract
        ),
        ("test-network", 201, [3; 32], 1)
    );
    server.join().unwrap();
    assert_eq!(
        status(node, b"{}".to_vec()).await.unwrap_err().code,
        "malformed_request"
    );
    let (node, server) = node_server(
        "GET /v1/status HTTP/1.1",
        vec![(
            "400 Bad Request",
            abi::encode(&abi::Refusal::new("status_denied", "private node")),
        )],
    );
    assert_eq!(
        status(node, Vec::new()).await.unwrap_err().code,
        "status_denied"
    );
    server.join().unwrap();
}

#[tokio::test]
async fn a_block_s_receipts_carry_refusals_as_errors() {
    let tx = |seq, receipt| backend::noded::Tx {
        hash: [seq as u8; 32],
        signer: vec![5; 32],
        seq,
        target: "chat".into(),
        payload: vec![7],
        receipt: Some(receipt),
    };
    let rejected = backend::noded::Receipt {
        program: "chat".into(),
        outcome: abi::Outcome::Rejected(abi::Refusal::new("not_member", "not in this room")),
        events: Vec::new(),
        nested: vec![backend::noded::Receipt {
            program: "identity".into(),
            outcome: abi::Outcome::Applied { output: vec![1] },
            events: vec![vec![2]],
            nested: Vec::new(),
        }],
    };
    let applied = backend::noded::Receipt {
        program: "chat".into(),
        outcome: abi::Outcome::Applied { output: vec![3] },
        events: vec![vec![4]],
        nested: Vec::new(),
    };
    let finalized = backend::noded::Finalized {
        height: 9,
        id: [1; 32],
        parent: [2; 32],
        time: 1_000,
        epoch: 0,
        proposer: None,
        txs: vec![tx(1, rejected), tx(2, applied)],
    };
    let (node, server) = node_server(
        "POST /v1/block HTTP/1.1",
        vec![("200 OK", abi::encode(&Some(finalized)))],
    );
    let answer = block(node, methods::encode(&methods::BlockRef::Height(9)))
        .await
        .unwrap();
    server.join().unwrap();
    let block = methods::decode::<Option<methods::Block>>(&answer)
        .unwrap()
        .unwrap();
    let receipts: Vec<_> = block.txs.into_iter().map(|tx| tx.receipt).collect();
    assert_eq!(
        receipts,
        vec![
            Some(methods::Receipt {
                program: "chat".into(),
                outcome: methods::Outcome::Rejected(wire::Error::new(
                    "not_member",
                    "not in this room"
                )),
                events: Vec::new(),
                nested: vec![methods::Receipt {
                    program: "identity".into(),
                    outcome: methods::Outcome::Applied { output: vec![1] },
                    events: vec![vec![2]],
                    nested: Vec::new(),
                }],
            }),
            Some(methods::Receipt {
                program: "chat".into(),
                outcome: methods::Outcome::Applied { output: vec![3] },
                events: vec![vec![4]],
                nested: Vec::new(),
            }),
        ]
    );
}

#[tokio::test]
async fn block_methods_carry_the_archive_s_blocks_as_method_types() {
    let finalized = backend::noded::Finalized {
        height: 9,
        id: [1; 32],
        parent: [2; 32],
        time: 1_000,
        epoch: 0,
        proposer: Some(vec![3; 32]),
        txs: vec![backend::noded::Tx {
            hash: [4; 32],
            signer: vec![5; 32],
            seq: 6,
            target: "chat".into(),
            payload: vec![7],
            receipt: None,
        }],
    };
    let (node, server) = node_server(
        "POST /v1/blocks HTTP/1.1",
        vec![("200 OK", abi::encode(&vec![finalized.clone()]))],
    );
    let page = methods::encode(&methods::BlockPage {
        before: Some(10),
        limit: 1,
    });
    let answer = blocks(node, page).await.unwrap();
    let decoded: Vec<methods::Block> = methods::decode(&answer).unwrap();
    assert_eq!(
        decoded,
        vec![methods::Block {
            height: 9,
            id: [1; 32],
            parent: [2; 32],
            time: 1_000,
            epoch: 0,
            proposer: Some(vec![3; 32]),
            txs: vec![methods::Tx {
                hash: [4; 32],
                signer: vec![5; 32],
                seq: 6,
                target: "chat".into(),
                payload: vec![7],
                receipt: None,
            }],
        }]
    );
    server.join().unwrap();

    let (node, server) = node_server(
        "POST /v1/block HTTP/1.1",
        vec![("200 OK", abi::encode(&None::<backend::noded::Finalized>))],
    );
    let answer = block(
        node.clone(),
        methods::encode(&methods::BlockRef::Id([8; 32])),
    )
    .await
    .unwrap();
    assert_eq!(
        methods::decode::<Option<methods::Block>>(&answer).unwrap(),
        None
    );
    server.join().unwrap();
    assert_eq!(
        block(node, b"x".to_vec()).await.unwrap_err().code,
        "malformed_request"
    );
}

/// `blob.get` replies `Option<Vec<u8>>`, borsh both ends like every method:
/// the blob when the node holds it, `None` when it does not — absent is not
/// a refusal.
#[tokio::test]
async fn blob_get_answers_the_blob_or_none() {
    let mut framed = b"sha256\0".to_vec();
    framed.extend_from_slice(b"blob body bytes");
    let ask = methods::encode(&format!("sha256:{}", "00".repeat(32)));
    let (node, server) = node_server(
        "POST /v1/blob/get HTTP/1.1",
        vec![("200 OK", abi::encode(&Some(framed)))],
    );
    let answer = blob_get(node, ask.clone()).await.unwrap();
    let decoded: Option<Vec<u8>> = methods::decode(&answer).unwrap();
    assert_eq!(decoded.as_deref(), Some(&b"blob body bytes"[..]));
    server.join().unwrap();
    let (node, server) = node_server(
        "POST /v1/blob/get HTTP/1.1",
        vec![("200 OK", abi::encode(&None::<Vec<u8>>))],
    );
    let answer = blob_get(node, ask).await.unwrap();
    assert_eq!(methods::decode::<Option<Vec<u8>>>(&answer).unwrap(), None);
    server.join().unwrap();
}

#[tokio::test]
async fn invite_preserves_blob_notes_ttl_and_typed_refusals() {
    // the node was asked for `ttl` days
    let asked_ttl = |bodies: Vec<Vec<u8>>, ttl: u64| {
        let [body] = bodies.as_slice() else {
            panic!("one request");
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(body).unwrap(),
            serde_json::json!({"ttl_days": ttl})
        );
    };
    let (node, server) = node_server(
        "POST /v1/invite HTTP/1.1",
        vec![(
            "200 OK",
            br#"{"invite":"paste-me","notes":[{"reason":"local_only","sentence":"Use on this box"}]}"#
                .to_vec(),
        )],
    );
    let mint = |ttl_days| methods::encode(&methods::CreateInvite { ttl_days });
    let answer = invite(node.clone(), mint(7)).await.unwrap();
    let decoded: methods::Invite = methods::decode(&answer).unwrap();
    assert_eq!(
        decoded,
        methods::Invite {
            invite: "paste-me".into(),
            notes: vec![wire::Error {
                code: "local_only".into(),
                message: "Use on this box".into()
            }]
        }
    );
    asked_ttl(server.join().unwrap(), 7);
    for ask in [Vec::new(), mint(0), b"7".to_vec()] {
        assert_eq!(
            invite(node.clone(), ask).await.unwrap_err().code,
            "malformed_request"
        );
    }
    for (http, body, reason) in [
        (
            "403 Forbidden",
            br#"{"reason":"invite_denied","error":"operator only"}"#.to_vec(),
            "invite_denied",
        ),
        // a core built after it dropped /v1/invite answers a bare 404, not
        // the node's own refusal envelope — that must read as "this node
        // doesn't do invites", not the raw transport error.
        ("404 Not Found", Vec::new(), "invite_unsupported"),
    ] {
        let (node, server) = node_server("POST /v1/invite HTTP/1.1", vec![(http, body)]);
        let refusal = invite(node, mint(1)).await.unwrap_err();
        assert_eq!(refusal.code, reason);
        asked_ttl(server.join().unwrap(), 1);
    }
    let (node, server) = node_server(
        "POST /v1/invite HTTP/1.1",
        vec![("404 Not Found", Vec::new())],
    );
    assert_eq!(
        invite(node, mint(1)).await.unwrap_err().message,
        "This node doesn't mint invites."
    );
    asked_ttl(server.join().unwrap(), 1);
    // an unrelated non-JSON non-2xx (a proxy's 502, say) must not be folded
    // into the same message: only http_error + 404 gets the friendlier text.
    let (node, server) = node_server(
        "POST /v1/invite HTTP/1.1",
        vec![("502 Bad Gateway", Vec::new())],
    );
    let refusal = invite(node, mint(1)).await.unwrap_err();
    assert_eq!(refusal.code, "http_error");
    assert!(refusal.message.starts_with("502"));
    asked_ttl(server.join().unwrap(), 1);
}
