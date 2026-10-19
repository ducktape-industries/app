use super::*;

fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!("ducktape-store-{}", rand::random::<u64>()))
}

fn set(key: &str, value: Option<&[u8]>) -> Request {
    Request::Set(key.to_owned(), value.map(<[u8]>::to_vec))
}

fn put(path: &Path, key: &str, value: Option<&[u8]>) -> Answer {
    run(path, &set(key, value))
}

fn read(path: &Path, key: &str) -> Option<Vec<u8>> {
    methods::decode(&run(path, &Request::Get(key.to_owned())).unwrap()).unwrap()
}

fn listed(path: &Path, prefix: &str) -> Vec<String> {
    methods::decode(&run(path, &Request::List(prefix.to_owned())).unwrap()).unwrap()
}

/// The code `operation` with `payload` is refused with.
fn refused(operation: &str, payload: &[u8]) -> String {
    Ask::read(Capability::Store, operation, payload)
        .unwrap()
        .unwrap_err()
        .code
}

/// The device's file on `chain` for `program`: no key seated.
fn device(config: &Path, chain: &str, program: &str) -> File {
    located(config, chain, "", program)
}

/// The props a view is handed with `signer` seated.
fn seated(signer: &str) -> Option<Vec<u8>> {
    Some(super::super::roster::props(
        true, "dev#01", signer, None, "",
    ))
}

/// Every reply `view` has had within ten seconds, until `enough` says so.
fn replies(view: &mut View, enough: impl Fn(&[wire::Event]) -> bool) -> Vec<wire::Event> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut heard = Vec::new();
    while !enough(&heard) && std::time::Instant::now() < deadline {
        view.replies.drain_into(&mut heard);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    heard
}

/// The keys a `store.changes` subscription (`id`) heard, in order.
fn heard_keys(events: &[wire::Event], id: u64) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            wire::Event::Response {
                id: heard,
                result: Ok(key),
                done: false,
            } if *heard == id => Some(methods::decode::<String>(key).unwrap()),
            _ => None,
        })
        .collect()
}

/// Each ask reads as its request; an empty key, a payload that is not
/// the method's or one given to `store.changes` is malformed; another
/// operation is not the store's.
#[test]
fn asks_read_as_their_requests() {
    let key = methods::encode(&"reads".to_owned());
    let pair = methods::encode(&("reads".to_owned(), Some(b"1".to_vec())));
    let file = |request| Some(Ok(Ask::File(request)));
    assert_eq!(
        Ask::read(Capability::Store, "get", &key),
        file(Request::Get("reads".into()))
    );
    assert_eq!(
        Ask::read(Capability::Store, "set", &pair),
        file(set("reads", Some(b"1")))
    );
    assert_eq!(
        Ask::read(Capability::Store, "list", &key),
        file(Request::List("reads".into()))
    );
    assert_eq!(
        Ask::read(Capability::Store, "changes", &[]),
        Some(Ok(Ask::Changes))
    );
    assert_eq!(Ask::read(Capability::Store, "open", &key), None);
    let malformed = methods::refusal::MALFORMED_REQUEST;
    let empty = methods::encode(&String::new());
    assert_eq!(refused("get", &empty), malformed);
    let unnamed = methods::encode(&(String::new(), Some(b"x".to_vec())));
    assert_eq!(refused("set", &unnamed), malformed);
    assert_eq!(refused("list", &[1]), malformed);
    assert_eq!(refused("changes", &key), malformed);
}

/// A request applied to the map: a `set` keeps or drops its key and is
/// the one that hands a map back to write; a `get` and a `list` answer
/// from it and write nothing.
#[test]
fn a_request_is_applied_to_the_map_alone() {
    let kept = Kept::from([("a".to_owned(), b"1".to_vec())]);
    let (answer, changed) = applied(kept.clone(), &set("b", Some(b"2")));
    assert_eq!(answer, methods::encode(&()));
    let both = Kept::from([
        ("a".to_owned(), b"1".to_vec()),
        ("b".to_owned(), b"2".to_vec()),
    ]);
    assert_eq!(changed, Some(both));
    assert_eq!(applied(kept.clone(), &set("a", None)).1, Some(Kept::new()));
    let (answer, changed) = applied(kept.clone(), &Request::Get("a".into()));
    assert_eq!(
        (answer, changed),
        (methods::encode(&Some(b"1".to_vec())), None)
    );
    let (answer, changed) = applied(kept, &Request::List(String::new()));
    assert_eq!(
        (answer, changed),
        (methods::encode(&vec!["a".to_owned()]), None)
    );
}

