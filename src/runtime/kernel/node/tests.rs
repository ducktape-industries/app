use super::*;

fn guest() -> Guest {
    let code = wasmtime::Module::new(
        crate::runtime::guest::engine(),
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0)
            (func (export "snapshot") (result i64) i64.const 0)
            (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#,
    )
    .unwrap();
    let mut guest = Guest::instantiate("node-test", &code, "node test").unwrap();
    guest.capabilities = Capability::ALL.to_vec();
    guest
}

/// The one refusal `id` got, as (code, message).
fn refused(guest: &mut Guest, id: u64) -> (String, String) {
    match guest.pending.pop() {
        Some(wire::Event::Response {
            id: answered,
            result: Err(refusal),
            done: true,
        }) if answered == id => (refusal.code, refusal.message),
        other => panic!("{other:?}"),
    }
}

/// Too many `module.changes` subscriptions is the same refusal as too many
/// `clock.ticks`: `subscription_limit`, said as such — not the malformed
/// request a nameless program is.
#[test]
fn too_many_changes_subscriptions_is_the_subscription_limit() {
    let mut guest = guest();
    changes(&mut guest, 1, &methods::encode(&String::new()));
    assert_eq!(
        refused(&mut guest, 1),
        (
            "malformed_request".to_owned(),
            "`module.changes` names no program".to_owned()
        )
    );
    guest.live_subscriptions = (0..MAX_SUBSCRIPTIONS as u64)
        .map(|id| (id, "chat".to_owned()))
        .collect();
    changes(&mut guest, 2, &methods::encode(&"chat".to_owned()));
    assert_eq!(
        refused(&mut guest, 2),
        (
            "subscription_limit".to_owned(),
            "too many `module.changes` subscriptions".to_owned()
        )
    );
}

/// A node that applies every op it is handed, but whose first answer on
/// route `lost` goes missing on the way back (a 502 from the proxy in front
/// of it). Answers `/v1/get` with the signer's next sequence, counts the
/// submits.
fn applying_node_with_a_lost_answer(
    lost: &'static str,
) -> (Node, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead as _, Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let submits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = submits.clone();
    std::thread::spawn(move || {
        let mut seq = 0u64;
        let mut lose = true;
        for mut stream in listener.incoming().flatten() {
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let route = line.split(' ').nth(1).unwrap().to_owned();
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
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let answer = match route.as_str() {
                noded::route::GET => abi::encode(&Some(abi::encode(&seq))),
                noded::route::SUBMIT => {
                    let frame: noded::Frame = abi::decode(&body).unwrap();
                    assert_eq!(frame.body.seq, seq, "signed at the sequence the node named");
                    seq += 1;
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    abi::encode(&noded::Receipt {
                        program: "registry".into(),
                        outcome: abi::Outcome::Applied { output: Vec::new() },
                        events: Vec::new(),
                        nested: Vec::new(),
                    })
                }
                other => panic!("{other}"),
            };
            let (status, body) = match route == lost && std::mem::take(&mut lose) {
                true => ("502 Bad Gateway", b"upstream went away".to_vec()),
                false => ("200 OK", answer),
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
    (node, submits)
}

/// An `op.submit` whose answer was lost is not asked again: the node may
/// have applied it, and a retry signs at the next sequence — a second
/// apply, not the same one. The view hears the loss and decides.
#[tokio::test]
async fn a_submit_whose_answer_was_lost_is_not_submitted_again() {
    use commonware_cryptography::Signer as _;
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::SUBMIT);
    let answer = until_answered(NODE_RETRY_BUDGET, unsent, || {
        submit(node.clone(), call_registry())
    })
    .await;
    assert_eq!(
        answer.unwrap_err().code,
        "node_failed",
        "the loss is the answer, named as the node's"
    );
    assert_eq!(
        submits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one submit: the op was not applied twice"
    );
}

fn call_registry() -> Vec<u8> {
    methods::encode(&methods::Call {
        target: "registry".into(),
        body: vec![1, 2, 3],
    })
}

/// The sequence read comes before any frame is signed: its lost answer is
/// asked again, and the one submit that follows goes through.
#[tokio::test]
async fn a_submit_whose_sequence_read_was_lost_is_asked_again() {
    use commonware_cryptography::Signer as _;
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::GET);
    let answer = until_answered(NODE_RETRY_BUDGET, unsent, || {
        submit(node.clone(), call_registry())
    })
    .await;
    assert_eq!(
        answer,
        Ok(Vec::new()),
        "nothing was sent: the read is retried"
    );
    assert_eq!(submits.load(std::sync::atomic::Ordering::SeqCst), 1);
}
