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

/// A host that runs no node (`OsServices::mint_invite` answers `None`)
/// refuses `invite.create` with the account gate's sentence, before any
/// request reaches the node.
#[tokio::test]
async fn a_host_that_runs_no_node_mints_no_invite() {
    let node = Node {
        client: RpcClient::new("http://127.0.0.1:1"),
        network: "test-network".into(),
    };
    let ask = methods::encode(&methods::CreateInvite { ttl_days: 7 });
    let refusal = invite(node, ask).await.unwrap_err();
    assert_eq!(refusal.code, refusal::INVITE_UNSUPPORTED);
    assert_eq!(
        refusal.message,
        "Only the machine running this node can mint its invites."
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

/// What `op.submit` answered, read as a view reads it.
fn decoded(answer: Answer) -> methods::Submitted {
    methods::decode(&answer.expect("a pooled frame is an answer")).unwrap()
}

/// The ids of the frames a node was sent, in order.
type Sent = std::sync::Arc<std::sync::Mutex<Vec<[u8; 32]>>>;

fn applying_node_with_a_lost_answer(lost: &'static str) -> (Node, Sent) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let submits = Sent::default();
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
                    counted.lock().unwrap().push(frame.body.id());
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
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::SUBMIT);
    let answer = submit(node.clone(), call_registry(), Claim::default()).await;
    let sent = submits.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "one submit: the op was not applied twice");
    assert_eq!(
        decoded(answer),
        methods::Submitted::InDoubt {
            frame: sent[0],
            until: TIP.height + 256,
        },
        "the loss is the answer: the frame the node may hold, and how long a block may carry it"
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
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
    let (node, submits) = applying_node_with_a_lost_answer(noded::route::STATUS);
    let answer = submit(node.clone(), call_registry(), Claim::default()).await;
    assert_eq!(
        answer.unwrap_err().code,
        refusal::RPC_CLIENT,
        "nothing was sent: the view may ask again"
    );
    assert!(submits.lock().unwrap().is_empty());
}

/// A node that answers the frame with its own refusal (a bad signature, a
/// stale anchor: HTTP 400) pooled nothing: the refusal is the answer.
#[tokio::test]
async fn a_frame_the_node_refuses_outright_is_refused_not_submitted() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
    let node = fake_node(vec![
        (
            noded::route::STATUS,
            Mode::Answer(abi::encode(&status_at(TIP))),
        ),
        (
            noded::route::SUBMIT,
            Mode::Refuse(abi::encode(&abi::Refusal::new(
                abi::reason::ANCHOR,
                "the frame's anchor is not one of the 256 blocks before block 900",
            ))),
        ),
    ]);
    let refusal = submit(node.node.clone(), call_registry(), Claim::default())
        .await
        .unwrap_err();
    assert_eq!(refusal.code, abi::reason::ANCHOR);
}

