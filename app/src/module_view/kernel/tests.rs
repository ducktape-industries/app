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
