use super::*;

/// The table on [`Mounted::reload_due`], row by row.
#[test]
fn a_roster_read_reloads_by_code_node_and_hold_off() {
    let now = Instant::now();
    let code = [1; 32];
    let held = |code, next| Retry {
        code,
        next,
        gap: RETRY_FIRST,
    };
    let running = now + RETRY_FIRST;
    let up = now - RETRY_FIRST;
    // (retry, same_code, asked_of_this_node) -> load
    let table = [
        (None, false, false, true),
        (None, true, false, true),
        (None, false, true, true),
        (None, true, true, false),
        // the gap is up: the re-attempt at the same code it promised
        (Some(held(Some(code), up)), true, true, true),
        (Some(held(Some(code), up)), false, true, true),
        // running: this very code waits, whatever else moved
        (Some(held(Some(code), running)), false, false, false),
        (Some(held(Some(code), running)), true, true, false),
        // running for another code, or for none: nothing held off here
        (Some(held(Some([2; 32]), running)), true, true, true),
        (Some(held(None, running)), true, true, true),
    ];
    for (nth, (retry, same_code, asked_of_this_node, load)) in table.into_iter().enumerate() {
        let seat = Mounted::seat();
        seat.lock().unwrap().retry = retry;
        assert_eq!(
            seat.lock()
                .unwrap()
                .reload_due(same_code, asked_of_this_node, code, now),
            load,
            "row {nth}"
        );
    }
}

/// A view that fails to load is held off under the blob the roster names,
/// not under its own section's hash: the two never match, and keying on
/// the section had every block reload the failed view while the hold-off
/// ran and skip it once the gap was up.
#[test]
fn a_failed_view_is_held_off_under_the_code_the_roster_names() {
    let code = [1; 32];
    let section = [9; 32];
    let seat = Mounted::seat();
    let mut locked = seat.lock().unwrap();
    locked.load_failed(
        "seat-tests-failed",
        None,
        Some(code),
        Unloaded {
            hash: Some(section),
            failure: Failure::Refused("not a view".into()),
        },
    );
    assert!(matches!(locked.slot, Slot::Failed(_)));
    let now = Instant::now();
    assert!(
        !locked.reload_due(true, true, code, now),
        "held off: the same code is not asked again this block"
    );
    assert!(
        locked.reload_due(true, true, code, now + RETRY_FIRST * 2),
        "the gap is up: asked again"
    );
    assert!(
        locked.reload_due(true, true, [2; 32], now),
        "another code is not held off"
    );

    // failing again on the same code widens the gap; a failure before any
    // candidate holds nothing off
    let held_off = locked.retry.take();
    locked.load_failed(
        "seat-tests-failed",
        held_off,
        Some(code),
        Unloaded {
            hash: Some(section),
            failure: Failure::Refused("not a view".into()),
        },
    );
    assert_eq!(locked.retry.as_ref().unwrap().gap, RETRY_FIRST * 2);
    let held_off = locked.retry.take();
    locked.load_failed(
        "seat-tests-failed",
        held_off,
        Some(code),
        Unloaded {
            hash: None,
            failure: Failure::Unreachable("no node".into()),
        },
    );
    assert_eq!(locked.retry.as_ref().unwrap().code, None);
    assert!(locked.reload_due(true, true, code, now));
}

/// Waits, up to ten seconds, for `done`; panics naming `what` if it never
/// is.
pub(super) fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A node on a local socket: it answers the registry's two roster
/// questions with `programs` and `views`, and a blob among `blobs` (bodies)
/// with its bytes. Any other blob it holds — the request read, no answer —
/// until it is dropped, counting the blobs it holds at once and the most it
/// ever did.
struct FakeNode {
    asked_of: Connection,
    held: Arc<Mutex<Held>>,
}

#[derive(Default)]
struct Held {
    now: usize,
    most: usize,
    released: bool,
}

/// What a fake node answers.
struct Answers {
    programs: Vec<module_registry::Entry>,
    views: Vec<module_registry::View>,
    blobs: Vec<(abi::BlobId, Vec<u8>)>,
}

