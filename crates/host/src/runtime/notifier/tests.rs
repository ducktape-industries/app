//! Notifiers against a fake node: what a roster read starts, keeps,
//! replaces and stops, and what one posts with no pane of its program open.
use super::super::seat::tests::{FakeNode, blob_id, eventually};
use super::*;
use wire::methods::Notification;

/// A notifier's module, its manifest naming it `name`, declaring
/// `capabilities` (`notify,` and the like) and the target `chat`: its first
/// turn posts `post`, every later one asks nothing.
fn notifier_posting(name: &str, capabilities: &str, post: &Notification) -> Vec<u8> {
    let posting = wire::Request {
        id: 1,
        kind: "notify.post".into(),
        payload: wire::methods::encode(post),
    };
    notifier_asking(name, capabilities, vec![posting])
}

/// [`notifier_posting`], its first turn asking `requests`.
fn notifier_asking(name: &str, capabilities: &str, requests: Vec<wire::Request>) -> Vec<u8> {
    let turn = |requests| {
        let turn = wire::notifier::Turn {
            requests,
            cancels: Vec::new(),
            busy: false,
        };
        let bytes = borsh::to_vec(&turn).unwrap();
        let len = bytes.len() as u32;
        (
            bytes
                .iter()
                .map(|byte| format!("\\{byte:02x}"))
                .collect::<String>(),
            len,
        )
    };
    let (first, first_len) = turn(requests);
    let (then, then_len) = turn(Vec::new());
    let manifest = format!(
        "ducktape.view.manifest\\n{name}\\n\\n{capabilities}\\n1\\n{}\\nchat,\\n",
        wire::WIRE_ID
    );
    wat::parse_str(format!(
        r#"(module
        (@custom "{}" "{manifest}")
        (memory (export "memory") 2)
        (global $n (mut i32) (i32.const 0))
        (data (i32.const 65536) "{first}")
        (data (i32.const 69632) "{then}")
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        (func (export "init"))
        (func (export "tick") (param i32 i32) (result i64)
            global.get $n i32.const 1 i32.add global.set $n
            global.get $n i32.const 1 i32.le_u
            if (result i64) i64.const {} else i64.const {} end))"#,
        wire::manifest::MANIFEST_SECTION,
        wire::abi::pack(65536, first_len),
        wire::abi::pack(69632, then_len),
    ))
    .unwrap()
}

/// A program's code blob carrying `notifier` as its notify section, as a
/// founding packs it: a core module of custom sections.
fn blob_carrying(notifier: &[u8]) -> Vec<u8> {
    fn leb(out: &mut Vec<u8>, mut value: usize) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            out.push(if value == 0 { byte } else { byte | 0x80 });
            if value == 0 {
                return;
            }
        }
    }
    let name = crate::backend::views::NOTIFY_SECTION.as_bytes();
    let mut body = Vec::new();
    leb(&mut body, name.len());
    body.extend_from_slice(name);
    body.extend_from_slice(notifier);
    let mut blob = b"\0asm\x01\0\0\0".to_vec();
    blob.push(0);
    leb(&mut blob, body.len());
    blob.extend_from_slice(&body);
    blob
}

fn mention() -> Notification {
    Notification {
        title: "ada mentioned you".into(),
        body: "look".into(),
        tag: "#general".into(),
        link: String::new(),
    }
}

/// `program` listed with `blob` as its code, as a roster read lists it.
fn listed(program: &str, blob: &[u8]) -> Program {
    Program {
        name: program.into(),
        code: blob_id(blob),
        bare: false,
    }
}

/// `program`'s rows in the app's notification centre.
fn rows(program: &str) -> Vec<notify::Entry> {
    notify::center()
        .lock()
        .entries()
        .filter(|entry| entry.program == program)
        .cloned()
        .collect()
}

fn allow(program: &str) {
    crate::backend::edit_prefs(|prefs| {
        prefs["notify_views"] = serde_json::json!({ program: "Allow" });
    });
}