/// The node pooled the frame and predicts the program refuses it: the
/// answer is the refusal AND the frame, which a block may still carry, so
/// the view does not take it for a write that never went out.
#[tokio::test]
async fn a_predicted_refusal_is_still_a_pooled_frame() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
    let node = fake_node(vec![
        (
            noded::route::STATUS,
            Mode::Answer(abi::encode(&status_at(TIP))),
        ),
        (
            noded::route::SUBMIT,
            Mode::Answer(abi::encode(&noded::Submitted::Preconfirmed(
                noded::Receipt {
                    outcome: abi::Outcome::Rejected(abi::Refusal::new(
                        abi::reason::ALREADY_EXISTS,
                        "that name is taken",
                    )),
                    ..applied()
                },
            ))),
        ),
    ]);
    match decoded(submit(node.node.clone(), call_registry(), Claim::default()).await) {
        methods::Submitted::Refused { until, refusal, .. } => {
            assert_eq!(until, TIP.height + 256);
            assert_eq!(
                (refusal.code.as_str(), refusal.message.as_str()),
                (abi::reason::ALREADY_EXISTS, "that name is taken")
            );
        }
        other => panic!("{other:?}"),
    }
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
    start(&mut view, 1, None, Claim::default(), async move {
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
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
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
    let answer = submit(node.node.clone(), call_registry(), Claim::default()).await;
    assert_eq!(
        elsewhere.accepted.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the signed frame went on to the host the redirect named"
    );
    assert!(
        matches!(decoded(answer), methods::Submitted::InDoubt { until, .. } if until == TIP.height + 256),
        "the node took the frame and answered what this client does not read: it may hold it"
    );
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
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
    let (node, applied) = node_refusing_duplicates();
    let (first, second) = tokio::join!(
        submit(node.clone(), call_registry(), Claim::default()),
        submit(node.clone(), call_registry(), Claim::default())
    );
    let applied_once = |answer| match decoded(answer) {
        methods::Submitted::Applied { frame, output, .. } if output.is_empty() => frame,
        other => panic!("{other:?}"),
    };
    let (first, second) = (applied_once(first), applied_once(second));
    assert_ne!(first, second, "each write carries its own salt");
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
/// every block after the connect. The old node never answers the heads'
/// status read, so the subscription is still running when the connect
/// comes (one the old node ended first stays ended:
/// `a_reconnect_leaves_an_ended_request_ended`).
#[test]
fn a_reconnect_opens_a_views_subscriptions_again_on_the_new_node() {
    let mut view = view();
    view.targets = vec!["chat".into()];
    follow_changes(&mut view, 1, changes_node(&[]), "chat".into());
    spawn_reply(&mut view, 2, std::future::pending::<Answer>());
    let stalled = fake_node(vec![(noded::route::STATUS, Mode::Stall)]);
    follow_heads(&mut view, 3, stalled.node.clone());
    let new = changes_node(&[7]);
    let rev = view.connection.rev + 1;
    view.reconnect(&crate::runtime::Connection {
        client: Some(new.client),
        network: new.network,
        chain: String::new(),
        rev,
        live: None,
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

/// What `view`'s replies hold once `count` have come, waiting up to two
/// seconds for them.
fn drained(view: &View, count: usize) -> Vec<wire::Event> {
    let mut items = Vec::new();
    for _ in 0..200 {
        view.replies.drain_into(&mut items);
        if items.len() >= count {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    items
}

/// The connection to `node`, one rev on from the view's.
fn connection_to(view: &View, node: Node) -> crate::runtime::Connection {
    crate::runtime::Connection {
        client: Some(node.client),
        network: node.network,
        chain: String::new(),
        rev: view.connection.rev + 1,
        live: None,
    }
}

/// A request is ended once. A `chain.heads` the old node ended (it answers
/// no status) and a reply already given, each still listed when the
/// connect comes, stay ended: the heads is not opened again on the new
/// node (which would end it a second time) and the reply is not refused
/// `stale_connection` after its answer. Before, a reconnect under load
/// reopened a heads the old node had just ended, and the view heard two
/// ends for one id. One request per view: a later spawn prunes a finished
/// task from the list.
#[test]
fn a_reconnect_leaves_an_ended_request_ended() {
    let ended_then_reconnected = |begin: &dyn Fn(&mut View)| {
        let mut view = view();
        begin(&mut view);
        let ended = drained(&view, 1);
        assert!(
            matches!(&ended[..], [wire::Event::Response { done: true, .. }]),
            "{ended:?}"
        );
        assert_eq!(view.tasks.len(), 1, "still listed");
        let new = connection_to(&view, changes_node(&[]));
        view.reconnect(&new);
        assert_eq!(view.pending, [], "nothing answered for an ended request");
        assert!(view.tasks.is_empty(), "nothing opened again");
        let mut after = Vec::new();
        view.replies.drain_into(&mut after);
        assert_eq!(after, [], "and nothing came after");
    };
    ended_then_reconnected(&|view| follow_heads(view, 3, changes_node(&[])));
    ended_then_reconnected(&|view| spawn_reply(view, 2, std::future::ready(Ok(vec![1]))));
}

/// A submit whose frame went out to a node that has not answered is in
/// doubt when a connect takes it: the old node may hold the frame. Before,
/// it was refused `stale_connection`, which tells the view nothing reached
/// the node, and a view that sends again on that may apply the op twice.
#[tokio::test]
async fn a_submit_out_at_a_reconnect_answers_in_doubt() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(41),
    ))
    .await;
    let old = fake_node(vec![
        (
            noded::route::STATUS,
            Mode::Answer(abi::encode(&status_at(TIP))),
        ),
        (noded::route::SUBMIT, Mode::Stall),
    ]);
    let mut view = view();
    let (replies, node) = (view.replies.clone(), old.node.clone());
    let claim = Claim::default();
    let held = claim.clone();
    start(&mut view, 5, None, claim, async move {
        let answer = submit(node, call_registry(), held.clone()).await;
        replies.answer(&held, 5, answer, true);
    });
    // the status read, then the submit: the frame is out
    for _ in 0..200 {
        if old.accepted.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let new = connection_to(&view, changes_node(&[]));
    view.reconnect(&new);
    match view.pending.as_slice() {
        [
            wire::Event::Response {
                id: 5,
                result: Ok(answer),
                done: true,
            },
        ] => assert!(
            matches!(
                methods::decode(answer).unwrap(),
                methods::Submitted::InDoubt { until, .. } if until == TIP.height + 256
            ),
            "in doubt, with how long a block may carry it"
        ),
        other => panic!("{other:?}"),
    }
}

/// One request off an upload's socket: its request line, and its body
/// whether sent by length or chunked (an upload streams, so it is chunked).
async fn read_http(stream: &mut tokio::io::BufReader<tokio::net::TcpStream>) -> (String, Vec<u8>) {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _};
    let mut request = String::new();
    stream.read_line(&mut request).await.unwrap();
    let (mut length, mut chunked) = (0, false);
    loop {
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        if line == "\r\n" {
            break;
        }
        let line = line.to_ascii_lowercase();
        if let Some(value) = line.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
        chunked |= line.starts_with("transfer-encoding:") && line.contains("chunked");
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    while chunked {
        let mut size = String::new();
        stream.read_line(&mut size).await.unwrap();
        let size = usize::from_str_radix(size.trim(), 16).unwrap();
        let mut chunk = vec![0; size + 2];
        stream.read_exact(&mut chunk).await.unwrap();
        body.extend_from_slice(&chunk[..size]);
        chunked = size > 0;
    }
    (request, body)
}

/// The program [`put_node`] binds to the blobs role: not the role's name,
/// so a put that names the role instead of its program goes nowhere.
const KEEPER: &str = "keeper";

/// A node an `op.put` meets: one blob upload taken (its body kept). With
/// `carried`, block 8 carries the upload's frame with that outcome (bundle
/// 5, receipted there) and the bundle is held at 9 once `hold` lets it;
/// without, block 8 + the anchor window (the frame's `until`) comes and no
/// block carries it.
struct PutNode {
    node: Node,
    upload: Upload,
    hold: std::sync::Arc<tokio::sync::Notify>,
}

type Upload = std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>>;

/// What every connection to a [`put_node`] shares.
#[derive(Clone)]
struct Put {
    upload: Upload,
    hold: std::sync::Arc<tokio::sync::Notify>,
    landed: std::sync::Arc<tokio::sync::watch::Sender<bool>>,
    landing: tokio::sync::watch::Receiver<bool>,
    carried: Option<abi::Outcome>,
}

/// [`put_node`]'s block 8 carrying the frame as applied, putting bundle 5.
fn bundle_five() -> Option<abi::Outcome> {
    Some(abi::Outcome::Applied {
        output: abi::encode(&5u64),
    })
}

fn put_node(carried: Option<abi::Outcome>) -> PutNode {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let node = Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let fake = PutNode {
        node,
        upload: Default::default(),
        hold: Default::default(),
    };
    let (landed, landing) = tokio::sync::watch::channel(false);
    let put = Put {
        upload: fake.upload.clone(),
        hold: fake.hold.clone(),
        landed: std::sync::Arc::new(landed),
        landing,
        carried,
    };
    handle().spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_put(stream, put.clone()));
        }
    });
    fake
}

/// The frame an upload carried: its length, then the frame.
fn uploaded_frame(upload: &[u8]) -> noded::Frame {
    let len = u32::from_le_bytes(upload[..4].try_into().unwrap()) as usize;
    abi::decode(&upload[4..4 + len]).unwrap()
}

/// One connection to a [`put_node`]: a changes socket, or one request.
async fn serve_put(stream: tokio::net::TcpStream, mut put: Put) {
    use futures::{SinkExt as _, StreamExt as _};
    use tokio::io::AsyncWriteExt as _;
    let mut head = [0; 64];
    let read = stream.peek(&mut head).await.unwrap();
    let line = String::from_utf8_lossy(&head[..read]).into_owned();
    if let Some(program) = line.strip_prefix("GET /v1/changes/") {
        let blocks = program.starts_with(noded::RECENT) || program.starts_with("%24recent");
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        put.landing.wait_for(|landed| *landed).await.unwrap();
        let change = |height, key: Vec<u8>, value| {
            let writes = vec![(key, Some(value))];
            let change = noded::Change {
                height,
                root: abi::Root([0; 32]),
                writes,
            };
            tokio_tungstenite::tungstenite::Message::Binary(abi::encode(&change))
        };
        let bundle = |custody| {
            abi::encode(&blob::contract::Bundle {
                putter: 1,
                height: 8,
                blobs: Vec::new(),
                custody,
            })
        };
        let key = blob::contract::bundle_key(5);
        let height = match put.carried {
            Some(_) => 8,
            None => TIP.height + noded::ANCHOR_WINDOW,
        };
        if blocks {
            let block = change(height, b"tip".to_vec(), Vec::new());
            socket.send(block).await.unwrap();
        } else if put.carried.is_some() {
            let receipted = blob::contract::Custody::Receipted(Vec::new());
            socket
                .send(change(8, key.clone(), bundle(receipted)))
                .await
                .unwrap();
            put.hold.notified().await;
            let held = blob::contract::Custody::Held { since: 9 };
            socket.send(change(9, key, bundle(held))).await.unwrap();
        }
        while socket.next().await.is_some() {}
        return;
    }
    let mut stream = tokio::io::BufReader::new(stream);
    let (line, body) = read_http(&mut stream).await;
    let answer = match line.split(' ').nth(1).unwrap() {
        noded::route::STATUS => {
            let mut status = status_at(TIP);
            status.roles.bind(abi::role::BLOBS, KEEPER);
            abi::encode(&status)
        }
        noded::route::BLOB_PUT => {
            *put.upload.lock().unwrap() = Some(body);
            put.landed.send_replace(true);
            abi::encode(&noded::Submitted::Pooled)
        }
        // block 8 carried the uploaded frame, and only that one
        noded::route::TX => {
            let ask: noded::TxAsk = abi::decode(&body).unwrap();
            let upload = put.upload.lock().unwrap().clone().unwrap();
            let id = uploaded_frame(&upload).body.id();
            let carried = put.carried.filter(|_| ask.tx == noded::TxRef::Hash(id));
            let located = carried.map(|outcome| noded::Located {
                height: 8,
                position: 0,
                time: 0,
                tx: noded::Tx {
                    hash: id,
                    signer: Vec::new(),
                    anchor: TIP,
                    target: KEEPER.into(),
                    size: 0,
                    payload: None,
                    receipt: Some(noded::Receipt {
                        program: KEEPER.into(),
                        outcome,
                        events: Vec::new(),
                        nested: Vec::new(),
                    }),
                },
            });
            abi::encode(&located)
        }
        other => panic!("{other}"),
    };
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        answer.len()
    );
    stream.write_all(reply.as_bytes()).await.unwrap();
    stream.write_all(&answer).await.unwrap();
}