/// Each view on each network has its own file, and a key kept by one is
/// absent to the others; a name never climbs out of the store.
#[test]
fn views_and_chains_keep_apart() {
    let config = scratch();
    let chat = path(&config, "dev#01", "chat");
    let forge = path(&config, "dev#01", "forge");
    let other = path(&config, "dev#02", "chat");
    put(&chat, "reads", Some(b"mine")).unwrap();
    assert_eq!(read(&chat, "reads").as_deref(), Some(&b"mine"[..]));
    assert_eq!(read(&forge, "reads"), None);
    assert_eq!(read(&other, "reads"), None);
    assert_ne!(path(&config, "a#b", "chat"), path(&config, "a_23b", "chat"));
    let sly = path(&config, "../..", "../x");
    assert!(sly.starts_with(&config));
    assert_eq!(sly.components().count(), config.components().count() + 2);
    let _ = std::fs::remove_dir_all(config);
}

/// Each seated key has its own file beside the device's, so two accounts
/// signed in on one device keep apart; with no key the device's own is
/// the file; a key never climbs out of its network's folder.
#[test]
fn keys_keep_apart() {
    let config = scratch();
    let alice = located(&config, "dev#01", "aa01", "chat");
    let bob = located(&config, "dev#01", "bb02", "chat");
    let none = device(&config, "dev#01", "chat");
    assert_eq!(none.path, none.device);
    assert_eq!((&alice.device, &bob.device), (&none.device, &none.device));
    assert_ne!(alice.path, bob.path);
    put(&alice.path, "rooms/collapsed", Some(b"work")).unwrap();
    assert_eq!(read(&bob.path, "rooms/collapsed"), None);
    assert_eq!(read(&none.path, "rooms/collapsed"), None);
    let sly = located(&config, "dev#01", "../..", "chat");
    assert!(sly.path.starts_with(config.join("dev_2301")));
    assert_eq!(
        sly.path.components().count(),
        config.components().count() + 3
    );
    let _ = std::fs::remove_dir_all(config);
}

/// A key's first requests read its own file, never the device's: a `get`
/// and a `list` asked through `ask` with a key seated find nothing the
/// device's file keeps (another key's cursors among it), and nothing is
/// copied into the key's file.
#[test]
fn a_keys_first_requests_read_nothing_of_the_device_file() {
    let config = scratch();
    let none = device(&config, "dev#01", "chat");
    let alice = located(&config, "dev#01", "aa01", "chat");
    put(&none.path, "reads/bb02", Some(b"7")).unwrap();
    let mut view = crate::runtime::kernel::tests::view();
    ask(
        &mut view,
        1,
        alice.clone(),
        Request::Get("reads/bb02".into()),
    );
    ask(&mut view, 2, alice.clone(), Request::List(String::new()));
    let answered = replies(&mut view, |answered| answered.len() >= 2);
    let mut answers: Vec<(u64, Vec<u8>)> = answered
        .into_iter()
        .filter_map(|event| match event {
            wire::Event::Response { id, result, .. } => Some((id, result.unwrap())),
            _ => None,
        })
        .collect();
    answers.sort();
    let nothing = methods::encode(&None::<Vec<u8>>);
    let no_keys = methods::encode(&Vec::<String>::new());
    assert_eq!(answers, [(1, nothing), (2, no_keys)]);
    assert!(
        !alice.path.exists(),
        "nothing is copied into the key's file"
    );
    assert_eq!(read(&none.path, "reads/bb02").as_deref(), Some(&b"7"[..]));
    let _ = std::fs::remove_dir_all(config);
}

/// `list` answers the keys under a prefix in byte order, every key for an
/// empty one, and none for a file not there.
#[test]
fn list_answers_the_keys_under_a_prefix() {
    let config = scratch();
    let file = path(&config, "dev#01", "chat");
    assert!(listed(&file, "").is_empty());
    for key in ["rooms/work", "emoji", "rooms/all", "roomsy"] {
        put(&file, key, Some(b"1")).unwrap();
    }
    assert_eq!(listed(&file, "rooms/"), ["rooms/all", "rooms/work"]);
    assert_eq!(
        listed(&file, ""),
        ["emoji", "rooms/all", "rooms/work", "roomsy"]
    );
    assert!(listed(&file, "zzz").is_empty());
    let _ = std::fs::remove_dir_all(config);
}

