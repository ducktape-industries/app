use super::*;
use std::io::{BufRead as _, Read as _, Write as _};

fn call(target: &str, body: &[u8]) -> Vec<u8> {
    doors::encode(&doors::Call {
        target: target.into(),
        body: body.to_vec(),
    })
}

fn guest() -> Guest {
    let code = wasmtime::Module::new(
        super::super::guest::engine(),
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "init") (param i32))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0)
            (func (export "snapshot") (result i64) i64.const 0)
            (func (export "restore") (param i32 i32 i32) (result i64) i64.const 0))"#,
    )
    .unwrap();
    Guest::instantiate("request-test", &code, "request test").unwrap()
}

#[test]
fn unknown_kinds_finish_with_a_typed_refusal() {
    let mut guest = guest();
    for (id, kind) in [
        "missing",
        "rpc.unknown",
        "rpc.view",
        "rpc.query_bytes",
        "op.submit_bytes",
        "chat.props",
        "picture.load",
    ]
    .into_iter()
    .enumerate()
    {
        guest.answer(
            wire::Request {
                id: id as u64,
                kind: kind.into(),
                payload: Vec::new(),
            },
            &None,
        );
        let Some(wire::Event::Response {
            id: answered,
            result: Err(refusal),
            done,
        }) = guest.pending.pop()
        else {
            panic!("{kind} must answer a refusal");
        };
        assert_eq!(answered, id as u64);
        assert!(done);
        assert_eq!(refusal.reason, "unknown_request");
        assert!(refusal.sentence.contains(kind));
    }
    assert!(guest.pending.is_empty());
}

/// Every kind in `doors::ALL` has a handler on this side: none of them is
/// `unknown_request`, whatever else a bare guest with no node refuses it
/// for. A door added to the list without a handler fails here.
#[test]
fn every_door_is_answered() {
    for (id, kind) in doors::ALL.iter().enumerate() {
        let mut guest = guest();
        guest.answer(
            wire::Request {
                id: id as u64,
                kind: (*kind).into(),
                payload: doors::encode(&doors::Call {
                    target: "registry".into(),
                    body: Vec::new(),
                }),
            },
            &None,
        );
        let refused = match guest.pending.pop() {
            Some(wire::Event::Response {
                result: Err(refusal),
                ..
            }) => Some(refusal.reason),
            _ => None,
        };
        assert_ne!(
            refused.as_deref(),
            Some("unknown_request"),
            "{kind} has no handler"
        );
    }
}

/// A node door routes to the node handler, which answers for the missing
/// node before it reads the request; what the request says is judged by
/// `query` itself, below.
#[test]
fn node_doors_answer_for_the_missing_node_first() {
    let mut guest = guest();
    assert!(answer(&mut guest, "rpc", "query", 7, b"not borsh"));
    assert!(matches!(guest.pending.pop(), Some(wire::Event::Response {
        id: 7, result: Err(refusal), done: true
    }) if refusal.reason == "not_connected"));
}

#[tokio::test]
async fn binary_queries_preserve_signed_payloads_raw_replies_and_node_refusals() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let payload = vec![0, 255, 128, 3];
    let expected = payload.clone();
    let server = std::thread::spawn(move || {
        for rejected in [false, true] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, "POST /v1/query HTTP/1.1\r\n");
            let mut length = None;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            let query: backend::noded::Query = abi::decode(&body).unwrap();
            assert_eq!(query.layer, backend::Layer::Preconfirmed);
            let frame: backend::noded::Frame = abi::decode(&query.frame).unwrap();
            assert_eq!(frame.body.target, "registry");
            assert_eq!(frame.body.network, b"test-network");
            assert_eq!(frame.body.payload, expected);
            assert_eq!(frame.proof.len(), 64);
            let (status, body) = if rejected {
                (
                    "400 Bad Request",
                    abi::encode(&abi::Refusal::new("query_denied", "no read")),
                )
            } else {
                ("200 OK", abi::encode(&vec![255u8, 0, 129]))
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let ask = call("registry", &payload);
    assert_eq!(
        query(node.clone(), ask.clone()).await.unwrap(),
        [255, 0, 129]
    );
    let refused = query(node.clone(), ask).await.unwrap_err();
    assert_eq!(refused.reason, "query_denied");
    assert_eq!(refused.sentence, "no read");
    server.join().unwrap();

    for ask in [
        b"registry".to_vec(),
        Vec::new(),
        call("", b""),
        call("../registry", b""),
    ] {
        assert_eq!(
            query(node.clone(), ask).await.unwrap_err().reason,
            "malformed_request"
        );
    }
}

fn node_server(
    status: &str,
    body: Vec<u8>,
    expected_path: &'static str,
    expected_ttl: Option<u64>,
) -> (node::Node, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let status = status.to_owned();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut reader = std::io::BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), expected_path);
        let mut length = 0;
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
        if let Some(ttl) = expected_ttl {
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({"ttl_days": ttl})
            );
        }
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    });
    (node, server)
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
        "200 OK",
        abi::encode(&expected),
        "GET /v1/status HTTP/1.1",
        None,
    );
    let answer = status(node.clone(), Vec::new()).await.unwrap();
    let decoded: doors::NodeStatus = doors::decode(&answer).unwrap();
    assert_eq!(
        (
            decoded.network.as_str(),
            decoded.height,
            decoded.root,
            decoded.contract
        ),
        ("test-network", 201, [3; 32], 1)
    );
    server.join().unwrap();
    assert_eq!(
        status(node, b"{}".to_vec()).await.unwrap_err().reason,
        "malformed_request"
    );
    let (node, server) = node_server(
        "400 Bad Request",
        abi::encode(&abi::Refusal::new("status_denied", "private node")),
        "GET /v1/status HTTP/1.1",
        None,
    );
    assert_eq!(
        status(node, Vec::new()).await.unwrap_err().reason,
        "status_denied"
    );
    server.join().unwrap();
}