fn bytes(kind: &str, bytes: &[u8]) -> methods::NewBlob {
    methods::NewBlob {
        kind: kind.into(),
        body: methods::BlobBody::Bytes(bytes.to_vec()),
    }
}

fn picked(file: u64, offset: u64, len: u64) -> methods::NewBlob {
    methods::NewBlob {
        kind: "chunk".into(),
        body: methods::BlobBody::Picked { file, offset, len },
    }
}

/// `op.put` of `blobs` as its ask checks it.
fn asked(view: &View, blobs: Vec<methods::NewBlob>) -> Result<Vec<Blob>, wire::Error> {
    blobs_of(view, &methods::encode(&methods::PutBlobs { blobs }))
}

/// The blobs of a put the ask takes.
fn put_of(view: &View, blobs: Vec<methods::NewBlob>) -> Vec<Blob> {
    match asked(view, blobs) {
        Ok(blobs) => blobs,
        Err(refusal) => panic!("{refusal:?}"),
    }
}

/// `op.put` puts two slices of a picked file and bytes of the view's own
/// as one bundle: a `Put` to the program bound to the blobs role naming
/// each framed blob's id, length and kind, and the framed blobs after it
/// on the upload, in order. It answers each one's sha256 only once the
/// network holds the bundle: a receipted bundle is not yet held.
#[tokio::test]
async fn a_put_answers_each_framed_blobs_sha256_once_the_bundle_is_held() {
    use commonware_cryptography::Signer as _;
    const MOST: u64 = methods::BLOB_MOST;
    let _seat = backend::seat_serial();
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(43),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tape.bin");
    let file: Vec<u8> = (0..MOST + 5).map(|at| at as u8).collect();
    std::fs::write(&path, &file).unwrap();
    let view = view();
    let handle = view.picker.pick(vec![path])[0].file;
    let blobs = vec![
        picked(handle, 0, MOST),
        picked(handle, MOST, 5),
        bytes("note", b"hi"),
    ];
    let blobs = put_of(&view, blobs);
    let fake = put_node(bundle_five());
    let mut answer = std::pin::pin!(put(fake.node.clone(), blobs));
    let waited = tokio::time::timeout(std::time::Duration::from_millis(500), &mut answer).await;
    assert!(waited.is_err(), "answered before the bundle was held");
    assert!(fake.upload.lock().unwrap().is_some(), "the upload went in");
    fake.hold.notify_one();
    let answer = tokio::time::timeout(std::time::Duration::from_secs(5), answer)
        .await
        .expect("answered once held");
    let framed = [
        blob::frame("chunk", &file[..MOST as usize]).unwrap(),
        blob::frame("chunk", &file[MOST as usize..]).unwrap(),
        blob::frame("note", b"hi").unwrap(),
    ];
    let entries: Vec<_> = framed
        .iter()
        .map(|framed| blob::contract::Entry::of(framed).unwrap())
        .collect();
    let ids: Vec<[u8; 32]> = entries
        .iter()
        .map(|entry| match entry.id {
            abi::BlobId::Sha256(digest) => digest,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        methods::decode::<Vec<[u8; 32]>>(&answer.unwrap()).unwrap(),
        ids
    );
    let upload = fake.upload.lock().unwrap().take().unwrap();
    let len = u32::from_le_bytes(upload[..4].try_into().unwrap()) as usize;
    let frame = uploaded_frame(&upload);
    assert_eq!(frame.body.target, KEEPER, "to the program the role binds");
    assert_eq!(frame.body.anchor, TIP);
    assert_eq!(
        abi::decode::<blob::contract::Op>(&frame.body.payload).unwrap(),
        blob::contract::Op::Put { blobs: entries },
        "the op names ids, lengths and kinds, never bytes"
    );
    assert_eq!(
        upload[4 + len..],
        framed.concat(),
        "each blob framed, in order"
    );
}

/// `op.put` against a [`put_node`] whose blocks carry the frame as
/// `carried` says, answered within five seconds.
async fn put_meeting(carried: Option<abi::Outcome>, seed: u64) -> Answer {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(seed),
    ))
    .await;
    let blobs = put_of(&view(), vec![bytes("chunk", b"tape")]);
    let fake = put_node(carried);
    let answer = put(fake.node.clone(), blobs);
    tokio::time::timeout(std::time::Duration::from_secs(5), answer)
        .await
        .expect("answered")
}

