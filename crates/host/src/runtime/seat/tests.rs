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

/// Claim: a seat's code is being read while its bytes come, waits once they
/// are all here (shown or not) and is prepared while a loader compiles it,
/// and is settled once drawn, compiled, none or failed; new code the network
/// named over a settled seat goes the same way, never back, the view held
/// staying up, until its load lands; a load that moved on since leaves it
/// settled.
#[test]
fn a_seat_is_prepared_by_its_stage_and_by_new_code_on_its_way() {
    let seat = Mounted::seat();
    let mut locked = seat.lock().unwrap();
    let mut prep = |slot| {
        locked.slot = slot;
        locked.prep()
    };
    let fetching = |received, total| Slot::Fetching { received, total };
    assert_eq!(prep(Slot::Loading), Prep::Reading);
    assert_eq!(prep(fetching(1, Some(2))), Prep::Reading);
    assert_eq!(prep(fetching(2, Some(2))), Prep::Waiting);
    assert_eq!(prep(Slot::Listed(Box::default())), Prep::Waiting);
    assert_eq!(prep(Slot::Compiling(Box::default())), Prep::Working);
    let compiled = || Slot::Compiled(ViewCode::for_test("Held", "", 320));
    for slot in [
        compiled(),
        Slot::Empty,
        Slot::Failed(Failure::Refused("x".into())),
    ] {
        assert_eq!(prep(slot), Prep::Settled);
    }
    locked.slot = compiled();
    let generation = locked.start();
    locked.replacing = Some((generation, Prep::Reading));
    assert_eq!(locked.prep(), Prep::Reading, "new code coming");
    for (stage, prep) in [
        (fetching(2, Some(2)), Prep::Waiting),
        (fetching(0, None), Prep::Waiting),
        (Slot::Compiling(Box::default()), Prep::Working),
    ] {
        locked.show(generation, stage);
        assert_eq!(locked.prep(), prep, "the new code's stage");
    }
    assert!(
        matches!(locked.slot, Slot::Compiled(_)),
        "the view held went"
    );
    let asked_of = connection().lock().unwrap().clone();
    let mut timing = LoadTiming::default();
    let loaded = Ok(Loaded::Empty);
    install(
        "seat-tests-prep",
        &mut locked,
        generation,
        &asked_of,
        None,
        loaded,
        &mut timing,
    );
    assert_eq!(locked.prep(), Prep::Settled, "landed and still preparing");
    locked.slot = compiled();
    let generation = locked.start();
    locked.replacing = Some((generation, Prep::Working));
    locked.start();
    assert_eq!(locked.prep(), Prep::Settled, "a load that moved on");
}

/// Claim: a load's stage never steps back (its loader reading again the
/// bytes the read ahead read leaves it listed), and a load that starts over
/// begins at `Loading`, none of the last one's stages its own.
#[test]
fn a_loads_stage_never_steps_back_and_a_new_load_starts_over() {
    let seat = Mounted::seat();
    let mut locked = seat.lock().unwrap();
    let generation = locked.start();
    locked.show(generation, Slot::Listed(Box::default()));
    let fetching = Slot::Fetching {
        received: 0,
        total: None,
    };
    locked.show(generation, fetching);
    assert!(
        matches!(locked.slot, Slot::Listed(_)),
        "the stage stepped back"
    );
    locked.show(generation, Slot::Compiling(Box::default()));
    assert!(matches!(locked.slot, Slot::Compiling(_)));
    locked.start();
    assert!(
        matches!(locked.slot, Slot::Loading),
        "the new load began where the last one was"
    );
}

