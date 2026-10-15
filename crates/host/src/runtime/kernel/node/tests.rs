use super::*;
use crate::runtime::kernel::tests::{Mode, fake_node, read_request, respond, view};

/// The one refusal `id` got, as (code, message).
fn refused(view: &mut View, id: u64) -> (String, String) {
    match view.pending.pop() {
        Some(wire::Event::Response {
            id: answered,
            result: Err(refusal),
            done: true,
        }) if answered == id => (refusal.code, refusal.message),
        other => panic!("{other:?}"),
    }
}

/// A `program.changes` that names no program is malformed.
#[test]
fn a_nameless_changes_subscription_is_malformed() {
    let mut view = view();
    changes(&mut view, 1, &methods::encode(&String::new()));
    assert_eq!(
        refused(&mut view, 1),
        (
            refusal::MALFORMED_REQUEST.to_owned(),
            "`program.changes` names no program".to_owned()
        )
    );
}

fn status_at(anchor: noded::Tip) -> noded::Status {
    noded::Status {
        network: "test-network".into(),
        time: 1,
        block_time_ms: 1_000,
        epoch_length: 100,
        height: anchor.height,
        tip: anchor.id,
        root: abi::Root([0; 32]),
        epoch: 0,
        identity: Vec::new(),
        contract: noded::NODE_CONTRACT,
        genesis: [0; 32],
        roles: crate::backend::test_roles(),
    }
}

const TIP: noded::Tip = noded::Tip {
    height: 7,
    id: [3; 32],
};

fn applied() -> noded::Receipt {
    noded::Receipt {
        program: "programs".into(),
        outcome: abi::Outcome::Applied { output: Vec::new() },
        events: Vec::new(),
        nested: Vec::new(),
    }
}

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
        let mut lose = true;
        for mut stream in listener.incoming().flatten() {
            let (line, body) = read_request(&mut stream);
            let route = line.split(' ').nth(1).unwrap().to_owned();
            let answer = match route.as_str() {
                noded::route::STATUS => abi::encode(&status_at(TIP)),
                noded::route::SUBMIT => {
                    let submit: noded::Submit = abi::decode(&body).unwrap();
                    assert_eq!(
                        submit.preconfirm,
                        noded::Preconfirm::On,
                        "a view's write is preconfirmed"
                    );
                    let frame: noded::Frame = abi::decode(&submit.frame).unwrap();
                    assert_eq!(frame.body.anchor, TIP, "anchored at the tip the node named");
                    assert!(frame.body.salt.is_some(), "a write carries a salt");
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    abi::encode(&noded::Submitted::Preconfirmed(applied()))
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

#[tokio::test]
async fn a_submit_whose_answer_was_lost_is_not_submitted_again() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::SUBMIT);
    let answer = submit(node.clone(), call_registry()).await;
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
        target: "programs".into(),
        body: vec![1, 2, 3],
    })
}