/// The block that carries the put rejects it: the program's refusal is
/// the answer, as it said it.
#[tokio::test]
async fn a_put_the_program_rejects_answers_its_refusal() {
    let rejected = abi::Refusal::new(abi::reason::UNAUTHORIZED, "no account puts here");
    let refusal = put_meeting(Some(abi::Outcome::Rejected(rejected)), 45)
        .await
        .unwrap_err();
    assert_eq!(
        (refusal.code.as_str(), refusal.message.as_str()),
        (abi::reason::UNAUTHORIZED, "no account puts here")
    );
}

/// No block carries the put by its frame's `until`: refused `anchor`,
/// saying so, not waited on forever.
#[tokio::test]
async fn a_put_no_block_carries_by_its_until_is_refused_anchor() {
    let refusal = put_meeting(None, 46).await.unwrap_err();
    assert_eq!(refusal.code, abi::reason::ANCHOR);
    let until = TIP.height + noded::ANCHOR_WINDOW;
    assert_eq!(
        refusal.message,
        format!("no block carried the put by block {until}, the last its frame may land at")
    );
}

/// What `op.put` refuses at the ask, before anything is read or signed.
#[test]
fn a_put_of_no_blob_one_too_big_or_a_slice_of_no_pick_is_malformed() {
    const MOST: u64 = methods::BLOB_MOST;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, b"abc").unwrap();
    let view = view();
    assert_eq!(view.picker.pick(vec![path])[0].file, 1);
    let cases = [
        (Vec::new(), "names no blob"),
        (vec![bytes("chunk", &vec![0; MOST as usize + 1])], "over"),
        (vec![picked(1, 0, MOST + 1)], "past"),
        (vec![picked(1, 2, 2)], "past"),
        (vec![picked(1, u64::MAX, 2)], "past"),
        (vec![picked(2, 0, 1)], "not of the latest pick"),
        (vec![bytes("two words", b"x")], "one word"),
        (vec![bytes("", b"x")], "one word"),
    ];
    for (blobs, why) in cases {
        let Err(refusal) = asked(&view, blobs) else {
            panic!("taken: {why}")
        };
        assert_eq!(refusal.code, refusal::MALFORMED_REQUEST);
        assert!(refusal.message.contains(why), "{}", refusal.message);
    }
    // the most a blob holds, and a whole file
    put_of(
        &view,
        vec![bytes("chunk", &vec![0; MOST as usize]), picked(1, 0, 3)],
    );
}

