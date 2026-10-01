use super::*;
use crate::runtime::kernel::tests::{guest, read_request, respond};

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
            refusal::MALFORMED_REQUEST.to_owned(),
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
            refusal::SUBSCRIPTION_LIMIT.to_owned(),
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
            let (line, body) = read_request(&mut stream);
            let route = line.split(' ').nth(1).unwrap().to_owned();
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
            respond(&mut stream, status, &body);
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
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::SUBMIT);
    let (answer, attempts) = until_answered(NODE_RETRY_BUDGET, unsent, || {
        submit(node.clone(), call_registry())
    })
    .await;
    assert_eq!(attempts, 1);
    assert_eq!(
        answer.unwrap_err().code,
        refusal::NODE_FAILED,
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
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::GET);
    let (answer, attempts) = until_answered(NODE_RETRY_BUDGET, unsent, || {
        submit(node.clone(), call_registry())
    })
    .await;
    assert_eq!(
        answer,
        Ok(Vec::new()),
        "nothing was sent: the read is retried"
    );
    assert_eq!(attempts, 2, "the lost read, then the one that went through");
    assert_eq!(submits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// A node that takes its time over a sequence read and a submit (`/v1/get`
/// answers after `takes`, `/v1/submit` moves the sequence after `takes`),
/// serves every connection on its own thread, and refuses a frame out of
/// sequence as the real one does (`reason::SEQUENCE`). Counts the frames
/// it applied.
fn slow_node(takes: std::time::Duration) -> (Node, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let applied = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = applied.clone();
    std::thread::spawn(move || {
        let seq = std::sync::Arc::new(std::sync::Mutex::new(0u64));
        for mut stream in listener.incoming().flatten() {
            let (seq, counted) = (seq.clone(), counted.clone());
            std::thread::spawn(move || {
                let (line, body) = read_request(&mut stream);
                let route = line.split(' ').nth(1).unwrap().to_owned();
                let answer = match route.as_str() {
                    noded::route::GET => {
                        let next = *seq.lock().unwrap();
                        std::thread::sleep(takes);
                        abi::encode(&Some(abi::encode(&next)))
                    }
                    noded::route::SUBMIT => {
                        let frame: noded::Frame = abi::decode(&body).unwrap();
                        std::thread::sleep(takes);
                        let mut next = seq.lock().unwrap();
                        let outcome = match frame.body.seq == *next {
                            true => {
                                *next += 1;
                                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                abi::Outcome::Applied { output: Vec::new() }
                            }
                            false => abi::Outcome::Rejected(abi::Refusal::new(
                                abi::reason::SEQUENCE,
                                format!(
                                    "sequence {} is not the signer's next, {next}",
                                    frame.body.seq
                                ),
                            )),
                        };
                        abi::encode(&noded::Receipt {
                            program: "registry".into(),
                            outcome,
                            events: Vec::new(),
                            nested: Vec::new(),
                        })
                    }
                    other => panic!("{other}"),
                };
                respond(&mut stream, "200 OK", &answer);
            });
        }
    });
    (node, applied)
}

/// Two `op.submit`s in flight at once (a reaction burst) both land: a
/// signer's writes take their sequence one after another, the second read
/// only once the first frame's receipt is back. Without the signer's turn,
/// or with it let go before the submit, both read the same next sequence
/// and the node refuses the second.
#[tokio::test]
async fn two_submits_in_flight_take_one_sequence_each() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, applied) = slow_node(std::time::Duration::from_millis(300));
    let (first, second) = tokio::join!(
        submit(node.clone(), call_registry()),
        submit(node.clone(), call_registry())
    );
    assert_eq!(first, Ok(Vec::new()));
    assert_eq!(
        second,
        Ok(Vec::new()),
        "signed at the sequence the first moved to"
    );
    assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 2);
}
