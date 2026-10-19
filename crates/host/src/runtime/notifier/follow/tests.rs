//! Followed sets against a node of their own: one run per chain, the log
//! traded with the centre in hand, the lock, a restart after a blip.
use super::super::tests::{allow, blob_carrying, mention, notifier_posting, rows, serial};
use super::*;
use crate::runtime::seat::tests::{FakeNode, blob_id, eventually};

/// A node of its own on `network` listing `program` with `blob` as its
/// code, the chain it serves, and a set following it.
fn followed(network: &str, program: &str, blob: &[u8]) -> (FakeNode, String, Followed) {
    let entry = ::programs::Entry {
        program: program.into(),
        code: blob_id(blob),
        params: Vec::new(),
    };
    let node = FakeNode::on(network, vec![entry], vec![blob.to_vec()]);
    let client = node.asked_of.client.clone().unwrap();
    let chain = chain_of(&handle().block_on(client.status()).unwrap());
    let followed = handle()
        .block_on(follow(client.endpoint().to_owned()))
        .expect("followed");
    (node, chain, followed)
}

/// `program`'s rows in `center`, and how many notices each folds.
fn counted(center: &CenterHandle, program: &str) -> Vec<u32> {
    let center = center.lock();
    let rows = center.entries().filter(|entry| entry.program == program);
    rows.map(|entry| entry.count).collect()
}

/// Settings that let every program's banner through.
fn allowing() -> Settings {
    Settings {
        banners: true,
        in_front: false,
        burst: 6,
        views: Default::default(),
        unasked: Some(Permission::Allow),
    }
}

/// Another listed network's notifier runs on that network's own node, into
/// that chain's own log and out the OS's banners under its chain, never the
/// app's centre; a connect of the network in hand stops the person's
/// notifiers there and leaves the set's running.
#[test]
fn a_set_answers_on_its_own_node_and_a_connect_leaves_it_running() {
    let _serial = serial();
    let program = "notifier-tests-followed";
    allow(program);
    let blob = blob_carrying(&notifier_posting("Followed", "notify,", &mention()));
    let (_node, chain, followed) = followed("followed-net", program, &blob);
    let set = &followed.0;
    let center = set.recipient().center.clone();
    eventually("its notifier posted", || counted(&center, program) == [1]);
    assert_eq!(center.lock().network(), chain);
    // the banner goes out on its queue after the row is logged
    handle().block_on(crate::runtime::kernel::drain());
    let tag = format!("{program}/#general");
    let banner = crate::contract::testing::banners_asked()
        .into_iter()
        .find(|banner| banner.tag == tag)
        .expect("its banner");
    assert_eq!(banner.chain, chain);
    assert!(rows(program).is_empty(), "the app's centre heard of it");

    stop_every();
    assert_eq!(set.notifiers.running(), [program]);
    assert!(!lock(&set.connection).stale());
}

/// One run per chain: no set follows the chain in hand, nor a chain a set
/// follows already; dropped, a set lets its chain be followed again.
#[test]
fn follow_refuses_the_chain_in_hand_and_a_second_set() {
    let _serial = serial();
    let node = FakeNode::on("refused-net", Vec::new(), Vec::new());
    let client = node.asked_of.client.clone().unwrap();
    let url = client.endpoint().to_owned();
    let chain = chain_of(&handle().block_on(client.status()).unwrap());
    let center = CenterHandle::default();
    take_up(&center, &chain);
    assert!(handle().block_on(follow(url.clone())).is_none(), "in hand");
    take_up(&center, "");
    let first = handle().block_on(follow(url.clone())).expect("out of hand");
    assert!(handle().block_on(follow(url.clone())).is_none(), "a second");
    assert_eq!(followed_url(&chain).as_deref(), Some(&url[..]));
    drop(first);
    assert_eq!(followed_url(&chain), None);
    assert!(handle().block_on(follow(url)).is_some(), "followed again");
}