/// `None` drops a key; what was set is read back from disk by a fresh
/// read, as after a relaunch.
#[test]
fn keys_are_dropped_and_kept_across_a_reopen() {
    let config = scratch();
    let file = path(&config, "dev#01", "chat");
    put(&file, "a", Some(b"1")).unwrap();
    put(&file, "b", Some(b"2")).unwrap();
    put(&file, "a", None).unwrap();
    let reopened = load(&file).unwrap();
    assert_eq!(reopened, Kept::from([("b".to_owned(), b"2".to_vec())]));
    assert_eq!(read(&file, "a"), None);
    assert_eq!(
        std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
        1
    );
    let _ = std::fs::remove_dir_all(config);
}

/// A `set` and a `get` behind it, asked back to back: neither is
/// answered on the thread that asked (the window thread, in a redraw),
/// and the `get` reads what the `set` kept.
#[test]
fn a_get_behind_a_set_reads_it_off_the_window_thread() {
    let config = scratch();
    let file = device(&config, "dev#01", "chat");
    let mut view = crate::runtime::kernel::tests::view();
    for nth in 0..20u64 {
        let value = nth.to_string().into_bytes();
        let kept = Request::Set("reads".into(), Some(value));
        ask(&mut view, 2 * nth, file.clone(), kept);
        let asked = Request::Get("reads".into());
        ask(&mut view, 2 * nth + 1, file.clone(), asked);
    }
    assert!(
        view.pending.is_empty(),
        "answered on the window thread: {:?}",
        view.pending
    );
    let answered = replies(&mut view, |answered| answered.len() >= 40);
    let mut gets: Vec<(u64, Option<Vec<u8>>)> = answered
        .into_iter()
        .filter_map(|event| match event {
            wire::Event::Response { id, result, .. } if id % 2 == 1 => {
                Some((id, methods::decode(&result.unwrap()).unwrap()))
            }
            _ => None,
        })
        .collect();
    gets.sort();
    let want: Vec<_> = (0..20u64)
        .map(|nth| (2 * nth + 1, Some(nth.to_string().into_bytes())))
        .collect();
    assert_eq!(gets, want, "each get reads the set just before it");
    let _ = std::fs::remove_dir_all(config);
}

/// A view's request reads the file of the key its props name: a `get` with
/// a key seated reads that key's file, never the device's.
#[test]
fn a_request_reads_the_file_of_the_key_its_props_name() {
    let mut view = crate::runtime::kernel::tests::view();
    view.connection.chain = "dev#01".into();
    let alice = file(&view, &seated("aa01")).unwrap();
    assert_eq!(alice.signer, "aa01");
    assert_ne!(alice.path, alice.device);
    let none = file(&view, &seated("")).unwrap();
    assert_eq!(none.path, none.device);
    assert_eq!(file(&view, &None).unwrap(), none);
}

/// A `set` that kept its value is told to every other instance of the
/// program listening on its file (`store.changes`), after the value is
/// in it; the instance that set it hears nothing of its own.
#[test]
fn a_set_is_told_to_the_programs_other_instances() {
    let config = scratch();
    let file = device(&config, "dev#01", "chat");
    let mut pane = crate::runtime::kernel::tests::view();
    let mut notifier = crate::runtime::kernel::tests::view();
    listen(&mut pane, 1, file.clone());
    listen(&mut notifier, 1, file.clone());
    ask(&mut pane, 2, file.clone(), set("reads", Some(b"3")));
    let heard = replies(&mut notifier, |heard| !heard.is_empty());
    let [
        wire::Event::Response {
            id: 1,
            result: Ok(key),
            done: false,
        },
    ] = heard.as_slice()
    else {
        panic!("{heard:?}");
    };
    assert_eq!(methods::decode::<String>(key).unwrap(), "reads");
    assert_eq!(load(&file.path).unwrap()["reads"], b"3", "told once kept");
    let mut own = Vec::new();
    pane.replies.drain_into(&mut own);
    assert!(
        own.iter()
            .all(|event| !matches!(event, wire::Event::Response { id: 1, .. })),
        "the writer heard its own set: {own:?}"
    );
    let _ = std::fs::remove_dir_all(config);
}