/// Claim: a load a pane claimed while it waited (a tap on a tile still
/// waiting) goes ahead of the preloads queued before it.
#[test]
fn a_load_a_pane_claimed_goes_first() {
    let load = |instance| {
        let seat = Mounted::seat();
        lock(&seat).instance = instance;
        Load {
            program: "seat-tests-next",
            seat,
            generation: 1,
            asked_of: Connection::default(),
            code: None,
        }
    };
    assert_eq!(
        first_claimed_or_first_waiting_load(&VecDeque::from([load(0), load(0), load(3), load(0)])),
        2
    );
    assert_eq!(
        first_claimed_or_first_waiting_load(&VecDeque::from([load(0), load(0)])),
        0
    );
}

/// Claim: a roster read reads every preload's bytes before any is prepared,
/// a view's kept as its manifest and the bytes let go; until the last is
/// read none is shown (each counts as read, still on its way), and then all
/// are, at once under the registry's lock the rail reads under: one listed
/// and prepared, one with no view none, and one whose bytes did not come
/// failed, holding none of the others. (That the loader's second read is
/// off the disk is not seen here: a test thread's files are its own, and
/// the read ahead runs on the runtime's threads.)
#[test]
fn a_roster_read_reads_every_program_before_it_prepares_any() {
    let view = view_declaring("Alpha", "", Tint::None, "");
    let code = blob_id(&view);
    let listed_view = ::programs::View {
        name: "ahead-view".into(),
        view: code,
    };
    let viewless = b"\0asm\x01\0\0\0".to_vec();
    let none = ::programs::Entry {
        program: "ahead-none".into(),
        code: blob_id(&viewless),
        params: Vec::new(),
    };
    let node = FakeNode::new(
        vec![none, listed("ahead-held", "never-served")],
        vec![listed_view],
        vec![view, viewless],
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = |program| lock(&registry)[&(program, 0)].clone();
    let (view, none, held) = (seat("ahead-view"), seat("ahead-none"), seat("ahead-held"));
    for seat in [&view, &none] {
        wait_for_seat(seat, |seat| seat.prep() == Prep::Waiting);
    }
    node.wait_for_held(|held| held == 1);
    assert!(
        matches!(lock(&view).slot, Slot::Fetching { .. }),
        "shown or prepared before every program was read"
    );
    assert!(
        matches!(lock(&none).slot, Slot::Fetching { .. }),
        "landed before every program was read"
    );
    assert_eq!(lock(&held).prep(), Prep::Reading);
    // the node lets the third go unanswered, under the rail's lock
    let answers = node.held.clone();
    let changed = node.changed.clone();
    let rail = lock(super::registry());
    drop(node);
    drop(
        changed
            .wait_while(answers.lock().unwrap(), |held| held.now != 0)
            .unwrap(),
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        matches!(lock(&view).slot, Slot::Fetching { .. }),
        "shown while the rail's lock was held"
    );
    drop(rail);
    for seat in [&view, &none, &held] {
        wait_for_seat(seat, |seat| !seat.slot.loading());
    }
    assert!(matches!(&lock(&view).slot, Slot::Compiled(code) if code.name == "Alpha"));
    assert!(matches!(lock(&none).slot, Slot::Empty));
    assert!(matches!(lock(&held).slot, Slot::Failed(_)));
}

/// Claim: a pane that claims a seat whose preload is still in a read ahead
/// (a link opened at a cold start) loads it itself at once, while another
/// read of the batch hangs; the batch's load of it is let go when it flips.
#[test]
fn a_seat_claimed_mid_read_is_loaded_while_another_read_hangs() {
    const PROGRAM: &str = "seat-tests-claimed-mid-read";
    let view = view_declaring("Claimed", "", Tint::None, "");
    let code = blob_id(&view);
    let listed_view = ::programs::View {
        name: PROGRAM.into(),
        view: code,
    };
    let node = FakeNode::new(
        vec![listed("seat-tests-hangs", "never-served")],
        vec![listed_view],
        vec![view],
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = lock(&registry)[&(PROGRAM, 0)].clone();
    wait_for_seat(&seat, |seat| seat.prep() == Prep::Waiting);
    node.wait_for_held(|held| held == 1);
    lock(super::registry()).insert((PROGRAM, 0), seat);
    let opened = mounted(PROGRAM, 7401);
    // with no node in a test, the pane's own load fails, offering Retry
    wait_for_seat(&opened, |seat| matches!(seat.slot, Slot::Failed(_)));
    assert_eq!(node.held().0, 1, "the other read stopped hanging");
    lock(super::registry()).remove(&(PROGRAM, 7401));
}

/// Claim: a read that panics is that program's failure, as a loader's
/// panic is, and the read ahead flips with every other program shown.
#[test]
fn a_read_that_panics_lands_failed_and_the_batch_flips() {
    let view = view_declaring("Beside", "", Tint::None, "");
    let code = blob_id(&view);
    let listed_view = ::programs::View {
        name: "seat-tests-beside-a-panic".into(),
        view: code,
    };
    let node = FakeNode::new(
        vec![listed(PANICS_READING, "panics")],
        vec![listed_view],
        vec![view],
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = |program| lock(&registry)[&(program, 0)].clone();
    let (panics, beside) = (seat(PANICS_READING), seat("seat-tests-beside-a-panic"));
    for seat in [&panics, &beside] {
        wait_for_seat(seat, |seat| !seat.slot.loading());
    }
    assert!(
        matches!(&lock(&panics).slot, Slot::Failed(failure) if failure.to_string().contains("stopped reading")),
        "the panic not its failure"
    );
    assert!(matches!(&lock(&beside).slot, Slot::Compiled(code) if code.name == "Beside"));
}

/// Claim: a roster read that finds a program moved to new code over a view
/// the seat holds counts it as being prepared while the new code comes, the
/// view held staying up; one that asks the same code again of a new node
/// does not: nothing new is coming.
#[test]
fn new_code_over_a_held_view_counts_as_being_prepared() {
    let (moved, same) = ("seat-tests-moved", "seat-tests-same");
    let node = FakeNode::new(
        vec![listed(moved, "moved-new"), listed(same, "same")],
        Vec::new(),
        Vec::new(),
    );
    let (roster, registry) = (Roster::default(), Registry::default());
    let held = |name: &str, seed: &str| crate::backend::views::Program {
        name: name.into(),
        code: blob_id(seed.as_bytes()),
        bare: false,
    };
    roster
        .lock()
        .extend([held(moved, "moved-old"), held(same, "same")]);
    let seat = |program| {
        let seat = Mounted::seat();
        {
            let mut locked = seat.lock().unwrap();
            locked.slot = Slot::Compiled(ViewCode::for_test("Held", "", 320));
            locked.generation = 1;
            // compiled off another node: both are asked again
            locked.rev = node.asked_of.rev.wrapping_add(1);
        }
        lock(&registry).insert((intern(program), 0), seat.clone());
        seat
    };
    let (moved_seat, same_seat) = (seat(moved), seat(same));
    read_roster(node.asked_of.clone(), &roster, &registry);
    assert_ne!(
        lock(&moved_seat).prep(),
        Prep::Settled,
        "new code not counted"
    );
    assert_eq!(
        lock(&same_seat).prep(),
        Prep::Settled,
        "the same code counted"
    );
    // both asked of the node, which holds them: neither has landed
    node.wait_for_held(|held| held == 2);
    assert_eq!(lock(&moved_seat).prep(), Prep::Reading);
    assert!(
        matches!(lock(&moved_seat).slot, Slot::Compiled(_)),
        "the view held went"
    );
    assert_eq!(lock(&same_seat).prep(), Prep::Settled);
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

/// The start a load parked on `seat`, once it is there, run as the
/// window thread runs it (`start_in`): at once, on the wasmtime runner.
fn start_parked(seat: &Arc<Mutex<Mounted>>, program: &'static str) {
    wait_for_seat(seat, |seat| {
        seat.to_start.is_some() || matches!(seat.slot, Slot::Failed(_) | Slot::Empty)
    });
    assert!(lock(seat).to_start.is_some(), "no start was parked");
    futures::executor::block_on(start_in(seat.clone(), program));
}

pub(in crate::runtime) fn wait_for_seat(
    seat: &Arc<Mutex<Mounted>>,
    done: impl Fn(&Mounted) -> bool,
) {
    let mut changed = lock(seat).wake.subscribe();
    futures::executor::block_on(async {
        loop {
            changed.borrow_and_update();
            if done(&lock(seat)) {
                break;
            }
            changed.changed().await.expect("the seat changed");
        }
    });
}

/// A node on a local socket: it answers the programs role's two roster
/// questions with `programs` and `views`, the roles question with `roles`
/// (counted), and
/// a blob among `blobs` (bodies) with its bytes. Any other blob it holds —
/// the request read, no answer — until it is dropped, counting the blobs it
/// holds at once and the most it ever did.
pub(in crate::runtime) struct FakeNode {
    pub(in crate::runtime) asked_of: Connection,
    held: Arc<Mutex<Held>>,
    changed: Arc<std::sync::Condvar>,
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
    /// The network its status names.
    network: String,
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
    pub(in crate::runtime) fn new(
        programs: Vec<::programs::Entry>,
        views: Vec<::programs::View>,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        Self::with_roles(programs, views, RolesAnswer::Bound(roles()), blobs)
    }

    /// [`FakeNode::new`] on `network` (a followed set's own, apart from
    /// every other test's), its status naming it.
    pub(in crate::runtime) fn on(
        network: &str,
        programs: Vec<::programs::Entry>,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        Self::serving(
            network,
            programs,
            Vec::new(),
            RolesAnswer::Bound(roles()),
            blobs,
        )
    }

    fn with_roles(
        programs: Vec<::programs::Entry>,
        views: Vec<::programs::View>,
        roles: RolesAnswer,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        Self::serving("fake-network", programs, views, roles, blobs)
    }

    fn serving(
        network: &str,
        programs: Vec<::programs::Entry>,
        views: Vec<::programs::View>,
        roles: RolesAnswer,
        blobs: Vec<Vec<u8>>,
    ) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let answers = Arc::new(Answers {
            network: network.into(),
            programs,
            views,
            roles,
            blobs: blobs
                .into_iter()
                .map(|body| (blob_id(&body), framed(&body)))
                .collect(),
        });
        let held = Arc::new(Mutex::new(Held::default()));
        let changed = Arc::new(std::sync::Condvar::new());
        let holding = held.clone();
        let changes = changed.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (answers, held) = (answers.clone(), holding.clone());
                let changed = changes.clone();
                std::thread::spawn(move || answer(stream, &answers, &held, &changed));
            }
        });
        FakeNode {
            asked_of: Connection {
                client: Some(crate::backend::RpcClient::new(format!("http://{address}"))),
                network: network.into(),
                chain: format!("{network}#0"),
                rev: connection().lock().unwrap().rev,
                live: None,
                apart: None,
            },
            held,
            changed,
        }
    }

    /// The blobs held now, and the most ever held at once.
    pub(in crate::runtime) fn held(&self) -> (usize, usize) {
        let held = self.held.lock().unwrap();
        (held.now, held.most)
    }

    pub(in crate::runtime) fn wait_for_held(&self, done: impl Fn(usize) -> bool) {
        drop(
            self.changed
                .wait_while(self.held.lock().unwrap(), |held| !done(held.now))
                .unwrap(),
        );
    }
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        self.held.lock().unwrap().released = true;
        self.changed.notify_all();
    }
}

