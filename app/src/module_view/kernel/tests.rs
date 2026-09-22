use super::*;
use base64::Engine as _;
use std::io::{BufRead as _, Read as _, Write as _};

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
        "rpc.stream",
        "rpc.admin",
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

#[test]
fn binary_queries_use_the_node_handler() {
    let mut guest = guest();
    assert!(answer(&mut guest, "rpc", "query_bytes", 7, b"not JSON"));
    assert!(matches!(guest.pending.pop(), Some(wire::Event::Response {
        id: 7, result: Err(refusal), done: true
    }) if refusal.reason == "malformed_request"));
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
    let ask = serde_json::json!({
        "target": "registry",
        "body_b64": base64::engine::general_purpose::STANDARD.encode(payload),
    });
    assert_eq!(
        query_bytes(node.clone(), ask.clone()).await.unwrap(),
        [255, 0, 129]
    );
    let refused = query_bytes(node.clone(), ask).await.unwrap_err();
    assert_eq!(refused.reason, "query_denied");
    assert_eq!(refused.sentence, "no read");
    server.join().unwrap();

    for ask in [
        serde_json::json!({"target": "registry"}),
        serde_json::json!({"target": "registry", "body_b64": 7}),
        serde_json::json!({"target": "registry", "body_b64": "!"}),
        serde_json::json!({"target": "../registry", "body_b64": ""}),
    ] {
        assert_eq!(
            query_bytes(node.clone(), ask).await.unwrap_err().reason,
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
    };
    let (node, server) = node_server(
        "200 OK",
        abi::encode(&expected),
        "GET /v1/status HTTP/1.1",
        None,
    );
    let answer = status(node.clone(), serde_json::Value::Null).await.unwrap();
    assert_eq!(
        abi::decode::<backend::noded::Status>(&answer).unwrap(),
        expected
    );
    server.join().unwrap();
    assert_eq!(
        status(node, serde_json::json!({}))
            .await
            .unwrap_err()
            .reason,
        "malformed_request"
    );
    let (node, server) = node_server(
        "400 Bad Request",
        abi::encode(&abi::Refusal::new("status_denied", "private node")),
        "GET /v1/status HTTP/1.1",
        None,
    );
    assert_eq!(
        status(node, serde_json::Value::Null)
            .await
            .unwrap_err()
            .reason,
        "status_denied"
    );
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
    let answer = invite(node.clone(), serde_json::json!({"ttl_days": 7}))
        .await
        .unwrap();
    let decoded: (String, Vec<(String, String)>) = abi::decode(&answer).unwrap();
    assert_eq!(
        decoded,
        (
            "paste-me".into(),
            vec![("local_only".into(), "Use on this box".into())]
        )
    );
    server.join().unwrap();
    for ask in [
        serde_json::json!({}),
        serde_json::json!({"ttl_days": 0}),
        serde_json::json!({"ttl_days": -1}),
        serde_json::json!({"ttl_days": "7"}),
    ] {
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
        ("404 Not Found", Vec::new(), "http_error"),
    ] {
        let (node, server) = node_server(http, body, "POST /v1/invite HTTP/1.1", Some(1));
        assert_eq!(
            invite(node, serde_json::json!({"ttl_days": 1}))
                .await
                .unwrap_err()
                .reason,
            reason
        );
        server.join().unwrap();
    }
}

#[test]
fn system_kinds_route_and_reject_invalid_json() {
    for kind in ["status", "invite"] {
        let mut guest = guest();
        assert!(answer(&mut guest, "rpc", kind, 19, b"invalid JSON"));
        assert!(matches!(guest.pending.pop(), Some(wire::Event::Response {
            id: 19, result: Err(refusal), done: true
        }) if refusal.reason == "malformed_request"));
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
            payload: b"null".to_vec(),
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
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded["endpoint"], "http://127.0.0.1:19001");
    assert_eq!(decoded["account"], "abcd");
    assert_eq!(decoded["dark"], true);
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