impl FakeNode {
    fn new(
        programs: Vec<module_registry::Entry>,
        views: Vec<module_registry::View>,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let answers = Arc::new(Answers {
            programs,
            views,
            blobs: blobs
                .into_iter()
                .map(|body| (blob_id(&body), framed(&body)))
                .collect(),
        });
        let held = Arc::new(Mutex::new(Held::default()));
        let holding = held.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (answers, held) = (answers.clone(), holding.clone());
                std::thread::spawn(move || answer(stream, &answers, &held));
            }
        });
        FakeNode {
            asked_of: Connection {
                client: Some(crate::backend::RpcClient::new(format!("http://{address}"))),
                network: "fake-network".into(),
                chain: "fake-network#0".into(),
                rev: connection().lock().unwrap().rev,
            },
            held,
        }
    }

    /// The blobs held now, and the most ever held at once.
    fn held(&self) -> (usize, usize) {
        let held = self.held.lock().unwrap();
        (held.now, held.most)
    }
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        self.held.lock().unwrap().released = true;
    }
}

fn answer(mut stream: std::net::TcpStream, answers: &Answers, held: &Mutex<Held>) {
    use super::super::kernel::tests::{read_request, respond};
    let (line, body) = read_request(&mut stream);
    if line.starts_with("POST /v1/query ") {
        let query: crate::backend::noded::Query = abi::decode(&body).unwrap();
        let frame: crate::backend::noded::Frame = abi::decode(&query.frame).unwrap();
        let reply = match abi::decode(&frame.body.payload).unwrap() {
            module_registry::Query::Views(_) => {
                module_registry::Reply::Views(answers.views.clone())
            }
            _ => module_registry::Reply::Programs(answers.programs.clone()),
        };
        return respond(&mut stream, "200 OK", &abi::encode(&abi::encode(&reply)));
    }
    let asked: abi::BlobId = abi::decode(&body).unwrap();
    if let Some((_, framed)) = answers.blobs.iter().find(|(id, _)| *id == asked) {
        return respond(&mut stream, "200 OK", &abi::encode(&Some(framed.clone())));
    }
    {
        let mut held = held.lock().unwrap();
        held.now += 1;
        held.most = held.most.max(held.now);
    }
    while !held.lock().unwrap().released {
        std::thread::sleep(Duration::from_millis(10));
    }
    held.lock().unwrap().now -= 1;
    respond(&mut stream, "404 Not Found", b"gone");
}

/// A blob body as the node stores it: `blob <len>\0<body>`.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut framed = format!("blob {}\0", body.len()).into_bytes();
    framed.extend_from_slice(body);
    framed
}

fn blob_id(body: &[u8]) -> abi::BlobId {
    use sha2::Digest as _;
    abi::BlobId::Sha256(sha2::Sha256::digest(framed(body)).into())
}

/// A program the node lists, its code a blob of `seed`'s.
fn listed(name: &str, seed: &str) -> module_registry::Entry {
    module_registry::Entry {
        program: name.into(),
        code: blob_id(seed.as_bytes()),
        params: Vec::new(),
    }
}

/// A node that lists 1,000 programs, none of whose code it answers: the
/// app takes the first `MAX_PROGRAMS` of them, and runs at most `LOADERS`
/// of their loads at once — the rest wait their turn rather than each
/// forking a thread.
#[test]
fn a_long_roster_is_capped_and_its_loads_queue() {
    let programs = (0..1000)
        .map(|n| listed(&format!("long-roster-{n:04}"), &format!("long-roster-{n}")))
        .collect();
    let node = FakeNode::new(programs, Vec::new(), Vec::new());
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let kept = crate::backend::views::MAX_PROGRAMS;
    assert_eq!(roster.lock().len(), kept, "the roster is capped");
    assert_eq!(roster.rail().len(), kept, "the rail lists what was kept");
    assert_eq!(lock(&registry).len(), kept, "only what was kept is seated");
    eventually("a load reached the node", || node.held().0 > 0);
    // time for a load per thread to have asked, had each got one
    std::thread::sleep(Duration::from_millis(500));
    let (_, most) = node.held();
    assert!(
        most <= LOADERS,
        "{most} loads ran at once, past the {LOADERS} loaders"
    );
}

/// A load a node never answers does not hold the roster read that queued
/// it: the read returns, and with it the per-block latch
/// (`deployments_checked`'s `IN_FLIGHT`), so the next block is checked.
#[test]
fn a_stalled_load_does_not_hold_the_roster_read() {
    let node = FakeNode::new(
        vec![listed("stalled-load", "stalled")],
        Vec::new(),
        Vec::new(),
    );
    let asked_of = node.asked_of.clone();
    let (read, returned) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        read_roster(asked_of, &Roster::default(), &Registry::default());
        let _ = read.send(());
    });
    returned
        .recv_timeout(Duration::from_secs(10))
        .expect("the roster read waited on the load it queued");
    eventually("the load is held by the node", || node.held().0 == 1);
}