/// A set in one key's file is told only to the instances running with that
/// key; one whose props move to the key hears its sets from then on.
#[test]
fn a_set_is_told_only_to_the_same_keys_instances() {
    let config = scratch();
    let alice = located(&config, "dev#01", "aa01", "chat");
    let bob = located(&config, "dev#01", "bb02", "chat");
    let mut pane = crate::runtime::kernel::tests::view();
    let mut notifier = crate::runtime::kernel::tests::view();
    let mut other = crate::runtime::kernel::tests::view();
    listen(&mut pane, 1, alice.clone());
    listen(&mut notifier, 1, alice.clone());
    listen(&mut other, 1, bob.clone());
    let set_reads = |pane: &mut View, id: u64, value: &[u8]| {
        ask(pane, id, alice.clone(), set("reads", Some(value)));
    };
    set_reads(&mut pane, 2, b"1");
    let heard = replies(&mut notifier, |heard| !heard_keys(heard, 1).is_empty());
    assert_eq!(heard_keys(&heard, 1), ["reads"]);
    let mut theirs = Vec::new();
    other.replies.drain_into(&mut theirs);
    assert!(heard_keys(&theirs, 1).is_empty(), "{theirs:?}");

    follow(&other, &seated("aa01"));
    set_reads(&mut pane, 3, b"2");
    let heard = replies(&mut other, |heard| !heard_keys(heard, 1).is_empty());
    assert_eq!(heard_keys(&heard, 1), ["reads"]);
    let _ = std::fs::remove_dir_all(config);
}

/// A `set` a view sent right before the app quits is on disk once the
/// quit drain is done, though it was still queued when the drain began.
#[test]
fn the_quit_drain_waits_for_a_queued_set() {
    let config = scratch();
    let file = device(&config, "dev#01", "chat");
    let mut view = crate::runtime::kernel::tests::view();
    let release = crate::runtime::kernel::tests::held(&file.path.to_string_lossy());
    ask(&mut view, 0, file.clone(), set("last", Some(b"word")));
    assert!(!file.path.exists(), "the set waits behind the held job");

    let mut drained = std::pin::pin!(crate::runtime::kernel::drain());
    assert!(
        crate::runtime::kernel::tests::waiting(drained.as_mut()),
        "the drain is done while the set is still queued"
    );
    release.send(()).unwrap();
    futures::executor::block_on(drained);
    assert_eq!(
        load(&file.path).unwrap(),
        Kept::from([("last".to_owned(), b"word".to_vec())])
    );
    let _ = std::fs::remove_dir_all(config);
}

/// A followed set's notifier keeps its file on its own chain while the
/// set runs, whatever the views' connection did since; once the set left
/// (its chain taken up in hand) it is refused, so the run in hand is the
/// file's one writer.
#[test]
fn a_left_set_writes_no_store() {
    let mut view = crate::runtime::kernel::tests::view();
    let apart = std::sync::Arc::new(super::super::roster::Apart::default());
    view.connection = super::super::Connection {
        chain: "other-network#1".into(),
        rev: view.connection.rev.wrapping_sub(1),
        apart: Some(apart.clone()),
        ..Default::default()
    };
    assert!(file(&view, &None).is_ok(), "{:?}", file(&view, &None));
    apart.left.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        file(&view, &None).unwrap_err().code,
        methods::refusal::STALE_CONNECTION
    );
}

/// A store file cut to nothing answers `None` to a view, not a host
/// fault: it is set aside as `.bad`, and the next `set` lands.
#[test]
fn a_cut_store_file_reads_as_empty_and_is_set_aside() {
    let config = scratch();
    let file = path(&config, "dev#01", "chat");
    put(&file, "a", Some(b"1")).unwrap();
    std::fs::write(&file, b"").unwrap();
    assert_eq!(read(&file, "a"), None);
    assert!(file.with_extension("borsh.bad").exists());
    assert!(!file.exists());
    put(&file, "a", Some(b"2")).unwrap();
    assert_eq!(read(&file, "a").as_deref(), Some(&b"2"[..]));
    let _ = std::fs::remove_dir_all(config);
}
