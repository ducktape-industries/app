//! The node methods against a fake node: what each hands the node and what
//! it makes of the answer.
use super::*;

fn call(target: &str, body: &[u8]) -> Vec<u8> {
    methods::encode(&methods::Call {
        target: target.into(),
        body: body.to_vec(),
    })
}

pub(super) fn node_server(
    expected_path: &'static str,
    responses: Vec<(&str, Vec<u8>)>,
) -> (node::Node, std::thread::JoinHandle<Vec<Vec<u8>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
        on: Default::default(),
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
        assert_eq!(query.layer, backend::Layer::Confirmed);
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
async fn a_view_query_reads_what_blocks_wrote_not_the_preconfirmed_overlay() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
        on: Default::default(),
    };
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let (_, received) = read_request(&mut stream);
        let asked: noded_client::wire::Query = abi::decode(&received).unwrap();
        let state: &[u8] = match asked.layer {
            backend::Layer::Confirmed => b"the frame a block wrote",
            backend::Layer::Preconfirmed => b"a frame no block wrote",
        };
        respond(&mut stream, "200 OK", &abi::encode(&state.to_vec()));
    });
    let read = query(node, call("harness", b"frames")).await.unwrap();
    server.join().unwrap();
    assert_eq!(read, b"the frame a block wrote");
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

#[tokio::test]
async fn blob_manifest_answers_what_the_view_in_a_blob_declares() {
    use sha2::Digest as _;
    use view_wire::manifest::{Manifest, Tint};
    let text =
        "ducktape.view.manifest\nChat\nRooms.\nchain,clipboard,\n480\n0123abcd\n\n\nblue\n1200";
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
        tint: Tint::Blue,
        width: Some(1200),
    };
    let ask = |body: &[u8]| {
        let mut framed = format!("program {}\0", body.len()).into_bytes();
        framed.extend_from_slice(body);
        let id = sha2::Sha256::digest(&framed);
        let held = fake_node(vec![(
            noded_client::wire::route::BLOB_GET,
            Mode::Answer(abi::encode(&Some(framed))),
        )]);
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
    let ask = methods::encode(&format!("sha256:{}", "00".repeat(32)));
    let answer = blob_manifest(absent.node.clone(), ask).await.unwrap();
    assert_eq!(
        methods::decode::<Option<methods::Manifested>>(&answer).unwrap(),
        None
    );
}

#[tokio::test]
async fn blob_manifest_keeps_only_code_the_network_runs() {
    use sha2::Digest as _;
    use std::sync::atomic::Ordering::SeqCst;
    let program = module(&[("ducktape.describe", b"named by nothing")]);
    let mut framed = format!("program {}\0", program.len()).into_bytes();
    framed.extend_from_slice(&program);
    let id = abi::BlobId::Sha256(sha2::Sha256::digest(&framed).into());
    let kept = crate::backend::views::cache_path(&id);
    let _ = std::fs::remove_file(&kept);
    let ask = methods::encode(&format!(
        "sha256:{}",
        noded_client::hex::hex_encode(id.digest())
    ));
    let size = |answer: Vec<u8>| {
        methods::decode::<Option<methods::Manifested>>(&answer)
            .unwrap()
            .map(|blob| blob.size)
    };
    let held = fake_node(vec![(
        noded_client::wire::route::BLOB_GET,
        Mode::Answer(abi::encode(&Some(framed.clone()))),
    )]);
    for _ in 0..2 {
        let answer = blob_manifest(held.node.clone(), ask.clone()).await.unwrap();
        assert_eq!(size(answer), Some(program.len() as u64));
        assert!(!kept.exists(), "a blob no program runs was kept");
    }
    assert_eq!(held.accepted.load(SeqCst), 2, "each ask read the node");

    let network = crate::runtime::roster::Network::new(true);
    network.roster.lock().push(crate::backend::views::Program {
        name: "listed".into(),
        code: id,
        bare: false,
    });
    let apart = crate::runtime::roster::On::of(&network);
    let listed = node::Node {
        on: apart.clone(),
        ..held.node.clone()
    };
    let answer = blob_manifest(listed, ask.clone()).await.unwrap();
    assert_eq!(size(answer), Some(program.len() as u64));
    assert!(kept.exists(), "code a program runs was not kept");
    let empty = fake_node(Vec::new());
    let from_disk = node::Node {
        on: apart,
        ..empty.node.clone()
    };
    let answer = blob_manifest(from_disk, ask).await;
    let _ = std::fs::remove_file(&kept);
    assert_eq!(
        answer.map(size),
        Ok(Some(program.len() as u64)),
        "answered off the disk"
    );
    assert_eq!(empty.accepted.load(SeqCst), 0);
}

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

#[test]
fn an_old_manifest_naming_targets_still_loads() {
    let text = "ducktape.view.manifest\nOld\nFrom before.\nop,program,\n480\n0123abcd\nchat,identity,\n\nteal\n";
    let view = module(&[("ducktape.view.manifest", text.as_bytes())]);
    assert_eq!(
        crate::runtime::manifest_of(&view),
        crate::runtime::Listing {
            name: "Old".into(),
            icon: String::new(),
            tint: view_wire::manifest::Tint::Teal,
            description: "From before.".into(),
            min_width: 480,
            width: None,
            capabilities: vec![methods::Capability::Op, methods::Capability::Program],
            wire: "0123abcd".into(),
        }
    );
}