fn answer(
    mut stream: std::net::TcpStream,
    answers: &Answers,
    held: &Mutex<Held>,
    changed: &std::sync::Condvar,
) {
    use super::super::kernel::tests::{read_request, respond};
    let (line, body) = read_request(&mut stream);
    if line.starts_with("GET /v1/status ") {
        let status = noded_client::wire::Status {
            network: answers.network.clone(),
            ..status(0)
        };
        return respond(&mut stream, "200 OK", &abi::encode(&status));
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
    changed.notify_all();
    drop(
        changed
            .wait_while(held.lock().unwrap(), |held| !held.released)
            .unwrap(),
    );
    respond(&mut stream, "404 Not Found", b"gone");
    held.lock().unwrap().now -= 1;
    changed.notify_all();
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
                apart: None,
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
    let roles = MockHost::roles();
    let seated = roles.iter().map(|(_, seat)| Entry {
        program: seat.clone(),
        code: blob_id(seat.as_bytes()),
        params: Vec::new(),
    });
    let genesis = Genesis {
        programs: std::iter::once(entry(running)).chain(seated).collect(),
        views: Vec::new(),
        hooks: Vec::new(),
        roles,
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

#[test]
fn the_roster_is_due_again_at_its_next_scheduled_change() {
    let node = ProgramsNode::swapping("due-swap", 5);
    let (roster, registry) = (Roster::default(), Registry::default());
    assert!(roster.due(0), "a roster never read is due");
    node.code_at(2, "due-swap", &roster, &registry);
    assert!(!roster.due(2) && !roster.due(3), "the swap lands at 5");
    assert!(roster.due(4), "the block being built is at its height");
    node.code_at(4, "due-swap", &roster, &registry);
    assert!(!roster.due(1_000), "nothing more is scheduled");
}

/// A blob body as the node stores a program's code: `program <len>\0<body>`.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut framed = format!("program {}\0", body.len()).into_bytes();
    framed.extend_from_slice(body);
    framed
}

pub(in crate::runtime) fn blob_id(body: &[u8]) -> abi::BlobId {
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
        2,
        "the next scheduled change is read with each roster read"
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
            roster.due(0),
            "a failed read is due again at the next block"
        );
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
    node.wait_for_held(|held| held > 0);
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
        .recv()
        .expect("the roster read waited on the load it queued");
    node.wait_for_held(|held| held == 1);
}

/// A view whose manifest says `name`, `icon` (none when empty), `tint`
/// and a 320px minimum, its `init` the instructions given; its other
/// exports trap.
fn view_declaring(name: &str, icon: &str, tint: Tint, init: &str) -> Vec<u8> {
    let manifest = format!(
        "ducktape.view.manifest\\n{name}\\n\\n\\n320\\n{}\\n\\n{icon}\\n{}",
        wire::WIRE_ID,
        tint.as_str()
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
    let view = view_declaring("Lazy", "", Tint::None, "unreachable");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: "lazy-view".into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = lock(&registry)[&("lazy-view", 0)].clone();
    wait_for_seat(&seat, |seat| !seat.slot.loading());
    assert!(
        matches!(
            &lock(&seat).slot,
            Slot::Compiled(view) if view.name == "Lazy" && view.icon.is_empty() && view.min_width == 320
        ),
        "the unopened view did not land compiled and unstarted"
    );
    // asked again (the seat not yet asked of this node, as on a reconnect):
    // compiled again, still unstarted, though the seat holds its code
    let mut landed = {
        let mut locked = lock(&seat);
        locked.rev = locked.rev.wrapping_add(1);
        locked.wake.subscribe()
    };
    read_roster(node.asked_of.clone(), &roster, &registry);
    futures::executor::block_on(landed.changed()).expect("the second preload landed");
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
    start_parked(&seat, "lazy-view");
    assert!(
        matches!(&lock(&seat).slot, Slot::Failed(failure) if failure.to_string().contains("init trapped")),
        "the claiming pane's load ran init"
    );
    // the fetch kept the blob in the developer's own cache: it goes
    let kept = crate::backend::dirs()
        .cache
        .join("programs")
        .join(abi::hex(code.digest()));
    let _ = std::fs::remove_file(&kept);
}

/// A rail row's icon and tint are the ones its view's manifest declares,
/// read with its name as the seat lands compiled and kept once a pane has
/// started the view; a view that declares none has none.
#[test]
fn a_rail_row_carries_the_icon_and_tint_its_views_manifest_declares() {
    const ICON: &str = "icons/hammer.svg";
    let views = [
        (
            "iconed-view",
            view_declaring("Iconed", ICON, Tint::Orange, ""),
        ),
        ("plain-view", view_declaring("Plain", "", Tint::None, "")),
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
        wait_for_seat(&seat, |seat| !seat.slot.loading());
        lock(registry()).insert((*program, 0), seat);
    }
    let icon = |program: &str| {
        let rows = roster.rail();
        let row = rows.iter().find(|row| row.program == program);
        let row = row.expect("a listed view");
        (row.icon.clone(), row.tint)
    };
    assert_eq!(
        icon("iconed-view"),
        (ICON.into(), Tint::Orange),
        "compiled, not started"
    );
    assert_eq!(
        icon("plain-view"),
        (String::new(), Tint::None),
        "its manifest declares none"
    );
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
    start_parked(&seat, "iconed-view");
    assert!(matches!(lock(&seat).slot, Slot::Ready(_)));
    assert_eq!(icon("iconed-view"), (ICON.into(), Tint::Orange), "started");
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
    let read = || {
        handle()
            .block_on(crate::backend::views::program_bytes(&client, &code))
            .map(|(blob, from)| (blob.to_vec(), from))
    };
    let first = read();
    let again = read();
    let _ = std::fs::remove_file(&kept);
    assert_eq!(first.unwrap(), (body.clone(), "node"));
    assert_eq!(again.unwrap(), (body, "disk"), "the node was asked again");
}

/// A program's code the cache cannot keep (a full disk; here a file where
/// its folder goes) is read all the same: whole off the node, hashed, as
/// before there was a cache. Before, the view did not open.
#[test]
fn a_program_the_cache_cannot_keep_is_read_all_the_same() {
    let body = b"\0asm\x01\0\0\0 a program with no room to keep".to_vec();
    let code = blob_id(&body);
    let programs = crate::backend::dirs().cache.join("programs");
    let _ = std::fs::remove_dir_all(&programs);
    std::fs::create_dir_all(programs.parent().unwrap()).unwrap();
    std::fs::write(&programs, b"not a folder").unwrap();
    let node = FakeNode::new(Vec::new(), Vec::new(), vec![body.clone()]);
    let client = node.asked_of.client.clone().unwrap();
    let read = handle().block_on(crate::backend::views::program_bytes(&client, &code));
    let _ = std::fs::remove_file(&programs);
    let (read, from) = read.unwrap();
    assert_eq!((&*read, from), (&body[..], "node"));
}

/// A program's code is streamed off the node into the cache as it lands,
/// never held whole: half of it is on the disk while the node still holds
/// the rest back, in a file named by this process (another instance of the
/// app landing the same code never truncates it). It is kept only whole
/// and hashing to its id: other bytes, or fewer than the answer said,
/// leave nothing behind.
#[test]
fn a_program_is_streamed_into_the_cache_and_kept_only_whole() {
    use super::super::kernel::tests::{read_request, respond};
    use std::io::Write as _;
    let body = vec![7; 64 << 10];
    let code = blob_id(&body);
    // the files `kept` is written beside, as they land, by this process
    let landing = |kept: &std::path::Path| {
        let name = format!("{}.{}-", abi::hex(code.digest()), std::process::id());
        std::fs::read_dir(kept.parent().unwrap())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|file| {
                let file = file.file_name().to_string_lossy().into_owned();
                file.starts_with(&name) && file.ends_with(".partial")
            })
            .map(|file| file.metadata().unwrap().len())
            .collect::<Vec<_>>()
    };
    let answer = abi::encode(&Some(framed(&body)));
    let mut other = answer.clone();
    *other.last_mut().unwrap() ^= 1;
    let short = answer[..answer.len() - 3].to_vec();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client =
        crate::backend::RpcClient::new(format!("http://{}", listener.local_addr().unwrap()));
    let served = answer.clone();
    std::thread::spawn(move || {
        let mut streams = listener.incoming().flatten();
        let mut stream = streams.next().unwrap();
        read_request(&mut stream);
        let (half, rest) = served.split_at(served.len() / 2);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            served.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(half).unwrap();
        released.recv().unwrap();
        stream.write_all(rest).unwrap();
        for (mut stream, answer) in streams.zip([other, short]) {
            read_request(&mut stream);
            respond(&mut stream, "200 OK", &answer);
        }
    });
    // a test thread's files are its own: the reading thread says where
    let (path, kept) = std::sync::mpsc::channel();
    let read = {
        let client = client.clone();
        std::thread::spawn(move || {
            path.send(crate::backend::views::cache_path(&code)).unwrap();
            handle()
                .block_on(crate::backend::views::program_bytes(&client, &code))
                .map(|(blob, from)| (blob.to_vec(), from))
        })
    };
    let there = kept.recv().unwrap();
    while landing(&there)
        .first()
        .is_none_or(|landed| *landed < (answer.len() / 2 - 5) as u64)
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    release.send(()).unwrap();
    assert_eq!(read.join().unwrap().unwrap(), (body, "node"));
    assert!(there.exists() && landing(&there).is_empty());
    let kept = crate::backend::views::cache_path(&code);
    for bad in ["other bytes", "fewer bytes"] {
        use crate::backend::views::Fetch;
        let read = handle()
            .block_on(crate::backend::views::program_bytes(&client, &code))
            .map(|(blob, from)| (blob.len(), from));
        let refused = match bad {
            "other bytes" => matches!(read, Err(Fetch::Corrupt)),
            _ => matches!(read, Err(Fetch::Unreadable(_))),
        };
        assert!(refused, "{bad}: {read:?}");
        assert!(
            !kept.exists() && landing(&kept).is_empty(),
            "{bad} left a file"
        );
    }
}

/// A pane that claims a seat compiled out of the blob the roster lists
/// starts that code: its view is up when the claim returns, with nothing
/// fetched or compiled again. Here the node is gone and the blob is off
/// the disk by then, so a load that fetched would fail.
#[test]
fn a_claimed_seat_starts_the_code_it_compiled() {
    let view = view_declaring("Quick", "", Tint::None, "");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: "quick-view".into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, registry) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &registry);
    let seat = lock(&registry)[&("quick-view", 0)].clone();
    wait_for_seat(&seat, |seat| !seat.slot.loading());
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
    // as `mounted` claims it, on the claiming thread, which then starts it
    claim(7).land(|load, timing| View::start(load.program, load.code, &load.seat, timing));
    start_parked(&seat, "quick-view");
    assert!(
        matches!(&lock(&seat).slot, Slot::Ready(view) if view.name == "Quick" && view.instance == 7),
        "the claim did not start the compiled view"
    );
    // as a loader hands on a seat claimed while its preload was on the way
    lock(&seat).slot = Slot::Compiled(preloaded);
    queue(claim(8));
    start_parked(&seat, "quick-view");
    assert!(
        matches!(&lock(&seat).slot, Slot::Ready(view) if view.instance == 8),
        "the loader started the compiled view"
    );
}