#[tokio::test]
async fn a_submit_whose_anchor_read_was_lost_is_refused_unsent() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::STATUS);
    let answer = submit(node.clone(), call_registry()).await;
    assert_eq!(
        answer.unwrap_err().code,
        refusal::RPC_CLIENT,
        "nothing was sent: the view may ask again"
    );
    assert_eq!(submits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// A view's `program.query` to a node that takes the request and never
/// answers holds its in-flight slot until the view cancels it, and the
/// cancel gives the slot back.
#[test]
fn a_query_to_a_stalled_node_holds_its_slot_until_the_view_cancels_it() {
    let fake = fake_node(vec![(noded::route::QUERY, Mode::Stall)]);
    let mut view = view();
    let replies = view.replies.clone();
    let mut changed = view.replies.changes();
    start(&mut view, 1, None, async move {
        let answer = query(fake.node.clone(), call_registry()).await;
        replies.item(1, answer, true);
    });
    assert!(view.replies.answer_owed());
    changed.borrow_and_update();
    view.tasks.clear();
    futures::executor::block_on(async {
        while view.replies.answer_owed() {
            changed.changed().await.unwrap();
        }
    });
    let mut answered = Vec::new();
    assert_eq!(view.replies.drain_into(&mut answered), 0, "no answer came");
}

/// A recovering node holds a request and answers once it is back: the
/// answer is waited for, however long the hold.
#[tokio::test]
async fn a_held_answer_lands() {
    let fake = fake_node(vec![(
        noded::route::QUERY,
        Mode::After(
            std::time::Duration::from_millis(1500),
            abi::encode(&vec![7u8]),
        ),
    )]);
    let answer = query(fake.node.clone(), call_registry()).await;
    assert_eq!(answer, Ok(vec![7]), "the held answer is the answer");
}

/// A node that answers a submit with a redirect to another host does not
/// get the signed frame sent on there: the redirect is the node's answer,
/// `node_failed`, and the submit is not signed again.
#[tokio::test]
async fn a_redirected_submit_goes_nowhere_else() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let elsewhere = fake_node(vec![(
        noded::route::SUBMIT,
        Mode::Answer(abi::encode(&noded::Submitted::Preconfirmed(
            noded::Receipt {
                program: "programs".into(),
                outcome: abi::Outcome::Applied { output: Vec::new() },
                events: Vec::new(),
                nested: Vec::new(),
            },
        ))),
    )]);
    let to = format!(
        "{}{}",
        elsewhere.node.client.endpoint(),
        noded::route::SUBMIT
    );
    let node = fake_node(vec![
        (
            noded::route::STATUS,
            Mode::Answer(abi::encode(&status_at(TIP))),
        ),
        (noded::route::SUBMIT, Mode::Redirect(to)),
    ]);
    let answer = submit(node.node.clone(), call_registry()).await;
    assert_eq!(
        elsewhere.accepted.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the signed frame went on to the host the redirect named"
    );
    assert_eq!(answer.unwrap_err().code, refusal::NODE_FAILED);
}

/// A node that hangs up on a read is asked once: the transport failure is
/// the view's answer.
#[tokio::test]
async fn a_node_that_hangs_up_is_asked_once() {
    let fake = fake_node(vec![(noded::route::QUERY, Mode::Close)]);
    let answer = query(fake.node.clone(), call_registry()).await;
    assert!(transport_failed(&answer.unwrap_err()));
    assert_eq!(fake.accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// `blob.get` of a blob bigger than any one answer the app once read is
/// answered whole.
#[tokio::test]
async fn a_large_blob_is_read_whole() {
    let body = vec![7u8; 17 << 20];
    let framed = [format!("blob {}\0", body.len()).as_bytes(), &body].concat();
    let fake = fake_node(vec![(
        noded::route::BLOB_GET,
        Mode::Answer(abi::encode(&Some(framed))),
    )]);
    let ask = methods::encode(&format!("sha256:{}", "00".repeat(32)));
    let answer = blob_get(fake.node.clone(), ask).await.unwrap();
    assert_eq!(
        methods::decode::<Option<Vec<u8>>>(&answer).unwrap(),
        Some(body)
    );
}

fn node_refusing_duplicates() -> (Node, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let applied_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = applied_count.clone();
    std::thread::spawn(move || {
        let mut carried = std::collections::HashSet::new();
        for mut stream in listener.incoming().flatten() {
            let (line, body) = read_request(&mut stream);
            let route = line.split(' ').nth(1).unwrap().to_owned();
            let answer = match route.as_str() {
                noded::route::STATUS => abi::encode(&status_at(TIP)),
                noded::route::SUBMIT => {
                    let submit: noded::Submit = abi::decode(&body).unwrap();
                    let frame: noded::Frame = abi::decode(&submit.frame).unwrap();
                    let body = frame.body;
                    let id = abi::encode(&(
                        body.signer,
                        body.anchor,
                        body.salt,
                        body.target,
                        body.payload,
                    ));
                    let receipt = match carried.insert(id) {
                        true => {
                            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            applied()
                        }
                        false => noded::Receipt {
                            outcome: abi::Outcome::Rejected(abi::Refusal::new(
                                abi::reason::DUPLICATE,
                                "a block already carried this frame",
                            )),
                            ..applied()
                        },
                    };
                    abi::encode(&noded::Submitted::Preconfirmed(receipt))
                }
                other => panic!("{other}"),
            };
            respond(&mut stream, "200 OK", &answer);
        }
    });
    (node, applied_count)
}

#[tokio::test]
async fn two_identical_submits_in_flight_are_two_frames() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(41)).await;
    let (node, applied) = node_refusing_duplicates();
    let (first, second) = tokio::join!(
        submit(node.clone(), call_registry()),
        submit(node.clone(), call_registry())
    );
    assert_eq!(first, Ok(Vec::new()));
    assert_eq!(second, Ok(Vec::new()), "each write carries its own salt");
    assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// A node whose `/v1/changes/<program>` socket sends one change at each of
/// `heights`, then stays open for as long as the client holds it.
fn changes_node(heights: &'static [u64]) -> Node {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let node = Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    handle().spawn(async move {
        use futures::{SinkExt as _, StreamExt as _};
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                // anything but the socket (a `chain.heads` status read) is dropped
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                for &height in heights {
                    // each block wrote one row, as a program's block does
                    let change = noded::Change {
                        height,
                        root: abi::Root([0; 32]),
                        writes: vec![(format!("row/{height}").into_bytes(), None)],
                    };
                    let sent =
                        tokio_tungstenite::tungstenite::Message::Binary(abi::encode(&change));
                    socket.send(sent).await.unwrap();
                }
                while socket.next().await.is_some() {}
            });
        }
    });
    node
}