/// Taking a followed chain up in hand trades logs in memory: its set's rows
/// come over to the app's centre, the chain left parks its own (taken up
/// again with no file read), the set stops, a late post of its notifier is
/// refused, and a late flush of its centre writes nothing over the file.
#[test]
fn take_up_trades_logs_and_refuses_the_left_sets_post() {
    let _serial = serial();
    let program = "notifier-tests-traded";
    allow(program);
    let blob = blob_carrying(&notifier_posting("Traded", "notify,", &mention()));
    let (_node, chain, followed) = followed("traded-net", program, &blob);
    let set = followed.0.clone();
    let source = set.recipient().center.clone();
    eventually("the set posted", || counted(&source, program) == [1]);
    let center = CenterHandle::default();
    let before = "traded-before#1";
    take_up(&center, before);
    let post = wire::methods::Notification {
        title: "before".into(),
        ..mention()
    };
    let now = std::time::Instant::now();
    center
        .lock()
        .post(&allowing(), "chat", "Chat", post, now, notify::wall());

    take_up(&center, &chain);
    assert_eq!(center.lock().network(), chain);
    assert_eq!(counted(&center, program), [1], "the set's row came over");
    assert_eq!(source.lock().network(), "");
    assert!(set.apart.left.load(std::sync::atomic::Ordering::SeqCst));
    assert!(set.notifiers.running().is_empty(), "the set runs on");

    let mut view = super::super::kernel::tests::view();
    view.program = program;
    view.connection = lock(&set.connection).clone();
    view.recipient = set.notifiers.recipient.clone();
    let late = wire::methods::encode(&mention());
    assert!(notify::answer(
        &mut view,
        Capability::Notify,
        "post",
        9,
        &late
    ));
    match view.pending.pop() {
        Some(wire::Event::Response {
            id: 9,
            result: Err(refusal),
            ..
        }) => assert_eq!(refusal.code, refusal::STALE_CONNECTION),
        other => panic!("{other:?}"),
    }
    let file = notify::log_file(&chain).unwrap();
    handle().block_on(super::super::kernel::drain());
    let _ = std::fs::remove_file(&file);
    // whatever moves it now (here, a mark read), it has no file to write
    source.lock().mark_all_read();
    handle().block_on(super::super::kernel::drain());
    assert!(!file.exists(), "the left set wrote over its chain's log");

    let parked = notify::log_file(before).unwrap();
    std::fs::remove_file(&parked).unwrap();
    take_up(&center, before);
    assert_eq!(counted(&center, "chat"), [1], "the parked log came back");
    take_up(&center, "");
}

/// A Block on a program stops its notifier on every network, a followed
/// one's too; let through again, it starts there on the set's own roster.
#[test]
fn a_block_stops_a_followed_notifier() {
    let _serial = serial();
    let program = "notifier-tests-blocked-followed";
    allow(program);
    let blob = blob_carrying(&notifier_posting("Blocked", "notify,", &mention()));
    let (_node, _, followed) = followed("blocked-net", program, &blob);
    assert_eq!(followed.0.notifiers.running(), [program]);
    crate::backend::edit_prefs(|prefs| {
        prefs["notify_views"] = serde_json::json!({ program: "Block" });
    });
    Settings::load();
    permission_moved(program, Permission::Block);
    assert!(followed.0.notifiers.running().is_empty());
    allow(program);
    Settings::load();
    permission_moved(program, Permission::Allow);
    assert_eq!(followed.0.notifiers.running(), [program]);
}

/// A followed node that answers again after it did not (a blip, a laptop
/// asleep) starts its set's notifiers over on it, as a connect does in
/// hand: each catches up again, where a feed that closed meanwhile would
/// have left it deaf.
#[test]
fn a_set_restarts_its_notifiers_when_its_node_answers_again() {
    let _serial = serial();
    let program = "notifier-tests-restarted";
    allow(program);
    let blob = blob_carrying(&notifier_posting("Restarted", "notify,", &mention()));
    let (_node, _, followed) = followed("restarted-net", program, &blob);
    let set = &followed.0;
    let center = set.recipient().center.clone();
    eventually("its notifier posted", || counted(&center, program) == [1]);
    let mut heard = Heard {
        height: 0,
        lost: false,
    };
    assert!(!set.polled(&mut heard, Some(0)), "nothing moved");
    assert!(!set.polled(&mut heard, None));
    assert!(set.polled(&mut heard, Some(0)), "back after none");
    handle().block_on(set.read());
    eventually("started over, it posted again", || {
        counted(&center, program) == [2]
    });
}