#[tokio::test]
async fn block_doors_carry_the_archive_s_blocks_as_door_types() {
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
        }],
    };
    let (node, server) = node_server(
        "200 OK",
        abi::encode(&vec![finalized.clone()]),
        "POST /v1/blocks HTTP/1.1",
        None,
    );
    let page = doors::encode(&doors::BlockPage {
        before: Some(10),
        limit: 1,
    });
    let answer = blocks(node, page).await.unwrap();
    let decoded: Vec<doors::Block> = doors::decode(&answer).unwrap();
    assert_eq!(
        decoded,
        vec![doors::Block {
            height: 9,
            id: [1; 32],
            parent: [2; 32],
            time: 1_000,
            epoch: 0,
            proposer: Some(vec![3; 32]),
            txs: vec![doors::Tx {
                hash: [4; 32],
                signer: vec![5; 32],
                seq: 6,
                target: "chat".into(),
                payload: vec![7],
            }],
        }]
    );
    server.join().unwrap();

    let (node, server) = node_server(
        "200 OK",
        abi::encode(&None::<backend::noded::Finalized>),
        "POST /v1/block HTTP/1.1",
        None,
    );
    let answer = block(node.clone(), doors::encode(&doors::BlockRef::Id([8; 32])))
        .await
        .unwrap();
    assert_eq!(
        doors::decode::<Option<doors::Block>>(&answer).unwrap(),
        None
    );
    server.join().unwrap();
    assert_eq!(
        block(node, b"x".to_vec()).await.unwrap_err().reason,
        "malformed_request"
    );
}

/// `BlobGet`'s door is declared `door!(BlobGet, "blob.get", String, Vec<u8>)`
/// — the standard macro, which borsh-wraps the reply on both ends, same as
/// every other door here. A view's generic `Door::decode_reply` expects that
/// wrapping on every blob it fetches, not just the ones this test happens to
/// exercise.
#[tokio::test]
async fn blob_get_answers_borsh_wrapped_bytes_a_view_can_decode() {
    let mut framed = b"sha256\0".to_vec();
    framed.extend_from_slice(b"blob body bytes");
    let (node, server) = node_server(
        "200 OK",
        abi::encode(&Some(framed)),
        "POST /v1/blob/get HTTP/1.1",
        None,
    );
    let ask = doors::encode(&format!("sha256:{}", "00".repeat(32)));
    let answer = blob_get(node, ask).await.unwrap();
    let decoded: Vec<u8> = doors::decode(&answer).unwrap();
    assert_eq!(decoded, b"blob body bytes");
    server.join().unwrap();
}