/// A connect inside the network, under a view that kept its code: its
/// `program.changes` subscription goes on, on the new node, so its follower
/// hears `None` (re-read what moved meanwhile) and then the new node's
/// blocks; its `chain.heads` goes on too, and ends with why when the new
/// node answers no status (this one serves only changes); a request still
/// running on the
/// old connection is refused `stale_connection`. Before, the subscriptions
/// were refused too, which ends a view's stream: the view went deaf to
/// every block after the connect.
#[test]
fn a_reconnect_opens_a_views_subscriptions_again_on_the_new_node() {
    let mut view = view();
    view.targets = vec!["chat".into()];
    follow_changes(&mut view, 1, changes_node(&[]), "chat".into());
    spawn_reply(&mut view, 2, std::future::pending::<Answer>());
    follow_heads(&mut view, 3, changes_node(&[]));
    let new = changes_node(&[7]);
    let rev = view.connection.rev + 1;
    view.reconnect(&crate::runtime::Connection {
        client: Some(new.client),
        network: new.network,
        chain: String::new(),
        rev,
    });
    assert_eq!(refused(&mut view, 2).0, refusal::STALE_CONNECTION);
    assert_eq!(view.pending, [], "a subscription was refused");
    let following: Vec<u64> = view
        .tasks
        .iter()
        .filter_map(|(id, task)| task.follows.as_ref().map(|_| *id))
        .collect();
    assert_eq!(following, [1, 3], "both subscriptions run again");
    let mut items = Vec::new();
    for _ in 0..200 {
        view.replies.drain_into(&mut items);
        if items.len() >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let item = |change: Option<methods::Change>| wire::Event::Response {
        id: 1,
        result: Ok(methods::encode(&change)),
        done: false,
    };
    let block = methods::Change {
        height: 7,
        keys: vec![b"row/7".to_vec()],
    };
    let (changes, heads): (Vec<_>, Vec<_>) = items
        .into_iter()
        .partition(|event| matches!(event, wire::Event::Response { id: 1, .. }));
    assert_eq!(
        changes,
        [item(None), item(Some(block))],
        "the follower is told to re-read, then hears the new node's block with the keys it wrote"
    );
    assert!(
        matches!(&heads[..], [wire::Event::Response { id: 3, result: Err(error), done: true }] if error.code == refusal::NODE_FAILED),
        "{heads:?}"
    );
    assert_eq!(view.live_subscriptions, [(1, "chat".to_owned())]);
}
