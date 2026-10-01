use super::*;
use crate::runtime::kernel::tests::{Mode, fake_node, guest, read_request, respond};

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
    let (answer, attempts) =
        until_answered(NODE_RETRY_BUDGET, noded::ANSWER_DEADLINE, unsent, || {
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
    let (answer, attempts) =
        until_answered(NODE_RETRY_BUDGET, noded::ANSWER_DEADLINE, unsent, || {
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

/// A view's `module.query` to a node that takes the request and never
/// answers is refused once its attempt's deadline passes, and gives its
/// in-flight slot back: before, it waited for good and held the slot.
#[test]
fn a_query_to_a_stalled_node_is_refused_at_its_deadline_and_frees_its_slot() {
    let fake = fake_node(vec![(noded::route::QUERY, Mode::Stall)]);
    let mut guest = guest();
    let replies = guest.replies.clone();
    let asked = std::time::Instant::now();
    start(&mut guest, 1, async move {
        let budget = std::time::Duration::from_secs(1);
        let per_attempt = std::time::Duration::from_millis(300);
        let (answer, _) = until_answered(budget, per_attempt, transport_failed, || {
            query(fake.node.clone(), call_registry())
        })
        .await;
        replies.item(1, answer, true);
    });
    assert!(guest.replies.any_in_flight());
    while guest.replies.any_in_flight() {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(10),
            "the stalled ask still holds its slot"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut answered = Vec::new();
    guest.replies.drain_into(&mut answered).unwrap();
    guest.pending = answered;
    assert_eq!(
        refused(&mut guest, 1),
        (
            refusal::RPC_CLIENT.to_owned(),
            crate::runtime::NODE_UNREACHABLE.to_owned()
        )
    );
}

/// A recovering node holds a request and answers once it is back, which
/// can outlast the whole retry budget: the answer still lands. The
/// deadline is each attempt's own; one taken from the budget's remainder
/// (`budget - elapsed`) would cut this answer off as a failure.
#[tokio::test]
async fn a_held_answer_outlasting_the_budget_still_lands() {
    let fake = fake_node(vec![(
        noded::route::QUERY,
        Mode::After(
            std::time::Duration::from_millis(1500),
            abi::encode(&vec![7u8]),
        ),
    )]);
    let budget = std::time::Duration::from_secs(1);
    let per_attempt = std::time::Duration::from_secs(3);
    let (answer, attempts) = until_answered(budget, per_attempt, transport_failed, || {
        query(fake.node.clone(), call_registry())
    })
    .await;
    assert_eq!(answer, Ok(vec![7]), "the held answer is the answer");
    assert_eq!(attempts, 1);
}

/// A node that hangs up on a read is asked again within the budget; the
/// view hears it as unreachable once the budget is spent.
#[tokio::test]
async fn a_node_that_hangs_up_is_asked_again_within_the_budget() {
    let fake = fake_node(vec![(noded::route::QUERY, Mode::Close)]);
    let budget = std::time::Duration::from_millis(1500);
    let (answer, attempts) =
        until_answered(budget, noded::ANSWER_DEADLINE, transport_failed, || {
            query(fake.node.clone(), call_registry())
        })
        .await;
    assert_eq!(answer.unwrap_err().code, refusal::RPC_CLIENT);
    assert_eq!(attempts, 2, "asked at once and a second later");
    assert_eq!(fake.accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// `blob.get` from a node streaming more than the view may read is refused
/// `too_large` as the bytes pass the cap, with what the node streamed past
/// it left unread: chunked, or declared by a `Content-Length` (refused
/// before any of the body is read).
#[tokio::test]
async fn a_blob_streamed_past_the_cap_is_refused_unread() {
    let len = 4 * MAX_BLOB_BYTES;
    let ask = methods::encode(&format!("sha256:{}", "00".repeat(32)));
    for declared in [false, true] {
        let mut fake = fake_node(vec![(
            noded::route::BLOB_GET,
            Mode::Stream { len, declared },
        )]);
        let refusal = blob_get(fake.node.clone(), ask.clone()).await.unwrap_err();
        assert_eq!(refusal.code, refusal::TOO_LARGE, "declared: {declared}");
        let streamed =
            tokio::time::timeout(std::time::Duration::from_secs(10), fake.streamed.recv())
                .await
                .expect("the node is still streaming")
                .unwrap();
        assert!(
            streamed < len,
            "declared: {declared}: the whole {len} bytes were read ({streamed})"
        );
    }
}

/// A `module.changes` socket the node keeps open and says nothing on is
/// taken for dead once it has been silent `idle`: closed, the view told
/// once to re-read (`None`), and opened again.
#[tokio::test]
async fn a_silent_changes_socket_is_reopened_and_the_view_told_once() {
    let fake = fake_node(vec![(noded::route::CHANGES, Mode::Silent)]);
    let replies = std::sync::Arc::new(Replies::default());
    let items = Items {
        drained: replies.drains(),
        replies: replies.clone(),
        id: 1,
    };
    let idle = std::time::Duration::from_secs(1);
    let following = tokio::spawn(follow_changes(
        fake.node.clone(),
        "chat".into(),
        items,
        idle,
    ));
    // silent from 0 s, torn down at ~1 s, reopened ~1 s later (the retry
    // delay), silent again until ~3 s
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    following.abort();
    let mut heard = Vec::new();
    replies.drain_into(&mut heard).unwrap();
    let reopened = wire::Event::Response {
        id: 1,
        result: Ok(methods::encode(&None::<u64>)),
        done: false,
    };
    assert_eq!(heard, [reopened], "one re-read, for the one silent socket");
    assert_eq!(
        fake.accepted.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the socket was opened again"
    );
}