/// A followed set keeps its notifiers' files where the run in hand keeps
/// them, so a switch finds each one's cursor (no notice posted twice), and
/// its banners queue as the run in hand's do.
#[test]
fn a_switch_finds_the_sets_cursor() {
    let _serial = serial();
    let followed = follow_for_test("http://127.0.0.1:9", "cursor-net#1", "", None).unwrap();
    let recipient = followed.0.recipient();
    assert_eq!(recipient.store, super::super::store::store_of(None));
    assert_eq!(
        notify::queue_of("chat", Some(recipient)),
        notify::queue_of("chat", None)
    );
}

/// A call's banner raised on a chain, then that chain left for another
/// and followed: the timer at its `until` asks the centre holding the
/// chain's log then (the set's), not the one it was posted into (now
/// another chain's), and takes the banner down.
#[test]
fn a_banners_until_fires_on_the_chains_centre_after_a_switch() {
    let _serial = serial();
    let center = CenterHandle::default();
    let chain = "until-net#1";
    take_up(&center, chain);
    let until = notify::wall() as u64 + 2;
    let call = wire::methods::Notification {
        kind: notify::NoticeKind::Call,
        until,
        ..mention()
    };
    let now = std::time::Instant::now();
    let (_, banner) = center
        .lock()
        .post(&allowing(), "chat", "Chat", call, now, 0);
    let banner = banner.expect("it rings");
    let under = (banner.chain.clone(), banner.tag.clone());
    let timer = notify::withdraw_at(center.clone(), None, "chat", under.clone(), until);
    take_up(&center, "until-other#2");
    let followed = follow_for_test("http://127.0.0.1:9", chain, "", None).unwrap();
    handle().block_on(timer).unwrap();
    handle().block_on(super::super::kernel::drain());
    assert!(
        crate::contract::testing::banners_withdrawn().contains(&under),
        "the banner stands"
    );
    let set = followed.0.recipient().center.clone();
    assert!(!set.lock().ring_over(&banner.tag, until), "taken off");
    take_up(&center, "");
}

/// A set followed while the app is locked hands its notifiers no key and
/// no account from its first turn, not once the follower gets to it; an
/// unlock hands them back.
#[test]
fn a_set_followed_while_locked_starts_with_no_key() {
    let _serial = serial();
    lock_followed(true);
    let followed = follow_for_test("http://127.0.0.1:9", "locked-net#1", "ab", Some(7)).unwrap();
    let handed = followed.handed();
    lock_followed(false);
    assert_eq!(handed, (String::new(), None), "it started with the key");
    assert_eq!(followed.handed(), ("ab".into(), Some(7)));
}

/// A set's log comes off its file on the blocking pool, never on the
/// thread the follow runs on (in the app, the views' one runtime thread),
/// and so never under the registry's lock: that thread asks for no block
/// anywhere near the log's size.
#[test]
fn a_set_reads_its_log_off_the_following_thread() {
    use super::super::tests::largest;
    let _serial = serial();
    let node = FakeNode::on("log-read-net", Vec::new(), Vec::new());
    let client = node.asked_of.client.clone().unwrap();
    let chain = chain_of(&handle().block_on(client.status()).unwrap());
    let file = notify::log_file(&chain).unwrap();
    let row = notify::Entry {
        id: 1,
        program: "chat".into(),
        view: "Chat".into(),
        title: "ada mentioned you".into(),
        body: "x".repeat(1 << 20),
        tag: String::new(),
        link: String::new(),
        at: 0,
        count: 1,
        read: false,
    };
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, serde_json::to_vec(&[row]).unwrap()).unwrap();
    let url = client.endpoint().to_owned();
    let (followed, largest) = largest::watching(|| handle().block_on(follow(url)));
    let followed = followed.expect("followed");
    assert_eq!(counted(&followed.center(), "chat"), [1], "its log unread");
    drop(followed);
    let _ = std::fs::remove_file(&file);
    assert!(
        largest < 64 << 10,
        "the following thread asked for {largest} bytes at once"
    );
}