/// A picked file whose size changed since the pick is refused
/// `host_fault` by the hash pass, before the node is asked anything.
#[tokio::test]
async fn a_put_of_a_file_whose_size_changed_since_the_pick_is_a_host_fault() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.txt");
    std::fs::write(&path, b"one").unwrap();
    let view = view();
    view.picker.pick(vec![path.clone()]);
    let blobs = put_of(&view, vec![picked(1, 0, 3)]);
    std::fs::write(&path, b"one two").unwrap();
    let nowhere = Node {
        client: RpcClient::new("http://127.0.0.1:1"),
        network: "test-network".into(),
    };
    let refusal = put(nowhere, blobs).await.unwrap_err();
    assert_eq!(refusal.code, refusal::HOST_FAULT);
    assert_eq!(refusal.message, "log.txt changed since it was picked");
}

/// A put waiting for its bundle to be held is a node task: cancelled
/// (as a view going away cancels its tasks), it gives its slot back and
/// answers nothing.
#[test]
fn a_put_waiting_for_its_bundle_goes_with_the_view() {
    use commonware_cryptography::Signer as _;
    let _seat = backend::seat_serial();
    futures::executor::block_on(backend::seat_key(std::sync::Arc::new(
        commonware_cryptography::ed25519::PrivateKey::from_seed(44),
    )));
    let fake = put_node(bundle_five());
    let mut view = view();
    let blobs = put_of(&view, vec![bytes("chunk", b"tape")]);
    let replies = view.replies.clone();
    let mut changed = view.replies.changes();
    let node = fake.node.clone();
    start(&mut view, 1, None, Claim::default(), async move {
        let answer = put(node, blobs).await;
        replies.item(1, answer, true);
    });
    for _ in 0..200 {
        if fake.upload.lock().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(fake.upload.lock().unwrap().is_some(), "the upload went in");
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

/// The throwaway try network (unit `garage5-try`, tunnelled to
/// `127.0.0.1:18947`; `DUCKTAPE_TRY_NODE` names another): a fresh key makes
/// its account, puts a picked file's two slices (1 MiB and 5 bytes) as
/// `chunk` blobs, and the blobs program says the network holds each id
/// answered. Never run against garage.
#[tokio::test]
#[ignore = "needs the try network's node"]
async fn the_try_network_holds_a_put() {
    use commonware_cryptography::Signer as _;
    const MOST: u64 = methods::BLOB_MOST;
    let _seat = backend::seat_serial();
    let key = commonware_cryptography::ed25519::PrivateKey::from_seed(noded::salt());
    backend::seat_key(std::sync::Arc::new(key)).await;
    let endpoint =
        std::env::var("DUCKTAPE_TRY_NODE").unwrap_or_else(|_| "http://127.0.0.1:18947".into());
    let client = RpcClient::new(endpoint);
    let status = client.status().await.unwrap();
    let name = format!("op-put {}", noded::salt() % 1_000_000);
    let account = backend::identity::create_plain_account(&client, &status.network, &name)
        .await
        .unwrap();
    eprintln!(
        "account {account:?} on {} at block {}",
        status.network, status.height
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("try.bin");
    let mut file: Vec<u8> = (0..MOST + 5).map(|at| (at * 7) as u8).collect();
    file[..8].copy_from_slice(&noded::salt().to_le_bytes());
    std::fs::write(&path, &file).unwrap();
    let view = view();
    let handle = view.picker.pick(vec![path])[0].file;
    let slices = vec![picked(handle, 0, MOST), picked(handle, MOST, 5)];
    let blobs = put_of(&view, slices);
    let node = Node {
        client: client.clone(),
        network: status.network.clone(),
    };
    let asked = std::time::Instant::now();
    let ids: Vec<[u8; 32]> = methods::decode(&put(node, blobs).await.unwrap()).unwrap();
    eprintln!("held after {:?}", asked.elapsed());
    let target = status.roles.bound(abi::role::BLOBS).unwrap();
    for id in ids {
        let blob = abi::BlobId::Sha256(id);
        let ask = abi::encode(&blob::contract::Query::Held { blob });
        let frame = backend::query_frame(&status.network, target, ask)
            .await
            .unwrap();
        let reply = client
            .query(backend::Layer::Confirmed, frame)
            .await
            .unwrap();
        let reply: blob::contract::Reply = abi::decode(&reply).unwrap();
        eprintln!("sha256:{} {reply:?}", noded_client::hex::hex_encode(&id));
        assert!(
            matches!(reply, blob::contract::Reply::Held(Some(_))),
            "{reply:?}"
        );
    }
}