/// No pane of the program is open, nor ever was: its notifier, started by
/// the roster read, posts, and the centre holds one row for it, raised as
/// one banner, its tag the program's own.
#[test]
fn a_notice_with_no_pane_open_is_one_row_and_one_banner() {
    let program = "notifier-tests-alone";
    allow(program);
    let blob = blob_carrying(&notifier_posting("Alone", "notify,", &mention()));
    let node = FakeNode::new(
        vec![::programs::Entry {
            program: program.into(),
            code: blob_id(&blob),
            params: Vec::new(),
        }],
        Vec::new(),
        vec![blob.clone()],
    );
    let notifiers = Notifiers::default();
    notifiers.reconcile(&node.asked_of, &[listed(program, &blob)]);
    eventually("the notifier posted", || !rows(program).is_empty());
    // its later turns post nothing more
    std::thread::sleep(Duration::from_millis(100));
    let rows = rows(program);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].view.as_str(), rows[0].title.as_str(), rows[0].count),
        ("Alone", "ada mentioned you", 1)
    );
    let tag = format!("{program}/#general");
    let asked: Vec<_> = crate::contract::testing::banners_asked()
        .into_iter()
        .filter(|banner| banner.tag == tag)
        .collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        !lock(registry())
            .keys()
            .any(|(seated, _)| *seated == program),
        "no pane, no seat"
    );
    assert_eq!(notifiers.running(), [program]);
}

