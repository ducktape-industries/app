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
pub(super) fn node_server(
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
            refusal::MALFORMED_REQUEST
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
    assert_eq!(refused.code, refusal::UNKNOWN_REQUEST);
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
        roles: backend::noded::test_roles(),
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
        refusal::MALFORMED_REQUEST
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
    let tx = |hash, receipt| backend::noded::Tx {
        hash: [hash; 32],
        signer: vec![5; 32],
        anchor: backend::noded::Tip {
            height: 8,
            id: [hash; 32],
        },
        target: "chat".into(),
        size: 1,
        payload: Some(vec![7]),
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

fn listed_tx(payload: Option<Vec<u8>>) -> backend::noded::Tx {
    backend::noded::Tx {
        hash: [4; 32],
        signer: vec![5; 32],
        anchor: backend::noded::Tip {
            height: 6,
            id: [6; 32],
        },
        target: "chat".into(),
        size: 5_000,
        payload,
        receipt: None,
    }
}

fn method_tx(payload: Option<Vec<u8>>) -> methods::Tx {
    methods::Tx {
        hash: [4; 32],
        signer: vec![5; 32],
        anchor: methods::Tip {
            height: 6,
            id: [6; 32],
        },
        target: "chat".into(),
        size: 5_000,
        payload,
        receipt: None,
    }
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
        txs: vec![listed_tx(None)],
    };
    let listed = backend::noded::Page {
        blocks: vec![finalized.clone()],
        listed_from: Some(4),
    };
    let (node, server) = node_server(
        "POST /v1/blocks HTTP/1.1",
        vec![("200 OK", abi::encode(&listed))],
    );
    let page = methods::encode(&methods::BlockPage {
        before: Some(10),
        limit: 1,
    });
    let answer = blocks(node, page).await.unwrap();
    let decoded: methods::BlockList = methods::decode(&answer).unwrap();
    assert_eq!(
        decoded,
        methods::BlockList {
            blocks: vec![methods::Block {
                height: 9,
                id: [1; 32],
                parent: [2; 32],
                time: 1_000,
                epoch: 0,
                proposer: Some(vec![3; 32]),
                txs: vec![method_tx(None)],
            }],
            listed_from: Some(4),
        }
    );
    server.join().unwrap();

    let located = backend::noded::Located {
        height: 9,
        position: 0,
        time: 1_000,
        tx: listed_tx(Some(vec![7; 5_000])),
    };
    let (node, server) = node_server(
        "POST /v1/tx HTTP/1.1",
        vec![("200 OK", abi::encode(&Some(located)))],
    );
    let ask = methods::encode(&methods::TxAsk {
        tx: methods::TxRef::At {
            height: 9,
            position: 0,
        },
        most: u32::MAX,
    });
    let answer = tx(node.clone(), ask).await.unwrap();
    assert_eq!(
        methods::decode::<Option<methods::Located>>(&answer).unwrap(),
        Some(methods::Located {
            height: 9,
            position: 0,
            time: 1_000,
            tx: method_tx(Some(vec![7; 5_000])),
        })
    );
    server.join().unwrap();
    assert_eq!(
        tx(node, b"x".to_vec()).await.unwrap_err().code,
        refusal::MALFORMED_REQUEST
    );

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
        refusal::MALFORMED_REQUEST
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
    let held = fake_node(vec![(
        backend::noded::route::BLOB_GET,
        Mode::Answer(abi::encode(&Some(framed))),
    )]);
    let answer = blob_get(held.node.clone(), ask.clone()).await.unwrap();
    let decoded: Option<Vec<u8>> = methods::decode(&answer).unwrap();
    assert_eq!(decoded.as_deref(), Some(&b"blob body bytes"[..]));
    let absent = fake_node(vec![(
        backend::noded::route::BLOB_GET,
        Mode::Answer(abi::encode(&None::<Vec<u8>>)),
    )]);
    let answer = blob_get(absent.node.clone(), ask).await.unwrap();
    assert_eq!(methods::decode::<Option<Vec<u8>>>(&answer).unwrap(), None);
}

/// A blob id is hex digits only: `+f` (which `from_str_radix` reads as 15)
/// is a malformed id, refused before the node is asked, so each digest has
/// one spelling.
#[tokio::test]
async fn blob_get_refuses_a_signed_hex_spelling() {
    let held = fake_node(vec![(
        backend::noded::route::BLOB_GET,
        Mode::Answer(abi::encode(&Some(b"sha256\0body".to_vec()))),
    )]);
    let ask = methods::encode(&format!("sha256:{}", "+f".repeat(32)));
    let refusal = blob_get(held.node.clone(), ask).await.unwrap_err();
    assert_eq!(refusal.code, refusal::MALFORMED_REQUEST);
    assert_eq!(held.accepted.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invite_refuses_a_ttl_it_cannot_mint_before_asking_the_node() {
    let held = fake_node(Vec::new());
    let mint = |ttl_days| methods::encode(&methods::CreateInvite { ttl_days });
    for ask in [Vec::new(), mint(0), b"7".to_vec()] {
        assert_eq!(
            invite(held.node.clone(), ask).await.unwrap_err().code,
            refusal::MALFORMED_REQUEST
        );
    }
    assert_eq!(held.accepted.load(std::sync::atomic::Ordering::SeqCst), 0);
}

fn node_key(seed: u8) -> commonware_cryptography::ed25519::PrivateKey {
    use commonware_codec::DecodeExt as _;
    commonware_cryptography::ed25519::PrivateKey::decode([seed; 32].as_slice()).unwrap()
}

fn validator_node(identity: &[u8], validators: Vec<Vec<u8>>) -> FakeNode {
    let status = backend::noded::Status {
        network: "test-network".into(),
        time: 12,
        block_time_ms: 1000,
        epoch_length: 64,
        height: 9,
        tip: [2; 32],
        root: abi::Root([3; 32]),
        epoch: 0,
        identity: identity.to_vec(),
        contract: backend::noded::NODE_CONTRACT,
        genesis: [5; 32],
        roles: backend::noded::test_roles(),
    };
    fake_node(vec![
        (
            backend::noded::route::STATUS,
            Mode::Answer(abi::encode(&status)),
        ),
        (
            backend::noded::route::QUERY,
            Mode::Answer(abi::encode(&abi::encode(&valset::Reply::Validators(
                validators,
            )))),
        ),
    ])
}

#[tokio::test]
async fn an_invite_is_the_validator_nodes_signed_grant_for_its_network() {
    use abi::role::validators::{INVITE_NAMESPACE, Invite};
    use commonware_codec::DecodeExt as _;
    use commonware_cryptography::{Signer as _, Verifier as _, ed25519};
    let key = node_key(7);
    let identity = key.public_key().as_ref().to_vec();
    let held = validator_node(&identity, vec![vec![1; 32], identity.clone()]);
    let now = 1_000_000;
    let hex = backend::invite::mint(&held.node.client, Some(key.clone()), 7, now)
        .await
        .unwrap();
    let invite: Invite = abi::decode(&backend::hex_decode(&hex).unwrap()).unwrap();
    assert_eq!(invite.issuer, identity);
    assert_eq!(invite.grant.network, b"test-network");
    assert_eq!(invite.grant.expires, now + 7 * 24 * 60 * 60 * 1000);
    let signature = ed25519::Signature::decode(invite.signature.as_slice()).unwrap();
    assert!(
        key.public_key()
            .verify(INVITE_NAMESPACE, &invite.grant.preimage(), &signature)
    );
    let again = backend::invite::mint(&held.node.client, Some(key), 7, now)
        .await
        .unwrap();
    let again: Invite = abi::decode(&backend::hex_decode(&again).unwrap()).unwrap();
    assert_ne!(again.grant.nonce, invite.grant.nonce);
}

#[tokio::test]
async fn only_the_machine_running_a_validator_node_mints_its_invites() {
    use commonware_cryptography::Signer as _;
    let key = node_key(7);
    let identity = key.public_key().as_ref().to_vec();
    let held = validator_node(&identity, vec![identity.clone()]);
    for key in [None, Some(node_key(8))] {
        let refusal = backend::invite::mint(&held.node.client, key, 7, 0)
            .await
            .unwrap_err();
        assert_eq!(refusal.code, refusal::INVITE_UNSUPPORTED);
        assert_eq!(
            refusal.message,
            "Only the machine running this node can mint its invites."
        );
    }
    let resident = validator_node(&identity, vec![vec![1; 32]]);
    let refusal = backend::invite::mint(&resident.node.client, Some(key), 7, 0)
        .await
        .unwrap_err();
    assert_eq!(refusal.code, refusal::INVITE_UNSUPPORTED);
    assert_eq!(
        refusal.message,
        "Only a validator's node can mint invites, and this node is a resident."
    );
}
