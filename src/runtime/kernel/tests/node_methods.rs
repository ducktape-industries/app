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
    let ask = call("programs", &payload);
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
        let query: noded_client::wire::Query = abi::decode(&body).unwrap();
        assert_eq!(query.layer, backend::Layer::Preconfirmed);
        let frame: noded_client::wire::Frame = abi::decode(&query.frame).unwrap();
        assert_eq!(frame.body.target, "programs");
        assert_eq!(frame.body.network, b"test-network");
        assert_eq!(frame.body.payload, payload);
        assert_eq!(frame.proof.len(), 64);
    }

    for ask in [
        b"programs".to_vec(),
        Vec::new(),
        call("", b""),
        call("../programs", b""),
    ] {
        assert_eq!(
            query(node.clone(), ask).await.unwrap_err().code,
            refusal::MALFORMED_REQUEST
        );
    }
}

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
    assert!(!node::transport_failed(&refused), "the node's word");
}

#[tokio::test]
async fn system_status_preserves_borsh_and_refusals() {
    let expected = noded_client::wire::Status {
        network: "test-network".into(),
        time: 12,
        block_time_ms: 500,
        epoch_length: 100,
        height: 201,
        tip: [2; 32],
        root: abi::Root([3; 32]),
        epoch: 2,
        identity: vec![4; 32],
        contract: noded_client::wire::NODE_CONTRACT,
        genesis: [5; 32],
        roles: crate::backend::test_roles(),
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
        (
            "test-network",
            201,
            [3; 32],
            noded_client::wire::NODE_CONTRACT
        )
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
    let tx = |hash, receipt| noded_client::wire::Tx {
        hash: [hash; 32],
        signer: vec![5; 32],
        anchor: noded_client::wire::Tip {
            height: 8,
            id: [hash; 32],
        },
        target: "chat".into(),
        size: 1,
        payload: Some(vec![7]),
        receipt: Some(receipt),
    };
    let rejected = noded_client::wire::Receipt {
        program: "chat".into(),
        outcome: abi::Outcome::Rejected(abi::Refusal::new("not_member", "not in this room")),
        events: Vec::new(),
        nested: vec![noded_client::wire::Receipt {
            program: "identity".into(),
            outcome: abi::Outcome::Applied { output: vec![1] },
            events: vec![vec![2]],
            nested: Vec::new(),
        }],
    };
    let applied = noded_client::wire::Receipt {
        program: "chat".into(),
        outcome: abi::Outcome::Applied { output: vec![3] },
        events: vec![vec![4]],
        nested: Vec::new(),
    };
    let finalized = noded_client::wire::Finalized {
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
    let ask = methods::encode(&methods::BlockAsk {
        by: methods::BlockRef::Height(9),
        most: u32::MAX,
    });
    let answer = block(node, ask).await.unwrap();
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

fn listed_tx(payload: Option<Vec<u8>>) -> noded_client::wire::Tx {
    noded_client::wire::Tx {
        hash: [4; 32],
        signer: vec![5; 32],
        anchor: noded_client::wire::Tip {
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
    let finalized = noded_client::wire::Finalized {
        height: 9,
        id: [1; 32],
        parent: [2; 32],
        time: 1_000,
        epoch: 0,
        proposer: Some(vec![3; 32]),
        txs: vec![listed_tx(None)],
    };
    let listed = noded_client::wire::Page {
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
        most: 16,
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
    let asked = server.join().unwrap();
    let summaries = noded_client::wire::Blocks {
        before: Some(10),
        limit: 1,
        most: 16,
    };
    assert_eq!(asked, [abi::encode(&summaries)]);

    let located = noded_client::wire::Located {
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
        vec![(
            "200 OK",
            abi::encode(&None::<noded_client::wire::Finalized>),
        )],
    );
    let answer = block(
        node.clone(),
        methods::encode(&methods::BlockAsk {
            by: methods::BlockRef::Id([8; 32]),
            most: 32,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        methods::decode::<Option<methods::Block>>(&answer).unwrap(),
        None
    );
    let asked = server.join().unwrap();
    let summary = noded_client::wire::BlockAsk {
        by: noded_client::wire::BlockRef::Id([8; 32]),
        most: 32,
    };
    assert_eq!(asked, [abi::encode(&summary)]);
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
        noded_client::wire::route::BLOB_GET,
        Mode::Answer(abi::encode(&Some(framed))),
    )]);
    let answer = blob_get(held.node.clone(), ask.clone()).await.unwrap();
    let decoded: Option<Vec<u8>> = methods::decode(&answer).unwrap();
    assert_eq!(decoded.as_deref(), Some(&b"blob body bytes"[..]));
    let absent = fake_node(vec![(
        noded_client::wire::route::BLOB_GET,
        Mode::Answer(abi::encode(&None::<Vec<u8>>)),
    )]);
    let answer = blob_get(absent.node.clone(), ask).await.unwrap();
    assert_eq!(methods::decode::<Option<Vec<u8>>>(&answer).unwrap(), None);
}

/// A core module of custom sections, as a program's code is laid out.
fn module(sections: &[(&str, &[u8])]) -> Vec<u8> {
    fn leb(mut n: usize, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }
    let mut bytes = b"\0asm\x01\0\0\0".to_vec();
    for (name, body) in sections {
        let mut section = Vec::new();
        leb(name.len(), &mut section);
        section.extend_from_slice(name.as_bytes());
        section.extend_from_slice(body);
        bytes.push(0);
        leb(section.len(), &mut bytes);
        bytes.extend_from_slice(&section);
    }
    bytes
}

/// `blob.manifest` reads the blob as a seat would: a program's view out of
/// its `ducktape.view` section with the manifest that view carries, a
/// view-only blob as the view itself, a program with no section as none,
/// a view this host cannot read (a program's or a view-only one) as a view
/// with no manifest, and a blob the node does not hold as `None`. The
/// blob's bytes never reach the view.
#[tokio::test]
async fn blob_manifest_answers_what_the_view_in_a_blob_declares() {
    use sha2::Digest as _;
    use view_wire::manifest::Manifest;
    let text = "ducktape.view.manifest\nChat\nRooms.\nchain,clipboard,\n480\n0123abcd\n\n";
    let view = module(&[("ducktape.view.manifest", text.as_bytes())]);
    let program = module(&[("ducktape.view", &view)]);
    let manifest = Manifest {
        wire_id: "0123abcd".into(),
        name: "Chat".into(),
        description: "Rooms.".into(),
        capabilities: vec![methods::Capability::Chain, methods::Capability::Clipboard],
        min_width: 480,
        targets: Vec::new(),
        icon: String::new(),
    };
    let ask = |body: &[u8]| {
        let mut framed = format!("program {}\0", body.len()).into_bytes();
        framed.extend_from_slice(body);
        let id = sha2::Sha256::digest(&framed);
        let held = fake_node(vec![(
            noded_client::wire::route::BLOB_GET,
            Mode::Answer(abi::encode(&Some(framed))),
        )]);
        listed(abi::BlobId::Sha256(id.into()));
        let ask = methods::encode(&format!("sha256:{}", noded_client::hex::hex_encode(&id)));
        async move {
            let answer = blob_manifest(held.node.clone(), ask).await.unwrap();
            methods::decode::<Option<methods::Manifested>>(&answer).unwrap()
        }
    };
    assert_eq!(
        ask(&program).await,
        Some(methods::Manifested {
            size: program.len() as u64,
            view: Some(methods::CarriedView {
                size: view.len() as u64,
                manifest: Some(manifest.clone()),
            }),
        })
    );
    assert_eq!(
        ask(&view).await,
        Some(methods::Manifested {
            size: view.len() as u64,
            view: Some(methods::CarriedView {
                size: view.len() as u64,
                manifest: Some(manifest),
            }),
        }),
        "a view-only blob is the view"
    );
    let bare = module(&[("ducktape.describe", b"not a view")]);
    assert_eq!(
        ask(&bare).await,
        Some(methods::Manifested {
            size: bare.len() as u64,
            view: None,
        }),
        "a program that draws nothing"
    );
    let unreadable = module(&[("ducktape.view.manifest", b"?")]);
    let unread = Some(methods::CarriedView {
        size: unreadable.len() as u64,
        manifest: None,
    });
    assert_eq!(
        ask(&module(&[("ducktape.view", &unreadable)]))
            .await
            .and_then(|blob| blob.view),
        unread,
        "a program's view whose manifest this host cannot read"
    );
    assert_eq!(
        ask(&unreadable).await.and_then(|blob| blob.view),
        unread,
        "a view-only blob this host cannot read is a view, not a program that draws nothing"
    );
    let absent = fake_node(vec![(
        noded_client::wire::route::BLOB_GET,
        Mode::Answer(abi::encode(&None::<Vec<u8>>)),
    )]);
    listed(abi::BlobId::Sha256([0; 32]));
    let ask = methods::encode(&format!("sha256:{}", "00".repeat(32)));
    let answer = blob_manifest(absent.node.clone(), ask).await.unwrap();
    assert_eq!(
        methods::decode::<Option<methods::Manifested>>(&answer).unwrap(),
        None
    );
}

/// `code` listed in the app's roster as a running program's, under a name
/// of its own.
fn listed(code: abi::BlobId) {
    crate::runtime::roster::roster()
        .lock()
        .push(crate::backend::views::Program {
            name: format!("manifest-test-{}", &abi::hex(code.digest())[..8]),
            code,
            bare: false,
        });
}

/// `blob.manifest` reads only code the programs role names: a listed
/// program's, a view-only entry's, or what a scheduled change brings, the
/// scheduled list read for the ask. An id none of them name is refused,
/// the node asked for the list and nothing else; a scheduled code is read
/// as a seat reads it, kept under its id, so the next ask reads the disk,
/// not the node.
#[tokio::test]
async fn blob_manifest_reads_only_code_the_roster_names() {
    use sha2::Digest as _;
    let program = module(&[("ducktape.describe", b"scheduled")]);
    let mut framed = format!("program {}\0", program.len()).into_bytes();
    framed.extend_from_slice(&program);
    let id = abi::BlobId::Sha256(sha2::Sha256::digest(&framed).into());
    let kept = crate::backend::views::cache_path(&id).unwrap();
    let _ = std::fs::remove_file(&kept);
    let scheduled = |items: Vec<::programs::Pending>| {
        let reply = ::programs::Reply::Scheduled(::programs::PageResponse {
            height: 12,
            items,
            next: None,
        });
        (
            noded_client::wire::route::QUERY,
            Mode::Answer(abi::encode(&abi::encode(&reply))),
        )
    };
    let brings = ::programs::Pending {
        height: 20,
        change: ::programs::Change::Set(::programs::Entry {
            program: "later".into(),
            code: id,
            params: Vec::new(),
        }),
        by: None,
    };
    let ask = methods::encode(&format!(
        "sha256:{}",
        noded_client::hex::hex_encode(id.digest())
    ));
    // nothing scheduled: refused, the node asked for the list alone
    let nothing = fake_node(vec![scheduled(Vec::new())]);
    let refusal = blob_manifest(nothing.node.clone(), ask.clone())
        .await
        .unwrap_err();
    assert_eq!(refusal.code, refusal::UNDECLARED_TARGET);
    assert_eq!(
        nothing.accepted.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an unnamed code reached the node"
    );
    // a scheduled change brings it: read, kept
    let held = fake_node(vec![
        scheduled(vec![brings.clone()]),
        (
            noded_client::wire::route::BLOB_GET,
            Mode::Answer(abi::encode(&Some(framed.clone()))),
        ),
    ]);
    let size = |answer: Vec<u8>| {
        methods::decode::<Option<methods::Manifested>>(&answer)
            .unwrap()
            .map(|blob| blob.size)
    };
    let answer = blob_manifest(held.node.clone(), ask.clone()).await.unwrap();
    assert_eq!(size(answer), Some(program.len() as u64));
    assert!(kept.exists(), "a named code's read was not kept");
    // the next ask reads the disk: a node with no blob route answers it
    let listed_only = fake_node(vec![scheduled(vec![brings])]);
    let answer = blob_manifest(listed_only.node.clone(), ask).await;
    let _ = std::fs::remove_file(&kept);
    assert_eq!(
        answer.map(size),
        Ok(Some(program.len() as u64)),
        "answered off the disk"
    );
}

/// A blob id is hex digits only: `+f` (which `from_str_radix` reads as 15)
/// is a malformed id, refused before the node is asked, so each digest has
/// one spelling.
#[tokio::test]
async fn blob_get_refuses_a_signed_hex_spelling() {
    let held = fake_node(vec![(
        noded_client::wire::route::BLOB_GET,
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
    let status = noded_client::wire::Status {
        network: "test-network".into(),
        time: 12,
        block_time_ms: 1000,
        epoch_length: 64,
        height: 9,
        tip: [2; 32],
        root: abi::Root([3; 32]),
        epoch: 0,
        identity: identity.to_vec(),
        contract: noded_client::wire::NODE_CONTRACT,
        genesis: [5; 32],
        roles: crate::backend::test_roles(),
    };
    fake_node(vec![
        (
            noded_client::wire::route::STATUS,
            Mode::Answer(abi::encode(&status)),
        ),
        (
            noded_client::wire::route::QUERY,
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
    let invite: Invite = abi::decode(&noded_client::hex::hex_decode(&hex).unwrap()).unwrap();
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
    let again: Invite = abi::decode(&noded_client::hex::hex_decode(&again).unwrap()).unwrap();
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