/// However many roster reads list the program (one each block), one
/// notifier runs for it: one post, not one per read. Other code listed
/// replaces it, and the program leaving the roster stops it.
#[test]
fn one_notifier_runs_per_program_until_its_code_moves() {
    let program = "notifier-tests-one";
    allow(program);
    let first = blob_carrying(&notifier_posting("One", "notify,", &mention()));
    let next = blob_carrying(&notifier_posting("One v2", "notify,", &mention()));
    let entry = |blob: &[u8]| ::programs::Entry {
        program: program.into(),
        code: blob_id(blob),
        params: Vec::new(),
    };
    let node = FakeNode::new(
        vec![entry(&first)],
        Vec::new(),
        vec![first.clone(), next.clone()],
    );
    let notifiers = Notifiers::default();
    notifiers.reconcile(&node.asked_of, &[listed(program, &first)]);
    eventually("the notifier posted", || !rows(program).is_empty());
    // the reads that follow, one a block, find it running
    for _ in 0..2 {
        notifiers.reconcile(&node.asked_of, &[listed(program, &first)]);
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(rows(program)[0].count, 1, "one notifier, one post");
    assert_eq!(notifiers.running(), [program]);

    // a new deployment: the new code's notifier posts under the same tag
    notifiers.reconcile(&node.asked_of, &[listed(program, &next)]);
    eventually("the new notifier posted", || {
        rows(program)[0].view == "One v2"
    });
    assert_eq!(notifiers.running(), [program]);

    notifiers.reconcile(&node.asked_of, &[]);
    assert!(notifiers.running().is_empty());
}

/// A program whose notices the person blocked runs no notifier; one let
/// through again does.
#[test]
fn a_blocked_program_runs_no_notifier() {
    let program = "notifier-tests-blocked";
    let blob = blob_carrying(&notifier_posting("Blocked", "notify,", &mention()));
    let node = FakeNode::new(Vec::new(), Vec::new(), vec![blob.clone()]);
    crate::backend::edit_prefs(|prefs| {
        prefs["notify_views"] = serde_json::json!({ program: "Block" });
    });
    // the kept settings are this thread's: read again
    Settings::load();
    let notifiers = Notifiers::default();
    notifiers.reconcile(&node.asked_of, &[listed(program, &blob)]);
    assert!(notifiers.running().is_empty());
    allow(program);
    Settings::load();
    notifiers.reconcile(&node.asked_of, &[listed(program, &blob)]);
    assert_eq!(notifiers.running(), [program]);
}

/// A notifier runs with no one in front of it: one declaring what a press
/// or a yes admits is refused before it starts.
#[test]
fn a_notifier_declaring_what_needs_a_person_is_refused() {
    let connection = connection().lock().unwrap().clone();
    for capability in ["link,", "op,", "clipboard,", "invite,"] {
        let bytes = notifier_posting("Pushy", &format!("notify,{capability}"), &mention());
        let refused = handle()
            .block_on(View::notifier("notifier-tests-pushy", &bytes, &connection))
            .err()
            .unwrap_or_else(|| panic!("{capability} started"));
        assert!(refused.contains("no one in front of it"), "{refused}");
    }
    let bytes = notifier_posting("Quiet", "notify,store,", &mention());
    let quiet = handle().block_on(View::notifier("notifier-tests-quiet", &bytes, &connection));
    assert!(quiet.is_ok(), "{:?}", quiet.err());
}

/// Banners a test reads back: each notice raised, as a channel sends it.
#[derive(Default)]
struct Raised(Mutex<Vec<notify::Notice>>);

impl crate::contract::Banners for Raised {
    fn post(&self, notice: &notify::Notice, _: Option<notify::Click>) -> bool {
        self.0.lock().unwrap().push(notice.clone());
        true
    }

    fn withdraw(&self, _: &str) {}
}

/// An account a host with no screen carries notices for, its store under
/// `store` and its notices raised on `raised`: what pushd hands
/// `notify_for`.
fn recipient(account: u64, store: PathBuf, raised: Arc<Raised>) -> Recipient {
    let session = super::super::props(true, "fake-network#0", "ab", Some(account), "");
    Recipient {
        session: tokio::sync::watch::channel(Some(session)).0,
        store,
        center: notify::CenterHandle::default(),
        settings: Settings {
            banners: true,
            in_front: false,
            burst: 6,
            views: Default::default(),
            unasked: Some(Permission::Allow),
        },
        banners: raised,
        said: futures::channel::mpsc::unbounded().0,
    }
}

/// A host with no screen runs a program's notifier once per account it
/// carries notices for: each one's notice goes into its own account's
/// centre and out its channel, with the program's words, tag and link,
/// never into the app's centre; and each keeps its store under its own
/// account's, so two accounts on one network and program never share a
/// file (chat keeps what it announced there).
#[test]
fn each_recipient_has_its_own_notices_and_store() {
    let program = "notifier-tests-recipients";
    let post = Notification {
        link: "duck://fake-network-00000000/chat/general/7".into(),
        ..mention()
    };
    let posting = wire::Request {
        id: 1,
        kind: "notify.post".into(),
        payload: wire::methods::encode(&post),
    };
    let keeping = wire::Request {
        id: 2,
        kind: "store.set".into(),
        payload: wire::methods::encode(&("notified".to_owned(), Some(b"7".to_vec()))),
    };
    let blob = blob_carrying(&notifier_asking(
        "Each",
        "notify,store,",
        vec![posting, keeping],
    ));
    let node = FakeNode::new(Vec::new(), Vec::new(), vec![blob.clone()]);
    let stores = tempfile::tempdir().unwrap();
    let accounts = [2u64, 3];
    let sets: Vec<(Arc<Raised>, PathBuf, Notifiers)> = accounts
        .iter()
        .map(|account| {
            let raised = Arc::new(Raised::default());
            let store = stores.path().join(account.to_string());
            let notifiers = Notifiers {
                running: Mutex::default(),
                recipient: Some(Arc::new(recipient(*account, store.clone(), raised.clone()))),
            };
            notifiers.reconcile(&node.asked_of, &[listed(program, &blob)]);
            (raised, store, notifiers)
        })
        .collect();
    for (raised, store, _) in &sets {
        let file = store
            .join("fake-network_230")
            .join(format!("{program}.borsh"));
        eventually("each account's notifier posted and kept", || {
            !raised.0.lock().unwrap().is_empty() && file.exists()
        });
        let raised = raised.0.lock().unwrap();
        assert_eq!(raised.len(), 1, "{raised:?}");
        assert_eq!(
            (
                &raised[0].program[..],
                &raised[0].tag[..],
                &raised[0].link[..]
            ),
            (program, &format!("{program}/#general")[..], &post.link[..])
        );
    }
    assert!(rows(program).is_empty(), "the app's centre heard of it");
    let kept = std::fs::read_dir(stores.path()).unwrap().count();
    assert_eq!(kept, 2, "one store per account");
}