/// A pane that opens a program whose seat compiled out of the blob the
/// roster lists has its view up in one poll of the start `mounted` parks
/// (`Seat::new` polls it on the claiming thread, `start_here`): no loader
/// and no stand-in between on the wasmtime runner. The node is gone and
/// the blob off the disk by then, so a load would fail.
#[test]
fn a_pane_opens_its_compiled_view_at_once() {
    const PROGRAM: &str = "opened-at-once";
    let view = view_declaring("At once", "", Tint::None, "");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: PROGRAM.into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, seats) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &seats);
    let seat = lock(&seats)[&(PROGRAM, 0)].clone();
    wait_for_seat(&seat, |seat| !seat.slot.loading());
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
    let polled = futures::FutureExt::now_or_never(start_in(opened.clone(), PROGRAM));
    let started = polled.is_some()
        && matches!(&lock(&opened).slot, Slot::Ready(view) if view.name == "At once");
    lock(registry()).remove(&(PROGRAM, 7101));
    super::super::roster::roster()
        .lock()
        .retain(|program| program.name != PROGRAM);
    assert!(
        started,
        "the view was not up when the pane's claim returned"
    );
}

/// A pane that lets its program go (a phone's Open closing it, a memory
/// warning): the seat is put back as the program's preloaded one, its view
/// stepped back to its compiled code, so the rail still lists the program
/// (not pending) and the next pane starts it with nothing loaded.
#[test]
fn a_seat_let_go_steps_back_to_its_code_and_stays_listed() {
    const PROGRAM: &str = "let-go";
    let view = view_declaring("Let go", "", Tint::None, "");
    let code = blob_id(&view);
    let listed = ::programs::View {
        name: PROGRAM.into(),
        view: code,
    };
    let node = FakeNode::new(Vec::new(), vec![listed], vec![view]);
    let (roster, seats) = (Roster::default(), Registry::default());
    read_roster(node.asked_of.clone(), &roster, &seats);
    let seat = lock(&seats)[&(PROGRAM, 0)].clone();
    wait_for_seat(&seat, |seat| !seat.slot.loading());
    drop(node);
    super::super::roster::roster()
        .lock()
        .push(crate::backend::views::Program {
            name: PROGRAM.into(),
            code,
            bare: true,
        });
    lock(registry()).insert((PROGRAM, 0), seat);
    let opened = mounted(PROGRAM, 7201);
    let started = futures::FutureExt::now_or_never(start_in(opened.clone(), PROGRAM)).is_some()
        && matches!(&lock(&opened).slot, Slot::Ready(_));

    let_go(PROGRAM, 7201);
    let back = lock(registry()).get(&(PROGRAM, 0)).cloned();
    let stepped = back.as_ref().map(|seat| {
        let locked = lock(seat);
        (
            matches!(&locked.slot, Slot::Compiled(code) if code.name == "Let go"),
            locked.instance,
        )
    });
    let row = super::super::roster::roster()
        .rail()
        .into_iter()
        .find(|row| row.program == PROGRAM);
    lock(registry()).remove(&(PROGRAM, 0));
    super::super::roster::roster()
        .lock()
        .retain(|program| program.name != PROGRAM);
    assert!(started, "the pane's view was not up");
    assert!(
        back.is_some_and(|seat| Arc::ptr_eq(&seat, &opened)),
        "the seat was not put back as the preloaded one"
    );
    assert_eq!(stepped, Some((true, 0)), "not stepped back to its code");
    let row = row.expect("the program left the rail");
    assert!(!row.pending, "the program let go reads pending");
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
    wait_for_seat(&seat, |seat| matches!(seat.slot, Slot::Failed(_)));
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
    wait_for_seat(&seat, |seat| matches!(seat.slot, Slot::Failed(_)));
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

#[test]
fn a_pane_mounted_before_the_roster_is_read_waits_for_the_read() {
    assert!(!super::asks_now(false, true, true), "asked of no node");
    assert!(
        !super::asks_now(true, false, false),
        "a pane failed as not listed before its network's roster was read"
    );
    assert!(super::asks_now(true, true, true));
    assert!(
        super::asks_now(true, false, true),
        "a program the read roster leaves out is never said to be"
    );
}