#[tokio::test]
async fn invite_preserves_blob_notes_ttl_and_typed_refusals() {
    let (node, server) = node_server(
        "200 OK",
        br#"{"invite":"paste-me","notes":[{"reason":"local_only","sentence":"Use on this box"}]}"#
            .to_vec(),
        "POST /v1/invite HTTP/1.1",
        Some(7),
    );
    let mint = |ttl_days| doors::encode(&doors::Mint { ttl_days });
    let answer = invite(node.clone(), mint(7)).await.unwrap();
    let decoded: doors::Minted = doors::decode(&answer).unwrap();
    assert_eq!(
        decoded,
        doors::Minted {
            invite: "paste-me".into(),
            notes: vec![doors::Note {
                reason: "local_only".into(),
                sentence: "Use on this box".into()
            }]
        }
    );
    server.join().unwrap();
    for ask in [Vec::new(), mint(0), b"7".to_vec()] {
        assert_eq!(
            invite(node.clone(), ask).await.unwrap_err().reason,
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
        let (node, server) = node_server(http, body, "POST /v1/invite HTTP/1.1", Some(1));
        let refusal = invite(node, mint(1)).await.unwrap_err();
        assert_eq!(refusal.reason, reason);
        server.join().unwrap();
    }
    let (node, server) = node_server(
        "404 Not Found",
        Vec::new(),
        "POST /v1/invite HTTP/1.1",
        Some(1),
    );
    assert_eq!(
        invite(node, mint(1)).await.unwrap_err().sentence,
        "This node doesn't mint invites."
    );
    server.join().unwrap();
    // an unrelated non-JSON non-2xx (a proxy's 502, say) must not be folded
    // into the same message: only http_error + 404 gets the friendlier text.
    let (node, server) = node_server(
        "502 Bad Gateway",
        Vec::new(),
        "POST /v1/invite HTTP/1.1",
        Some(1),
    );
    let refusal = invite(node, mint(1)).await.unwrap_err();
    assert_eq!(refusal.reason, "http_error");
    assert!(refusal.sentence.starts_with("502"));
    server.join().unwrap();
}

#[test]
fn system_kinds_route_to_the_node_handler() {
    for kind in ["status", "invite"] {
        let mut guest = guest();
        assert!(answer(&mut guest, "rpc", kind, 19, b"invalid"));
        assert!(matches!(guest.pending.pop(), Some(wire::Event::Response {
            id: 19, result: Err(refusal), done: true
        }) if refusal.reason == "not_connected"));
    }
}

#[test]
fn host_props_is_program_independent_and_tracks_updates() {
    let mut guest = guest();
    let props = Some(super::super::props(
        true,
        true,
        "test-network",
        "abcd",
        "http://127.0.0.1:19001",
    ));
    guest.answer(
        wire::Request {
            id: 22,
            kind: "host.props".into(),
            payload: Vec::new(),
        },
        &props,
    );
    assert_eq!(guest.props_subscription, Some(22));
    let Some(wire::Event::Response {
        result: Ok(bytes),
        done: false,
        ..
    }) = guest.pending.pop()
    else {
        panic!("props must stay live")
    };
    let decoded: doors::Session = doors::decode(&bytes).unwrap();
    assert_eq!(decoded.endpoint, "http://127.0.0.1:19001");
    assert_eq!(decoded.account, "abcd");
    assert!(decoded.dark);
    let changed = Some(super::super::props(
        false,
        true,
        "test-network",
        "abcd",
        "http://127.0.0.1:19001",
    ));
    guest.sync_props(&changed);
    assert!(matches!(
        guest.pending.pop(),
        Some(wire::Event::Response {
            id: 22,
            result: Ok(_),
            done: false
        })
    ));
}

/// Reproduces "Couldn't create this channel: Unexpected length of input":
/// `create_channel` mints its id with `host.ask::<Id>("channel".into())`
/// before it ever submits an op, and `Id` (`door!(Id, "host.id", String,
/// String)`) answers borsh like every other door — so the reply the view
/// decodes with `Id::decode_reply` (`doors::decode::<String>`) must be one
/// it, not raw UTF-8 with no length prefix, which the guest happily builds
/// and only the view's decode fails on later.
#[test]
fn host_id_answers_a_borsh_string_a_view_can_decode() {
    let mut guest = guest();
    guest.answer(
        wire::Request {
            id: 9,
            kind: "host.id".into(),
            payload: doors::encode(&"channel".to_string()),
        },
        &None,
    );
    let Some(wire::Event::Response {
        id: 9,
        result: Ok(bytes),
        done: true,
    }) = guest.pending.pop()
    else {
        panic!("host.id must answer once");
    };
    let minted: String =
        doors::decode(&bytes).expect("a view decodes `host.id`'s reply as one borsh String");
    assert!(minted.starts_with("channel-"), "{minted}");
}
