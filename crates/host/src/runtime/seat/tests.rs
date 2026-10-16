use super::super::roster::read_roster;
use super::*;

/// The table on [`Mounted::reload_due`], row by row.
#[test]
fn a_roster_read_reloads_by_code_node_and_hold_off() {
    let code = [1; 32];
    let held = |code| Some(Retry { code });
    // (retry, same_code, asked_of_this_node) -> load
    let table = [
        (None, false, false, true),
        (None, true, false, true),
        (None, false, true, true),
        (None, true, true, false),
        // this very code waits, whatever else moved
        (held(Some(code)), false, false, false),
        (held(Some(code)), true, true, false),
        // held for another code, or for none: nothing held off here
        (held(Some([2; 32])), true, true, true),
        (held(None), true, true, true),
    ];
    for (nth, (retry, same_code, asked_of_this_node, load)) in table.into_iter().enumerate() {
        let seat = Mounted::seat();
        seat.lock().unwrap().retry = retry;
        assert_eq!(
            seat.lock()
                .unwrap()
                .reload_due(same_code, asked_of_this_node, code),
            load,
            "row {nth}"
        );
    }
}

/// A view that fails to load is held off under the blob the roster names,
/// not under its own section's hash: the two never match, and keying on
/// the section had every block reload the failed view.
#[test]
fn a_failed_view_is_held_off_under_the_code_the_roster_names() {
    let code = [1; 32];
    let section = [9; 32];
    let seat = Mounted::seat();
    let mut locked = seat.lock().unwrap();
    locked.load_failed(
        "seat-tests-failed",
        Some(code),
        Unloaded {
            hash: Some(section),
            failure: Failure::Refused("not a view".into()),
        },
    );
    assert!(matches!(locked.slot, Slot::Failed(_)));
    assert!(
        !locked.reload_due(true, true, code),
        "held off: the same code is not asked again"
    );
    assert!(
        locked.reload_due(true, true, [2; 32]),
        "another code is not held off"
    );

    // a failure before any candidate holds nothing off
    locked.load_failed(
        "seat-tests-failed",
        Some(code),
        Unloaded {
            hash: None,
            failure: Failure::Unreachable("no node".into()),
        },
    );
    assert_eq!(locked.retry.as_ref().unwrap().code, None);
    assert!(locked.reload_due(true, true, code));
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

/// A node on a local socket: it answers the programs role's two roster
/// questions with `programs` and `views`, the roles question with `roles`
/// (counted), and
/// a blob among `blobs` (bodies) with its bytes. Any other blob it holds —
/// the request read, no answer — until it is dropped, counting the blobs it
/// holds at once and the most it ever did.
struct FakeNode {
    asked_of: Connection,
    held: Arc<Mutex<Held>>,
}

#[derive(Default)]
struct Held {
    now: usize,
    most: usize,
    released: bool,
    roles_asked: usize,
    scheduled_asked: usize,
}

/// What a fake node answers.
struct Answers {
    programs: Vec<::programs::Entry>,
    views: Vec<::programs::View>,
    roles: RolesAnswer,
    blobs: Vec<(abi::BlobId, Vec<u8>)>,
}

enum RolesAnswer {
    Bound(abi::Roles),
    Refused,
    Unreadable,
}

impl FakeNode {
    fn new(
        programs: Vec<::programs::Entry>,
        views: Vec<::programs::View>,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        Self::with_roles(programs, views, RolesAnswer::Bound(roles()), blobs)
    }

    fn with_roles(
        programs: Vec<::programs::Entry>,
        views: Vec<::programs::View>,
        roles: RolesAnswer,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let answers = Arc::new(Answers {
            programs,
            views,
            roles,
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
                live: None,
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
    if line.starts_with("GET /v1/status ") {
        return respond(&mut stream, "200 OK", &abi::encode(&status(0)));
    }
    if line.starts_with("POST /v1/query ") {
        let query: noded_client::wire::Query = abi::decode(&body).unwrap();
        let frame: noded_client::wire::Frame = abi::decode(&query.frame).unwrap();
        let reply = match abi::decode(&frame.body.payload).unwrap() {
            ::programs::Query::Roster => ::programs::Reply::Roster {
                height: 1,
                programs: answers.programs.clone(),
                views: answers.views.clone(),
            },
            ::programs::Query::Roles => {
                held.lock().unwrap().roles_asked += 1;
                match &answers.roles {
                    RolesAnswer::Bound(roles) => ::programs::Reply::Roles(roles.clone()),
                    RolesAnswer::Refused => {
                        let refused = abi::Refusal::new("invalid_input", "Query did not decode");
                        return respond(&mut stream, "400 Bad Request", &abi::encode(&refused));
                    }
                    RolesAnswer::Unreadable => {
                        let newer = [abi::encode(&::programs::Reply::Roles(roles())), vec![1]];
                        return respond(&mut stream, "200 OK", &abi::encode(&newer.concat()));
                    }
                }
            }
            // answered, and counted: a roster read never asks it
            ::programs::Query::Scheduled { .. } => {
                held.lock().unwrap().scheduled_asked += 1;
                ::programs::Reply::Scheduled(::programs::PageResponse {
                    height: 1,
                    items: Vec::new(),
                    next: None,
                })
            }
            other => panic!("unexpected programs query: {other:?}"),
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

fn status(height: u64) -> noded_client::wire::Status {
    use noded_client::wire as noded;
    noded::Status {
        network: "fake-network".into(),
        time: 1,
        block_time_ms: 1_000,
        epoch_length: 100,
        height,
        tip: [0; 32],
        root: abi::Root([0; 32]),
        epoch: 0,
        identity: Vec::new(),
        contract: noded::NODE_CONTRACT,
        genesis: [0; 32],
        roles: crate::backend::test_roles(),
    }
}

struct ProgramsNode {
    asked_of: Connection,
    tip: Arc<std::sync::atomic::AtomicU64>,
    running: abi::BlobId,
    scheduled: abi::BlobId,
}

impl ProgramsNode {
    fn swapping(program: &'static str, at: u64) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let tip = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let height = tip.clone();
        let (founded, codes) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (programs, running, scheduled) = programs_swapping(program, at);
            founded.send((running, scheduled)).unwrap();
            for mut stream in listener.incoming().flatten() {
                let tip = height.load(std::sync::atomic::Ordering::SeqCst);
                serve_programs(&mut stream, &programs, tip);
            }
        });
        let (running, scheduled) = codes.recv().unwrap();
        ProgramsNode {
            asked_of: Connection {
                client: Some(crate::backend::RpcClient::new(format!("http://{address}"))),
                network: "fake-network".into(),
                chain: "fake-network#0".into(),
                rev: connection().lock().unwrap().rev,
                live: None,
            },
            tip,
            running,
            scheduled,
        }
    }

    fn code_at(
        &self,
        tip: u64,
        program: &str,
        roster: &Roster,
        registry: &Registry,
    ) -> Option<abi::BlobId> {
        self.tip.store(tip, std::sync::atomic::Ordering::SeqCst);
        read_roster(self.asked_of.clone(), roster, registry);
        let listed = roster.lock();
        listed
            .iter()
            .find(|entry| entry.name == program)
            .map(|entry| entry.code)
    }
}

fn programs_env(height: u64, origin: ::program::Origin) -> ::program::Env {
    ::program::Env {
        height,
        origin,
        sender: None,
        ..::program::MockHost::env(::programs::PROGRAM)
    }
}

fn programs_swapping(program: &str, at: u64) -> (::program::MockHost, abi::BlobId, abi::BlobId) {
    use ::program::{HashKind, MockHost, Origin, Program as _, Sibling};
    use ::programs::{CODE_KIND, Change, Entry, Genesis, Op, Programs, Scheduled};
    let store = MockHost::default();
    let entry = |code| Entry {
        program: program.into(),
        code,
        params: Vec::new(),
    };
    let running = blob_id(b"running code");
    let genesis = Genesis {
        programs: vec![entry(running)],
        views: Vec::new(),
        hooks: Vec::new(),
    };
    Programs::init(
        &store.exec(programs_env(0, Origin::Root)),
        &abi::encode(&genesis),
    )
    .unwrap();
    let body = b"scheduled code";
    let scheduled = ::program::blob_id(HashKind::Sha256, CODE_KIND, body).unwrap();
    let held: Sibling = Box::new(move |_, _| {
        Ok(abi::encode(&::blobs::Reply::Held(Some(::blobs::Holding {
            bundle: 0,
            len: body.len() as u64,
            kind: CODE_KIND.into(),
            since: 1,
        }))))
    });
    store
        .borrow_mut()
        .siblings
        .insert(MockHost::role(abi::role::BLOBS), held);
    let swap = Op::Schedule(Scheduled {
        height: at,
        change: Change::Set(entry(scheduled)),
    });
    let validators = Origin::Program(MockHost::role(abi::role::VALIDATORS));
    Programs::execute(&store.exec(programs_env(1, validators)), swap).unwrap();
    (store, running, scheduled)
}

fn serve_programs(stream: &mut std::net::TcpStream, programs: &::program::MockHost, tip: u64) {
    use super::super::kernel::tests::{read_request, respond};
    use ::program::Program as _;
    let (line, body) = read_request(stream);
    if line.starts_with("GET /v1/status ") {
        return respond(stream, "200 OK", &abi::encode(&status(tip)));
    }
    if !line.starts_with("POST /v1/query ") {
        return respond(stream, "200 OK", &abi::encode(&None::<Vec<u8>>));
    }
    let query: noded_client::wire::Query = abi::decode(&body).unwrap();
    let frame: noded_client::wire::Frame = abi::decode(&query.frame).unwrap();
    let asked = abi::decode(&frame.body.payload).unwrap();
    let env = programs_env(tip + 1, ::program::Origin::Signed(frame.body.signer));
    let reply = ::programs::Programs::query(&programs.query(env), asked).unwrap();
    respond(stream, "200 OK", &abi::encode(&abi::encode(&reply)));
}

#[test]
fn a_scheduled_swap_is_read_once_the_block_being_built_reaches_its_height() {
    let node = ProgramsNode::swapping("scheduled-swap", 5);
    let (roster, registry) = (Roster::default(), Registry::default());
    let code_at = |tip| node.code_at(tip, "scheduled-swap", &roster, &registry);
    assert_eq!(
        code_at(3),
        Some(node.running),
        "the block being built is below its height"
    );
    assert_eq!(
        code_at(4),
        Some(node.scheduled),
        "the block being built is at its height"
    );
    assert_eq!(
        code_at(9),
        Some(node.scheduled),
        "past it, no programs op since"
    );
}

/// A blob body as the node stores a program's code: `program <len>\0<body>`.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut framed = format!("program {}\0", body.len()).into_bytes();
    framed.extend_from_slice(body);
    framed
}

fn blob_id(body: &[u8]) -> abi::BlobId {
    use sha2::Digest as _;
    abi::BlobId::Sha256(sha2::Sha256::digest(framed(body)).into())
}

/// A program the node lists, its code a blob of `seed`'s.
fn listed(name: &str, seed: &str) -> ::programs::Entry {
    ::programs::Entry {
        program: name.into(),
        code: blob_id(seed.as_bytes()),
        params: Vec::new(),
    }
}

fn roles() -> abi::Roles {
    abi::Roles::from_iter([
        (abi::role::PROGRAMS, "roles-programs"),
        (abi::role::VALIDATORS, "roles-valset"),
        (abi::role::IDENTITY, "roles-identity"),
        (abi::role::BLOBS, "roles-blobs"),
        (abi::role::AWAITS, "roles-awaits"),
    ])
}

#[test]
fn the_roles_come_with_the_roster_and_are_asked_once() {
    let roles = roles();
    let node = FakeNode::with_roles(
        vec![listed("roles-programs", "roles")],
        Vec::new(),
        RolesAnswer::Bound(roles.clone()),
        Vec::new(),
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    assert_eq!(roster.roles(), Some(roles.clone()));
    read_roster(node.asked_of.clone(), &roster, &registry);
    let asked = node.held.lock().unwrap().roles_asked;
    assert_eq!(asked, 1, "not every block");
    assert_eq!(
        node.held.lock().unwrap().scheduled_asked,
        0,
        "the scheduled list is a view's ask (blob.manifest), never a roster read's"
    );
    let upgraded = FakeNode::with_roles(
        vec![listed("roles-programs", "upgraded")],
        Vec::new(),
        RolesAnswer::Bound(roles.clone()),
        Vec::new(),
    );
    read_roster(upgraded.asked_of.clone(), &roster, &registry);
    let asked = upgraded.held.lock().unwrap().roles_asked;
    assert_eq!(asked, 1, "asked again once the roster moved");
}

#[test]
fn a_roles_answer_refused_or_unreadable_lists_no_program_and_is_asked_again() {
    for answer in [RolesAnswer::Refused, RolesAnswer::Unreadable] {
        let node = FakeNode::with_roles(
            vec![listed("roles-unknown", "roles")],
            Vec::new(),
            answer,
            Vec::new(),
        );
        let (roster, registry) = (Roster::default(), Registry::default());
        read_roster(node.asked_of.clone(), &roster, &registry);
        assert_eq!(roster.roles(), None);
        assert!(
            roster.lock().is_empty(),
            "no view loads while the role bindings are unknown"
        );
        assert!(lock(&registry).is_empty());
        read_roster(node.asked_of.clone(), &roster, &registry);
        assert_eq!(node.held.lock().unwrap().roles_asked, 2, "asked again");
    }
}

/// Claim: the roster takes a chain id only in its one spelling
/// (`program::is_name`): a newline, a space or a capital in a program's or
/// a view-only entry's id leaves it off the roster, the rail and the seats;
/// a well-spelled one beside them is kept.
#[test]
fn a_misspelled_id_is_left_off_the_roster() {
    let node = FakeNode::new(
        vec![
            listed("\nSystem", "misspelled-newline"),
            listed("a b", "misspelled-space"),
            listed("System", "misspelled-capital"),
            listed("spelled-ok", "spelled-ok"),
        ],
        vec![::programs::View {
            name: "Misspelled View".into(),
            view: blob_id(b"misspelled-view"),
        }],
        Vec::new(),
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let names: Vec<String> = roster.lock().iter().map(|p| p.name.clone()).collect();
    assert_eq!(names, ["spelled-ok"]);
    assert_eq!(roster.rail().len(), 1, "the rail lists what was kept");
    let seats: Vec<&str> = lock(&registry)
        .keys()
        .map(|(program, _)| *program)
        .collect();
    assert_eq!(seats, ["spelled-ok"]);
}

/// Claim: the consent card names the program whose view asks by the seat
/// that view runs in (`view.program`), and a seat is only made from the
/// roster: a program listed as `"\nSystem"`, whose card would end the line
/// after "Program " and read "System asks …" below it, gets no seat, so no
/// view is ever loaded to ask as it.
#[test]
fn a_newline_id_never_gets_a_seat_to_ask_from() {
    let node = FakeNode::new(
        vec![listed("\nSystem", "newline-asker")],
        Vec::new(),
        Vec::new(),
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    assert!(lock(&registry).is_empty(), "seated: its view could ask");
    assert!(roster.lock().is_empty(), "listed");
}

/// A node that lists 1,000 programs, none of whose code it answers: the
/// app takes every one of them, and runs at most one load a core at once —
/// the rest wait their turn rather than each forking a thread.
#[test]
fn a_long_roster_is_taken_whole_and_its_loads_queue() {
    let programs = (0..1000)
        .map(|n| listed(&format!("long-roster-{n:04}"), &format!("long-roster-{n}")))
        .collect();
    let node = FakeNode::new(programs, Vec::new(), Vec::new());
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let listed = 1000;
    assert_eq!(roster.lock().len(), listed, "the roster is taken whole");
    assert_eq!(roster.rail().len(), listed, "the rail lists every program");
    assert_eq!(lock(&registry).len(), listed, "every program is seated");
    eventually("a load reached the node", || node.held().0 > 0);
    // time for a load per thread to have asked, had each got one
    std::thread::sleep(Duration::from_millis(500));
    let (_, most) = node.held();
    let loaders = super::super::loaders();
    assert!(
        most <= loaders,
        "{most} loads ran at once, past the {loaders} loaders"
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

/// A view whose manifest says `name`, `icon` (none when empty) and a 320px
/// minimum, its `init` the instructions given; its other exports trap.
fn view_declaring(name: &str, icon: &str, init: &str) -> Vec<u8> {
    let manifest = format!(
        "ducktape.view.manifest\\n{name}\\n\\n\\n320\\n{}\\n\\n{icon}",
        wire::WIRE_ID
    );
    wat::parse_str(format!(
        r#"(module
        (@custom "{}" "{manifest}")
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        (func (export "init") {init})
        (func (export "tick") (param i32 i32) (result i64) unreachable)
        (func (export "snapshot") (result i64) unreachable)
        (func (export "restore") (param i32 i32) (result i64) unreachable))"#,
        wire::manifest::MANIFEST_SECTION
    ))
    .unwrap()
}

/// A program no pane has opened is compiled for the rail and never
/// started: its `init` does not run. Here `init` traps, so a load that
/// started the view fails: the preload lands compiled, named by its
/// manifest, and only the load of the pane that claims the seat runs
/// `init`, and fails on it.
#[test]
fn an_unopened_program_runs_no_init() {
    let view = view_declaring("Lazy", "", "unreachable");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: "lazy-view".into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = lock(&registry)[&("lazy-view", 0)].clone();
    eventually("the preload landed", || !lock(&seat).slot.loading());
    assert!(
        matches!(
            &lock(&seat).slot,
            Slot::Compiled(view) if view.name == "Lazy" && view.icon.is_empty() && view.min_width == 320
        ),
        "the unopened view did not land compiled and unstarted"
    );
    // asked again (the seat not yet asked of this node, as on a reconnect):
    // compiled again, still unstarted, though the seat holds its code
    let landed = {
        let mut locked = lock(&seat);
        locked.rev = locked.rev.wrapping_add(1);
        locked.wake.subscribe()
    };
    read_roster(node.asked_of.clone(), &roster, &registry);
    eventually("the second preload landed", || {
        landed.has_changed().unwrap_or(false)
    });
    assert!(
        matches!(&lock(&seat).slot, Slot::Compiled(_)),
        "a preload over the compiled seat started the view"
    );
    // a pane claims the seat: its load starts the view
    let generation = {
        let mut locked = lock(&seat);
        locked.instance = 7;
        locked.start()
    };
    queue(Load {
        program: "lazy-view",
        seat: seat.clone(),
        generation,
        asked_of: node.asked_of.clone(),
        code: Some((code, true)),
    });
    eventually(
        "the claiming pane's load ran init",
        || matches!(&lock(&seat).slot, Slot::Failed(failure) if failure.to_string().contains("init trapped")),
    );
    // the fetch kept the blob in the developer's own cache: it goes
    let kept = crate::backend::dirs()
        .cache
        .join("programs")
        .join(abi::hex(code.digest()));
    let _ = std::fs::remove_file(&kept);
}

/// A rail row's icon is the one its view's manifest declares, read with its
/// name as the seat lands compiled and kept once a pane has started the
/// view; a view that declares none has none.
#[test]
fn a_rail_row_carries_the_icon_its_views_manifest_declares() {
    const ICON: &str = "icons/hammer.svg";
    let views = [
        ("iconed-view", view_declaring("Iconed", ICON, "")),
        ("plain-view", view_declaring("Plain", "", "")),
    ];
    let listed = views
        .iter()
        .map(|(name, view)| ::programs::View {
            name: (*name).into(),
            view: blob_id(view),
        })
        .collect();
    let blobs = views.iter().map(|(_, view)| view.clone()).collect();
    let node = FakeNode::new(Vec::new(), listed, blobs);
    let (roster, seats) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &seats);
    // the rail reads the app's seats: these two are among them while it looks
    for (program, _) in &views {
        let seat = lock(&seats)[&(*program, 0)].clone();
        eventually("the preload landed", || !lock(&seat).slot.loading());
        lock(registry()).insert((*program, 0), seat);
    }
    let icon = |program: &str| {
        let rows = roster.rail();
        let row = rows.iter().find(|row| row.program == program);
        row.expect("a listed view").icon.clone()
    };
    assert_eq!(icon("iconed-view"), ICON, "compiled, not started");
    assert_eq!(icon("plain-view"), "", "its manifest declares none");
    // a pane claims the seat: the view its load starts is that manifest's
    let seat = lock(&seats)[&("iconed-view", 0)].clone();
    let generation = {
        let mut locked = lock(&seat);
        locked.instance = 7;
        locked.start()
    };
    queue(Load {
        program: "iconed-view",
        seat: seat.clone(),
        generation,
        asked_of: node.asked_of.clone(),
        code: Some((blob_id(&views[0].1), true)),
    });
    eventually("the claiming pane's load started the view", || {
        matches!(lock(&seat).slot, Slot::Ready(_))
    });
    assert_eq!(icon("iconed-view"), ICON, "started");
    for (program, view) in &views {
        lock(registry()).remove(&(*program, 0));
        // the fetch kept the blob in the developer's own cache: it goes
        let kept = crate::backend::dirs()
            .cache
            .join("programs")
            .join(abi::hex(blob_id(view).digest()));
        let _ = std::fs::remove_file(&kept);
    }
}

/// A program's code read once is read off the disk after: the cache keeps
/// the blob as the node framed it (`program`, not `blob`), so it hashes to
/// its id. Re-framed as a `blob`, no program ever did, and every view load
/// fetched its whole program from the node again.
#[test]
fn a_program_read_once_is_read_off_the_disk() {
    let body = b"\0asm\x01\0\0\0 a program read once".to_vec();
    let code = blob_id(&body);
    let kept = crate::backend::dirs()
        .cache
        .join("programs")
        .join(abi::hex(code.digest()));
    let _ = std::fs::remove_file(&kept);
    let node = FakeNode::new(Vec::new(), Vec::new(), vec![body.clone()]);
    let client = node.asked_of.client.clone().unwrap();
    let read = || handle().block_on(crate::backend::views::program_bytes(&client, &code));
    let first = read();
    let again = read();
    let _ = std::fs::remove_file(&kept);
    assert_eq!(first.unwrap(), (body.clone(), "node"));
    assert_eq!(again.unwrap(), (body, "disk"), "the node was asked again");
}

/// A pane that claims a seat compiled out of the blob the roster lists
/// starts that code: its view is up when the claim returns, with nothing
/// fetched or compiled again. Here the node is gone and the blob is off
/// the disk by then, so a load that fetched would fail.
#[test]
fn a_claimed_seat_starts_the_code_it_compiled() {
    let view = view_declaring("Quick", "", "");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: "quick-view".into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = lock(&registry)[&("quick-view", 0)].clone();
    eventually("the preload landed", || !lock(&seat).slot.loading());
    let _ = std::fs::remove_file(
        crate::backend::dirs()
            .cache
            .join("programs")
            .join(abi::hex(code.digest())),
    );
    let gone = Connection {
        client: Some(crate::backend::RpcClient::new("http://127.0.0.1:9")),
        ..node.asked_of.clone()
    };
    drop(node);
    let preloaded = match &lock(&seat).slot {
        Slot::Compiled(view) => view.clone(),
        _ => panic!("the preload did not land compiled"),
    };
    assert!(lock(&seat).slot.startable(Some((code, true))));
    let claim = |instance| {
        let mut locked = lock(&seat);
        locked.instance = instance;
        Load {
            program: "quick-view",
            seat: seat.clone(),
            generation: locked.start(),
            asked_of: gone.clone(),
            code: Some((code, true)),
        }
    };
    // as `mounted` starts it, on the claiming thread
    claim(7).land(|load, timing| {
        View::start(load.program, load.code, load.generation, &load.seat, timing)
    });
    assert!(
        matches!(&lock(&seat).slot, Slot::Ready(view) if view.name == "Quick" && view.instance == 7),
        "the claim did not start the compiled view"
    );
    // as a loader starts a seat claimed while its preload was on the way
    lock(&seat).slot = Slot::Compiled(preloaded);
    queue(claim(8));
    eventually(
        "the loader started the compiled view",
        || matches!(&lock(&seat).slot, Slot::Ready(view) if view.instance == 8),
    );
}

/// A pane that opens a program whose seat compiled out of the blob the
/// roster lists has its view up when `mounted` returns: started on the
/// thread that claimed it, with no loader and no stand-in between. The
/// node is gone and the blob off the disk by then, so a load would fail.
#[test]
fn a_pane_opens_its_compiled_view_at_once() {
    const PROGRAM: &str = "opened-at-once";
    let view = view_declaring("At once", "", "");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: PROGRAM.into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, seats) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &seats);
    let seat = lock(&seats)[&(PROGRAM, 0)].clone();
    eventually("the preload landed", || !lock(&seat).slot.loading());
    drop(node);
    let _ = std::fs::remove_file(
        crate::backend::dirs()
            .cache
            .join("programs")
            .join(abi::hex(code.digest())),
    );
    // the app's roster lists it, and its seat is the app's
    super::super::roster::roster()
        .lock()
        .push(crate::backend::views::Program {
            name: PROGRAM.into(),
            code,
            bare: true,
        });
    lock(registry()).insert((PROGRAM, 0), seat);
    let opened = mounted(PROGRAM, 7101);
    let started = matches!(&lock(&opened).slot, Slot::Ready(view) if view.name == "At once");
    lock(registry()).remove(&(PROGRAM, 7101));
    super::super::roster::roster()
        .lock()
        .retain(|program| program.name != PROGRAM);
    assert!(
        started,
        "the view was not up when the pane's claim returned"
    );
}

/// Only a compiled seat out of the very blob a pane asks for is started as
/// it is; anything else is loaded.
#[test]
fn a_seat_is_startable_only_compiled_out_of_the_blob_asked_for() {
    let (one, other) = (blob_id(b"one"), blob_id(b"other"));
    let compiled = |blob| {
        Slot::Compiled(Box::new(ViewCode {
            blob,
            ..*ViewCode::for_test("View", "", 320)
        }))
    };
    assert!(compiled(Some(one)).startable(Some((one, false))));
    assert!(
        !compiled(Some(one)).startable(Some((other, false))),
        "another blob"
    );
    assert!(
        !compiled(Some(one)).startable(None),
        "a blob the roster no longer lists"
    );
    assert!(!compiled(None).startable(None), "the developer's override");
    assert!(
        !compiled(None).startable(Some((one, false))),
        "the developer's override"
    );
    assert!(
        !Slot::Loading.startable(Some((one, false))),
        "not compiled yet"
    );
}

/// A pane that claims a seat preloaded only as far as compiled starts its
/// view: the seat leaves `Compiled` for a load of its own (with no node in
/// a test, a failure offering Retry), where its pane said "Loading" for good.
#[test]
fn a_pane_starts_the_view_its_seat_compiled() {
    const PROGRAM: &str = "claimed-compiled-seat";
    let preloaded = Mounted::seat();
    lock(&preloaded).slot = Slot::Compiled(ViewCode::for_test("Claimed", "", 320));
    lock(registry()).insert((PROGRAM, 0), preloaded);
    let seat = mounted(PROGRAM, 7001);
    eventually("the claiming pane's load ran", || {
        matches!(lock(&seat).slot, Slot::Failed(_))
    });
    lock(registry()).remove(&(PROGRAM, 7001));
}

/// A pane that claims a seat while its preload is on the way: the preload
/// lands compiled, and the pane's own load starts the view.
#[test]
fn a_seat_claimed_during_its_preload_is_started() {
    let seat = Mounted::seat();
    let generation = {
        let mut locked = lock(&seat);
        locked.instance = 7;
        locked.start()
    };
    let preload = Load {
        program: "claimed-during-preload",
        seat: seat.clone(),
        generation,
        asked_of: Connection::default(),
        code: None,
    };
    preload.land(|_, _| Ok(Loaded::Compiled(ViewCode::for_test("Claimed", "", 320))));
    eventually("the claiming pane's own load ran", || {
        matches!(lock(&seat).slot, Slot::Failed(_))
    });
}

/// A seat preloaded as compiled and reloading (a new deployment, a
/// reconnect) when a pane claims it: the pane's own load is the one the
/// seat waits for, and the stale reload, landing compiled, starts nothing.
/// Handing off on what the seat holds, it superseded the pane's load, and
/// the two loads leapfrogged with the pane on "Loading" for good.
#[test]
fn a_stale_preload_does_not_supersede_the_panes_load() {
    let seat = Mounted::seat();
    lock(&seat).slot = Slot::Compiled(ViewCode::for_test("Stale", "", 320));
    let reload = lock(&seat).start();
    let pane = {
        let mut locked = lock(&seat);
        locked.instance = 7;
        locked.start()
    };
    Load {
        program: "stale-preload",
        seat: seat.clone(),
        generation: reload,
        asked_of: Connection::default(),
        code: None,
    }
    .land(|_, _| Ok(Loaded::Compiled(ViewCode::for_test("Stale", "", 320))));
    assert_eq!(
        lock(&seat).generation,
        pane,
        "the stale reload superseded the pane's own load"
    );
}

/// A claimed seat's load asked of a node the app has since left lands
/// nowhere, and starts nothing: the new node's roster read asks the seat
/// again. Handing off on what the seat holds, every follow-on load reused
/// the left node and was refused in turn, without end.
#[test]
fn a_load_from_a_node_the_app_left_starts_no_other() {
    let seat = Mounted::seat();
    lock(&seat).slot = Slot::Compiled(ViewCode::for_test("Left", "", 320));
    let generation = {
        let mut locked = lock(&seat);
        locked.instance = 7;
        locked.start()
    };
    let left = Connection {
        rev: connection().lock().unwrap().rev.wrapping_add(1),
        ..Connection::default()
    };
    Load {
        program: "left-node",
        seat: seat.clone(),
        generation,
        asked_of: left,
        code: None,
    }
    .land(|_, _| Ok(Loaded::Compiled(ViewCode::for_test("Left", "", 320))));
    assert_eq!(
        lock(&seat).generation,
        generation,
        "a load from a left node started another"
    );
}
