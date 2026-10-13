use std::time::{Duration, Instant};

use inbox_api::{Entry, Push, Read};
use notices::{Kind, Link};

use super::center::{Center, CenterHandle, InboxState};
use super::settings::{BURST_PREF, FRONT_PREF, NOTIFY_PREF, PROGRAMS_PREF, Permission, Settings};
use crate::backend::inbox::fake::FakeInbox;
use crate::runtime::WindowKey;

pub(crate) fn entry(id: i64, program: &str, tag: &str, height: u64) -> Entry {
    Entry {
        id,
        program: program.into(),
        key: format!("{tag}/{id}"),
        kind: Kind::Mention,
        from: Some(1),
        title: format!("title {id}"),
        body: "b".into(),
        tag: tag.into(),
        link: Link {
            program: program.into(),
            segments: vec![tag.into(), id.to_string()],
        },
        height,
        tx: "ab".into(),
        path: "/0#0".into(),
        at_ms: 1_790_000_000_000 + id as u64 * 1000,
        read: false,
    }
}

fn settings(permission: Option<Permission>) -> Settings {
    Settings {
        banners: true,
        in_front: false,
        burst: 3,
        programs: permission
            .map(|permission| ("chat".to_owned(), permission))
            .into_iter()
            .collect(),
    }
}

fn ready() -> Center {
    let mut center = Center::default();
    center.set_network("net#0a0b0c0d");
    center.set_state(InboxState::Ready);
    center
}

/// Rows fold by `(program, tag)` with a count and the newest height;
/// another program's same tag is its own row, and an entry with no tag
/// is never folded.
#[test]
fn rows_fold_by_program_and_tag() {
    let mut center = ready();
    center.replace(
        vec![
            entry(5, "chat", "design", 50),
            entry(4, "forge", "design", 40),
            entry(3, "chat", "", 30),
            entry(2, "chat", "", 20),
            entry(1, "chat", "design", 10),
        ],
        5,
    );
    let rows = center.rows();
    let folded: Vec<_> = rows
        .iter()
        .map(|row| (row.id, row.program.as_str(), row.count, row.through_height))
        .collect();
    assert_eq!(
        folded,
        vec![
            (5, "chat", 2, 50),
            (4, "forge", 1, 40),
            (3, "chat", 1, 30),
            (2, "chat", 1, 20)
        ]
    );
    assert_eq!(rows[0].title, "title 5", "the newest entry names the row");
    assert_eq!(center.unread(), 5);
}

/// The person's word decides first: none yet counts and asks in the
/// centre, Silent counts, Allow raises a banner tagged by the program.
#[test]
fn a_programs_permission_decides_ask_count_or_banner() {
    let now = Instant::now();
    let mut center = ready();
    assert!(
        center
            .arrived(&settings(None), entry(1, "chat", "", 1), now, "Chat")
            .is_none()
    );
    assert!(center.asking.contains("chat") && center.rows().len() == 1);
    assert!(
        center
            .arrived(
                &settings(Some(Permission::Silent)),
                entry(2, "chat", "", 2),
                now,
                "Chat"
            )
            .is_none()
    );
    let banner = center
        .arrived(
            &settings(Some(Permission::Allow)),
            entry(3, "chat", "room", 3),
            now,
            "Chat",
        )
        .unwrap();
    assert_eq!(
        (banner.title.as_str(), banner.tag.as_str()),
        ("title 3", "entry/chat/room")
    );
    assert_eq!(center.unread(), 3);
    let mut off = settings(Some(Permission::Allow));
    off.banners = false;
    assert!(
        center
            .arrived(&off, entry(4, "chat", "", 4), now, "Chat")
            .is_none()
    );
    // the same entry pushed twice counts once
    assert!(
        center
            .arrived(&off, entry(4, "chat", "", 4), now, "Chat")
            .is_none()
    );
    assert_eq!(center.unread(), 4);
}

/// The window in front gets no banner unless the person asked for it.
#[test]
fn the_focused_view_is_not_bannered_unless_show_is_set() {
    let now = Instant::now();
    let mut center = ready();
    let window = WindowKey::unique();
    let mut allowed = settings(Some(Permission::Allow));
    center.set_front(window, true, "chat");
    assert!(
        center
            .arrived(&allowed, entry(1, "chat", "", 1), now, "Chat")
            .is_none()
    );
    allowed.in_front = true;
    assert!(
        center
            .arrived(&allowed, entry(2, "chat", "", 2), now, "Chat")
            .is_some()
    );
    allowed.in_front = false;
    center.set_front(WindowKey::unique(), false, "");
    assert!(center.front.is_some());
    center.set_front(window, false, "");
    assert!(
        center
            .arrived(&allowed, entry(3, "chat", "", 3), now, "Chat")
            .is_some()
    );
    // a notice whose link opens the view in front is in front too
    center.set_front(window, true, "forge");
    let mut on_forge = entry(4, "chat", "", 4);
    on_forge.link.program = "forge".into();
    assert!(center.arrived(&allowed, on_forge, now, "Chat").is_none());
}

/// A burst of three a minute: the fourth is counted and raises one "more"
/// banner keyed by the program, counting up; a token back, a banner again.
#[test]
fn a_burst_past_the_limit_coalesces_into_one_more_banner() {
    let start = Instant::now();
    let mut center = ready();
    let allowed = settings(Some(Permission::Allow));
    for id in 1..=3 {
        let banner = center
            .arrived(&allowed, entry(id, "chat", "", 1), start, "Chat")
            .unwrap();
        assert_eq!(banner.title, format!("title {id}"));
    }
    let more = center
        .arrived(&allowed, entry(4, "chat", "", 1), start, "Chat")
        .unwrap();
    assert_eq!(
        (more.title.as_str(), more.tag.as_str(), more.opens),
        ("1 more from Chat", "more/chat", None)
    );
    // a program named `more` is told apart from the "more" banner by the
    // field, not by its tag
    let mut allowed_more = allowed.clone();
    allowed_more
        .programs
        .insert("more".into(), Permission::Allow);
    let own = center
        .arrived(&allowed_more, entry(40, "more", "chat", 1), start, "More")
        .unwrap();
    assert_eq!((own.tag.as_str(), own.opens), ("entry/more/chat", Some(40)));
    assert_ne!(own.tag, more.tag, "no pair can forge the host's tag");
    let more = center
        .arrived(&allowed, entry(5, "chat", "", 1), start, "Chat")
        .unwrap();
    assert_eq!(more.title, "2 more from Chat");
    let later = start + Duration::from_secs(21);
    let banner = center
        .arrived(&allowed, entry(6, "chat", "", 1), later, "Chat")
        .unwrap();
    assert_eq!(banner.title, "title 6");
    assert_eq!(center.unread(), 7, "the six, and `more`'s one");
}

/// A `gone` takes the entry off the page and its count down; a `read`
/// elsewhere does the same for the count, and says when it names an
/// entry this page does not have.
#[test]
fn gone_and_read_elsewhere_move_the_page() {
    let mut center = ready();
    center.replace(
        vec![entry(2, "chat", "room", 2), entry(1, "chat", "room", 1)],
        2,
    );
    center.removed(&[2]);
    assert_eq!(
        (center.rows().len(), center.rows()[0].count, center.unread()),
        (1, 1, 1)
    );
    assert!(!center.read_elsewhere(&[1]));
    assert_eq!(center.unread(), 0);
    assert!(center.rows()[0].read);
    assert!(center.read_elsewhere(&[9]), "an id off the page");
}

/// A banner stands for one entry: it comes down when that entry is read
/// on this device (a row opened, Mark all read) or elsewhere, or gone;
/// an older entry under the same tag going takes no newer banner down.
#[test]
fn a_banner_stands_for_its_entry_until_that_entry_is_read_or_gone() {
    let now = Instant::now();
    let mut center = ready();
    let allowed = settings(Some(Permission::Allow));
    assert!(
        center
            .arrived(&allowed, entry(1, "chat", "room", 1), now, "Chat")
            .is_some()
    );
    assert!(
        center
            .arrived(&allowed, entry(2, "chat", "room", 2), now, "Chat")
            .is_some()
    );
    assert_eq!(center.standing.get("entry/chat/room"), Some(&2));
    // the older entry hidden elsewhere: the newer one's banner stands
    center.removed(&[1]);
    assert_eq!(center.standing.get("entry/chat/room"), Some(&2));
    // read on this device: down
    center.open(2);
    assert!(center.standing.is_empty());
    assert!(
        center
            .arrived(&allowed, entry(3, "chat", "room", 3), now, "Chat")
            .is_some()
    );
    center.mark_all_read();
    assert!(center.standing.is_empty(), "Mark all read took it down");
    assert!(
        center
            .arrived(&allowed, entry(5, "chat", "room", 5), now, "Chat")
            .is_some()
    );
    center.read_elsewhere(&[5]);
    assert!(center.standing.is_empty(), "read elsewhere took it down");
    // gone takes it down too
    assert!(
        center
            .arrived(&allowed, entry(6, "chat", "room", 6), now, "Chat")
            .is_some()
    );
    center.removed(&[6]);
    assert!(center.standing.is_empty());
}

/// Opening a row reads its entries through the row's newest height and
/// mints the notice's link for the chain in hand; a link that does not
/// mint opens nothing.
#[test]
fn opening_a_row_reads_through_its_newest_height_and_mints_the_link() {
    let mut center = ready();
    center.replace(
        vec![entry(2, "chat", "room", 20), entry(1, "chat", "room", 10)],
        2,
    );
    let opened = center.open(2).unwrap();
    assert!(opened.link.is_some());
    assert_eq!(
        opened.link,
        ducklink::mint("net#0a0b0c0d", "chat", &["room", "2"]),
    );
    assert!(center.rows()[0].read);
    assert_eq!(center.unread(), 0);
    assert_eq!(center.open(9), None);
    let mut bad = entry(3, "chat", "", 30);
    bad.link.segments = vec!["a/b".into()];
    center.replace(vec![bad], 1);
    assert_eq!(
        center.open(3).unwrap().link,
        None,
        "a link that does not mint"
    );
}

/// An untagged row is one entry: opening it reads that entry alone (by
/// id, kept as a call for the connect), not the program's older untagged
/// entries, and keeps no watermark.
#[test]
fn opening_an_untagged_row_reads_that_entry_alone() {
    let mut center = ready();
    center.replace(vec![entry(2, "chat", "", 20), entry(1, "chat", "", 10)], 2);
    center.set_state(InboxState::Off);
    center.open(2);
    let rows = center.rows();
    assert_eq!((rows[0].read, rows[1].read), (true, false));
    assert_eq!(
        center.pending_for_test(),
        1,
        "the read by id waits for the connect"
    );
    assert!(
        center.watermark("chat", "", 20),
        "no watermark was kept at 20"
    );
}

/// Banners the centre stops tracking come down with it: the previous
/// account's at a key lock (`forget_account`), and those of entries a
/// re-read page no longer lists (a mute from another device).
#[test]
fn a_banner_the_centre_stops_tracking_comes_down() {
    let now = Instant::now();
    let mut center = ready();
    let allowed = settings(Some(Permission::Allow));
    let withdrawn = || super::withdrawn().lock().unwrap().clone();
    center.arrived(&allowed, entry(1, "chat", "forget-a", 1), now, "Chat");
    center.arrived(&allowed, entry(2, "chat", "forget-b", 2), now, "Chat");
    center.replace(vec![entry(2, "chat", "forget-b", 2)], 1);
    assert!(withdrawn().contains(&"entry/chat/forget-a".to_owned()));
    assert!(!withdrawn().contains(&"entry/chat/forget-b".to_owned()));
    center.forget_account();
    assert!(withdrawn().contains(&"entry/chat/forget-b".to_owned()));
    assert!(center.standing.is_empty());
}

/// An untagged entry's banner is tracked under its own tag and comes
/// down with its entry; the "N more" banners go with the account.
#[test]
fn an_untagged_banner_and_the_more_banner_come_down_too() {
    let now = Instant::now();
    let mut center = ready();
    let allowed = settings(Some(Permission::Allow));
    let withdrawn = || super::withdrawn().lock().unwrap().clone();
    let own = center
        .arrived(&allowed, entry(71, "chat", "", 1), now, "Chat")
        .unwrap();
    assert_eq!(own.tag, "id/71");
    assert_eq!(center.standing.get("id/71"), Some(&71));
    center.read_elsewhere(&[71]);
    assert!(withdrawn().contains(&"id/71".to_owned()));
    for id in 72..=76 {
        center.arrived(&allowed, entry(id, "chat", "", 1), now, "Chat");
    }
    center.forget_account();
    assert!(withdrawn().contains(&"more/chat".to_owned()));
}

/// The runner's writes are checked against its stamp under the one lock
/// that writes: a dropped runner's end and its page land nowhere.
#[tokio::test]
async fn a_dropped_runners_writes_land_nowhere() {
    let handle = CenterHandle::default();
    handle.lock().set_network("net#0");
    let stale = handle.lock().claim();
    let live = handle.lock().claim();
    let fake = FakeInbox::start().await;
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let client = std::sync::Arc::new(crate::backend::inbox::Client::new(&rpc, "net#0", "d"));
    let page = || inbox_api::Entries {
        entries: vec![entry(1, "chat", "room", 1)],
        unread: 1,
    };
    let status = || inbox_api::Status {
        chain: "net#0".into(),
        indexed_height: 1,
        tip: 1,
        stopped: None,
        stalled: None,
    };
    let lost = std::sync::Arc::new(tokio::sync::Notify::new());
    assert!(!super::follow::fill(
        &handle,
        stale,
        &client,
        &lost,
        status(),
        Default::default(),
        page()
    ));
    assert!(handle.lock().rows().is_empty());
    assert_eq!(handle.lock().state, InboxState::Off);
    assert!(!super::follow::settle(
        &handle,
        stale,
        &Err(crate::backend::inbox::Error::NoInbox)
    ));
    assert_eq!(handle.lock().state, InboxState::Off);
    assert!(super::follow::fill(
        &handle,
        live,
        &client,
        &lost,
        status(),
        Default::default(),
        page()
    ));
    assert_eq!(handle.lock().rows().len(), 1);
    assert_eq!(handle.lock().state, InboxState::Ready);
    assert!(super::follow::settle(
        &handle,
        live,
        &Err(crate::backend::inbox::Error::Unreachable("x".into()))
    ));
    assert!(matches!(
        handle.lock().state,
        InboxState::Unreachable { .. }
    ));
    assert!(!super::follow::settle(
        &handle,
        live,
        &Err(crate::backend::inbox::Error::NoInbox)
    ));
    assert_eq!(handle.lock().state, InboxState::NoInbox);
}

/// A view's watermark marks the tag's rows at or below it read and moves
/// once: the same or a lower height is not a move, a higher one is. The
/// count follows the watermark wherever it is applied: a page listed
/// under a kept watermark counts its entries off, and an entry arriving
/// under it is read and not counted.
#[test]
fn a_watermark_marks_read_and_moves_once() {
    let mut center = ready();
    center.replace(
        vec![
            entry(3, "chat", "room", 30),
            entry(2, "chat", "room", 20),
            entry(1, "chat", "other", 10),
        ],
        3,
    );
    assert!(center.watermark("chat", "room", 20));
    assert_eq!(center.unread(), 2);
    assert!(!center.watermark("chat", "room", 20));
    assert!(!center.watermark("chat", "room", 15));
    assert!(center.watermark("chat", "room", 30));
    assert_eq!(center.unread(), 1, "another tag's row stands");
    // an entry arriving under the watermark is read on arrival
    assert!(
        center
            .arrived(
                &settings(Some(Permission::Allow)),
                entry(4, "chat", "room", 25),
                Instant::now(),
                "Chat"
            )
            .is_none()
    );
    assert_eq!(center.unread(), 1);
    // the page listed again under the kept watermark: the inbox still
    // counts the three (its read is on the way); here they are read, and
    // the count says so
    center.replace(
        vec![
            entry(3, "chat", "room", 30),
            entry(2, "chat", "room", 20),
            entry(1, "chat", "other", 10),
        ],
        3,
    );
    assert!(center.rows().iter().filter(|row| row.read).count() == 1);
    assert_eq!(center.unread(), 1, "rows shown read are not counted");
}

/// A runner writes under its stamp: once the centre was claimed again
/// (the runner dropped, a new one started), the old stamp is refused.
#[test]
fn a_dropped_runners_stamp_is_refused() {
    let mut center = ready();
    let first = center.claim();
    assert!(center.stamped(first));
    let next = center.claim();
    assert!(!center.stamped(first) && center.stamped(next));
    center.set_network("other#1");
    assert!(center.stamped(next), "the stamp outlives a network change");
}

/// Without the inbox in hand, Mark all read, Clear read and a mute wait
/// for the next connect (the last mutes set alone); a tag read does not,
/// its watermark is kept.
#[test]
fn calls_without_the_inbox_wait_for_the_connect() {
    let mut center = Center::default();
    center.set_network("net#0");
    center.mark_all_read();
    center.clear_read();
    center.set_muted("chat", true);
    center.set_muted("forge", true);
    center.watermark("chat", "room", 3);
    assert_eq!(center.pending_for_test(), 3, "all read, clear, one mutes");
}

/// Without an inbox the bell shows no count, whatever the page holds.
#[test]
fn no_inbox_shows_no_count() {
    let mut center = ready();
    center.replace(vec![entry(1, "chat", "", 1)], 1);
    assert_eq!(center.unread(), 1);
    center.set_state(InboxState::NoInbox);
    assert_eq!(center.unread(), 0);
}

/// A mute replaces the account's mutes with the program in (or out) and
/// takes the program's question down.
#[test]
fn a_mute_moves_the_mutes_and_the_question() {
    let mut center = ready();
    center.ask_for_test("chat");
    center.set_muted("chat", true);
    assert!(center.muted("chat") && !center.asking.contains("chat"));
    center.set_muted("chat", false);
    assert!(!center.muted("chat"));
}

/// The settings read the prefs keys, with `"Block"` (the old word) no
/// longer one: it reads as no word.
#[test]
fn settings_read_the_prefs_keys() {
    let prefs = serde_json::json!({
        NOTIFY_PREF: false,
        FRONT_PREF: "show",
        BURST_PREF: 12,
        PROGRAMS_PREF: {"chat": "Allow", "forge": "Silent", "old": "Block"},
    });
    let settings = Settings::of(&prefs);
    assert_eq!(
        (settings.banners, settings.in_front, settings.burst),
        (false, true, 12)
    );
    assert_eq!(settings.programs.get("chat"), Some(&Permission::Allow));
    assert_eq!(settings.programs.get("forge"), Some(&Permission::Silent));
    assert_eq!(settings.programs.get("old"), None);
    let defaults = Settings::of(&serde_json::json!({BURST_PREF: 7}));
    assert_eq!(
        (defaults.banners, defaults.in_front, defaults.burst),
        (true, false, 6)
    );
}

/// A save stores what it wrote: the kept settings hold the word at once,
/// with no read in between for a load in flight to put the old settings
/// back over. Other tests load their own prefs into the shared cache
/// meanwhile, so the read is tried a few times: one that sees the word
/// proves the save stored it (a save that only forgets never does).
#[test]
fn a_save_stores_the_kept_settings() {
    let seen = (0..20).find_map(|_| {
        super::settings::set_permission("kept-test", Permission::Allow);
        Settings::kept_now()?.programs.get("kept-test").copied()
    });
    assert_eq!(seen, Some(Permission::Allow));
    // and the inbox runner, on another thread, reads the same cache
    let seen = (0..20).find_map(|_| {
        super::settings::set_permission("kept-test", Permission::Silent);
        std::thread::spawn(|| Settings::kept().programs.get("kept-test").copied())
            .join()
            .unwrap()
            .filter(|word| *word == Permission::Silent)
    });
    assert_eq!(seen, Some(Permission::Silent));
}

/// The old log directory goes, once, at start.
#[test]
fn the_old_log_directory_is_deleted() {
    let config = tempfile::tempdir().unwrap();
    let old = config.path().join("notifications");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("net.json"), b"[]").unwrap();
    super::remove_old_log(config.path());
    assert!(!old.exists());
    super::remove_old_log(config.path());
}

// ---------- the runner against a fake inbox ----------

async fn until(what: impl Fn() -> bool) {
    for _ in 0..400 {
        if what() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the centre never got there");
}

pub(crate) async fn seated(seed: u64) -> crate::backend::SeatSerial {
    use commonware_cryptography::Signer as _;
    let serial = crate::backend::seat_serial();
    crate::backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(
        seed,
    ))
    .await;
    serial
}

fn tag(height: u64) -> Read {
    Read::Tag {
        program: "chat".into(),
        tag: "room".into(),
        through_height: height,
    }
}

/// A connect reads the first page and the mutes; a `live` entry moves the
/// bell's count; a `gone` takes its row down; a row opened reads upstream
/// and the count agrees with the inbox's; dropping the runner forgets the
/// page and logs out.
#[tokio::test]
async fn the_runner_fills_the_centre_and_follows_live() {
    let _seat = seated(21).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    fake.set_mutes(inbox_api::Mutes {
        programs: vec!["forge".into()],
        ..Default::default()
    });
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    assert_eq!(center.lock().unread(), 1);
    assert!(center.lock().muted("forge"));
    fake.arrive(entry(2, "chat", "room", 2));
    until(|| center.lock().unread() == 2).await;
    assert_eq!(center.lock().rows()[0].count, 2);
    fake.push(Push::Gone { ids: vec![2] });
    until(|| center.lock().unread() == 1).await;
    assert_eq!(center.lock().rows()[0].count, 1);
    // a read elsewhere of an entry off this page reads the count again
    let mut read = entry(1, "chat", "room", 1);
    read.read = true;
    fake.set_entries(vec![read]);
    fake.push(Push::Read { ids: vec![77] });
    until(|| center.lock().unread() == 0).await;
    // the person's open goes upstream through the runner's client, and
    // the inbox's count comes to agree
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    fake.push(Push::Read { ids: vec![77] });
    until(|| center.lock().unread() == 1).await;
    center.lock().open(1);
    until(|| !fake.reads().is_empty()).await;
    assert_eq!(fake.reads()[0], tag(1));
    until(|| fake.unread() == 0).await;
    assert_eq!(center.lock().unread(), 0);
    // a view's watermark goes upstream once per move: the open's height is
    // kept, so the same height sends nothing
    center.lock().watermark("chat", "room", 1);
    center.lock().watermark("chat", "room", 3);
    until(|| fake.reads().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fake.reads(), vec![tag(1), tag(3)]);
    // dropped: the page goes, the session is logged out
    drop(_follower);
    assert!(center.lock().rows().is_empty());
    assert_eq!(center.lock().state, InboxState::Off);
    until(|| fake.logouts() == 1).await;
}

/// The socket is open before the page is read, so an entry pushed between
/// the two is not lost: the fake counts the sockets at the page.
#[tokio::test]
async fn live_is_subscribed_before_the_page_is_read() {
    let _seat = seated(28).await;
    let fake = FakeInbox::start().await;
    let center = CenterHandle::default();
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    assert_eq!(fake.sockets_at_page(), Some(1));
}

/// The label for a "N more" banner is looked up before the centre is
/// taken: a lookup that takes the centre itself (as the roster takes the
/// seats, which a view's `seen` holds while it takes the centre) does not
/// stop the runner.
#[tokio::test]
async fn the_label_is_looked_up_before_the_centre_is_locked() {
    let _seat = seated(29).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    center.lock().watermark("chat", "room", 10);
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let client = std::sync::Arc::new(crate::backend::inbox::Client::new(&rpc, "net#0", "d"));
    let locking = center.clone();
    let _follower = super::start_with(
        client,
        center.clone(),
        std::sync::Arc::new(move |program: &str| {
            let _taken = locking.lock();
            program.to_owned()
        }),
    );
    until(|| fake.reads().contains(&tag(10))).await;
    // an arrival under the watermark: looked up, then read upstream again
    fake.arrive(entry(2, "chat", "room", 5));
    until(|| fake.reads().len() == 2).await;
    assert_eq!(fake.reads()[1], tag(10));
    assert_eq!((center.lock().unread(), fake.unread()), (0, 0));
}

/// A row opened while the inbox is away is kept as a watermark and sent
/// when the inbox is back; the count agrees once it is.
#[tokio::test]
async fn a_row_opened_while_the_inbox_is_away_is_read_when_it_is_back() {
    let _seat = seated(30).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![
        entry(2, "chat", "room", 20),
        entry(1, "chat", "room", 1),
    ]);
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    fake.set_down(true);
    until(|| matches!(center.lock().state, InboxState::Unreachable { .. })).await;
    // the fold's newest height is what the open reads through
    center.lock().open(2);
    assert_eq!(center.lock().unread(), 0);
    assert!(fake.reads().is_empty());
    fake.set_down(false);
    until(|| fake.reads().contains(&tag(20))).await;
    until(|| fake.unread() == 0 && center.lock().unread() == 0).await;
}

/// A call that fails after its runner was dropped is never replayed by
/// the next runner (another account's, maybe); one that fails while the
/// inbox is still in hand (a deadline on a healthy socket) goes again now.
#[tokio::test]
async fn a_failed_call_stays_with_its_runner() {
    let _seat = seated(33).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let short = || {
        std::sync::Arc::new(
            crate::backend::inbox::Client::new(&rpc, "net#0", "d")
                .with_clock(Duration::from_millis(500), Duration::from_secs(30)),
        )
    };
    let label: super::follow::LabelOf = std::sync::Arc::new(|program: &str| program.to_owned());
    let follower = super::start_with(short(), center.clone(), label.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    // held past the deadline, then the runner dropped: the call is gone
    fake.hold_reads(2_000);
    center.lock().mark_all_read();
    until(|| fake.reads().len() == 1).await;
    drop(follower);
    tokio::time::sleep(Duration::from_millis(700)).await;
    fake.hold_reads(0);
    let _next = super::start_with(short(), center.clone(), label);
    until(|| center.lock().state == InboxState::Ready).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.reads().len(),
        1,
        "never replayed under the next runner"
    );
    assert_eq!(center.lock().pending_for_test(), 0);
    // held past the deadline while the socket holds: the connection is
    // lost, the call is kept, and the reconnect replays it once
    fake.hold("hide", 2_000);
    center.lock().clear_read();
    until(|| fake.hides() == 1).await;
    until(|| matches!(center.lock().state, InboxState::Unreachable { .. })).await;
    assert_eq!(fake.hides(), 1, "never re-sent from the task");
    fake.hold("hide", 0);
    until(|| center.lock().state == InboxState::Ready).await;
    until(|| fake.hides() == 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.hides(), 2, "once");
}

/// A mutes call kept for the connect carries the mutes as they stand
/// then: a set the inbox sent meanwhile is what goes out, never the set
/// at the click.
#[tokio::test]
async fn a_kept_mutes_call_carries_the_mutes_as_they_stand() {
    let _seat = seated(35).await;
    let fake = FakeInbox::start().await;
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    center.lock().set_muted("chat", true);
    center.lock().set_mutes(inbox_api::Mutes {
        programs: vec!["zed".into()],
        ..Default::default()
    });
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| fake.mute_puts().len() == 1).await;
    assert_eq!(fake.mute_puts()[0].programs, vec!["zed".to_owned()]);
}

/// An unread entry under a watermark the inbox already confirmed (indexed
/// after the read, while this device was away) is read upstream again
/// when the page lists it.
#[tokio::test]
async fn a_page_under_a_confirmed_watermark_reads_it_again() {
    let _seat = seated(34).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    center.lock().watermark("chat", "room", 10);
    until(|| fake.reads() == vec![tag(10)]).await;
    // the inbox indexes one more under it; the page is read again
    fake.set_entries(vec![
        entry(2, "chat", "room", 5),
        entry(1, "chat", "room", 1),
    ]);
    fake.push(Push::Read { ids: vec![77] });
    until(|| fake.reads() == vec![tag(10), tag(10)]).await;
    until(|| fake.unread() == 0 && center.lock().unread() == 0).await;
}

/// Mark all read, Clear read and a mute made without the inbox in hand go
/// out on the connect, in the order they were made.
#[tokio::test]
async fn calls_made_without_the_inbox_go_out_on_the_connect() {
    let _seat = seated(31).await;
    let fake = FakeInbox::start().await;
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    center.lock().mark_all_read();
    center.lock().clear_read();
    center.lock().set_muted("chat", true);
    let rpc = crate::backend::RpcClient::new(fake.origin());
    fake.hold_reads(400);
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| fake.reads().contains(&Read::All { all: true })).await;
    // the read's answer is held: the hide waits for it
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fake.hides(), 0, "the hide waits for the read's answer");
    until(|| fake.hides() == 1).await;
    until(|| fake.mute_puts().len() == 1).await;
    assert_eq!(fake.mute_puts()[0].programs, vec!["chat".to_owned()]);
    // the answers went out in the order the calls were made: the read,
    // the hide, then the mutes `PUT` (the connect's own `prefs` read first)
    let answered = fake.answered();
    let at = |name: &str| answered.iter().rposition(|call| call == name).unwrap();
    assert!(
        at("read") < at("hide") && at("hide") < at("prefs"),
        "{answered:?}"
    );
}

/// A 404 says this address keeps no inbox; a refused login is shown and
/// not retried.
#[tokio::test]
async fn no_inbox_and_a_refused_login_are_stated_not_retried() {
    let _seat = seated(22).await;
    let fake = FakeInbox::start().await;
    fake.set_no_inbox(true);
    let center = CenterHandle::default();
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::NoInbox).await;
    assert_eq!(center.lock().unread(), 0);
    until(|| _follower.ended()).await;
    drop(_follower);
    assert_eq!(center.lock().state, InboxState::Off);

    fake.set_no_inbox(false);
    fake.deny_logins(true);
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| matches!(center.lock().state, InboxState::Denied(_))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fake.logins(), 1, "a refused login is not retried");
}

/// The inbox going away keeps the page and says since when; nothing
/// arrives meanwhile; back, `entries` fills the gap.
#[tokio::test]
async fn unreachable_keeps_the_page_says_since_when_then_refills() {
    let _seat = seated(23).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    let center = CenterHandle::default();
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    fake.set_down(true);
    until(|| matches!(center.lock().state, InboxState::Unreachable { .. })).await;
    let InboxState::Unreachable { since } = center.lock().state.clone() else {
        unreachable!()
    };
    assert!(since > 0);
    assert_eq!(
        center.lock().rows().len(),
        1,
        "the page read this session stands"
    );
    fake.arrive(entry(2, "chat", "room", 2));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(center.lock().unread(), 1, "nothing lands while unreachable");
    fake.set_down(false);
    until(|| center.lock().state == InboxState::Ready).await;
    assert_eq!(center.lock().unread(), 2, "entries filled the gap");
    assert_eq!(center.lock().rows()[0].count, 2);
    // the backoff starts over after a healthy connection: the third
    // return takes a second, not four
    for _ in 0..2 {
        fake.set_down(true);
        until(|| matches!(center.lock().state, InboxState::Unreachable { .. })).await;
        let started = Instant::now();
        fake.set_down(false);
        until(|| center.lock().state == InboxState::Ready).await;
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

/// Mute goes upstream as the account's mutes with the program in; Clear
/// read hides upstream; a stopped follower is shown beside the count.
#[tokio::test]
async fn a_mute_and_a_clear_go_upstream_and_a_stopped_follower_is_shown() {
    let _seat = seated(24).await;
    let fake = FakeInbox::start().await;
    fake.set_stopped(Some(inbox_api::Stopped {
        height: 1200,
        reason: "a hole".into(),
    }));
    let center = CenterHandle::default();
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    assert_eq!(
        center.lock().stuck.as_ref().map(|stuck| stuck.height),
        Some(1200)
    );
    center.lock().set_muted("chat", true);
    until(|| !fake.mute_puts().is_empty()).await;
    assert_eq!(fake.mute_puts()[0].programs, vec!["chat".to_owned()]);
    assert!(fake.mute_puts()[0].kinds.is_empty());
    center.lock().clear_read();
    until(|| fake.hides() == 1).await;
    // the follower recovering while connected: the pushed status
    fake.push(Push::Status(inbox_api::Status {
        chain: "net#0".into(),
        indexed_height: 1300,
        tip: 1300,
        stopped: None,
        stalled: None,
    }));
    until(|| center.lock().stuck.is_none()).await;
}

/// `notify.seen` from a view is a watermark: the program's rows under the
/// tag through the height are read and the centre redraws once; the same
/// watermark again moves nothing.
#[test]
fn a_views_seen_is_a_watermark_on_the_centre() {
    use crate::runtime::Intent;
    use view_wire::methods::{self, Seen};
    let mut guest = crate::runtime::kernel::tests::guest();
    let program = guest.module;
    {
        let mut center = super::center().lock();
        center.set_network("seen-test#1");
        center.set_state(InboxState::Ready);
        center.replace(
            vec![entry(2, program, "room", 20), entry(1, program, "room", 10)],
            2,
        );
    }
    let seen = |id: u64, height: u64| view_wire::Request {
        id,
        kind: "notify.seen".into(),
        payload: methods::encode(&Seen {
            tag: "room".into(),
            through_height: height,
        }),
    };
    guest.answer(seen(1, 10), &None);
    assert_eq!(super::center().lock().unread(), 1);
    assert!(guest.intents.contains(&Intent::Notified));
    guest.intents.clear();
    guest.answer(seen(2, 10), &None);
    assert!(guest.intents.is_empty(), "the same watermark moved nothing");
    guest.answer(seen(3, 20), &None);
    assert_eq!(super::center().lock().unread(), 0);
    super::center().lock().set_network("");
}

/// The inbox's `Prefs` push replaces the mutes and re-reads the page
/// (mutes apply to stored rows); a `PUT` over `MAX_MUTES` is refused in a
/// sentence and the mutes go back to what the inbox keeps.
#[tokio::test]
async fn a_prefs_push_moves_the_mutes_and_the_page_and_a_refused_put_is_said() {
    let _seat = seated(25).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![
        entry(2, "forge", "web", 2),
        entry(1, "chat", "room", 1),
    ]);
    let center = CenterHandle::default();
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    assert_eq!(center.lock().rows().len(), 2);
    // another device mutes forge: the push lands, the page is read again
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    fake.push(Push::Prefs {
        mutes: inbox_api::Mutes {
            programs: vec!["forge".into()],
            ..Default::default()
        },
    });
    until(|| center.lock().muted("forge") && center.lock().rows().len() == 1).await;
    // this device's own mute comes back as the inbox keeps it: sorted, deduped
    center.lock().set_muted("chat", true);
    until(|| center.lock().mutes.programs == vec!["chat".to_owned(), "forge".to_owned()]).await;
    assert_eq!(center.lock().refused, None);
    // a list over the cap is refused, said, and the mutes put back
    {
        let mut center = center.lock();
        center.mutes.programs = (0..inbox_api::MAX_MUTES).map(|n| format!("p{n}")).collect();
        center.set_muted("one-more", true);
    }
    until(|| center.lock().refused.is_some()).await;
    assert!(
        center
            .lock()
            .refused
            .as_deref()
            .unwrap()
            .contains("at most")
    );
    assert_eq!(
        center.lock().mutes.programs,
        vec!["chat".to_owned(), "forge".to_owned()]
    );
}

/// A scrolling view's burst of watermarks under one tag sends at most one
/// read at a time: the first goes out, the newest waits behind it and
/// goes when it answers; the ones between are never sent. Another tag's
/// read is its own.
#[tokio::test]
async fn a_burst_of_watermarks_sends_one_read_per_tag_at_a_time() {
    let _seat = seated(26).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    fake.hold_reads(400);
    let center = CenterHandle::default();
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    for height in 10..=15 {
        center.lock().watermark("chat", "room", height);
    }
    center.lock().watermark("chat", "other", 3);
    until(|| fake.reads().len() == 3).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let reads = fake.reads();
    assert_eq!(reads.len(), 3, "first, newest, and the other tag's");
    let heights: Vec<(String, u64)> = reads
        .iter()
        .filter_map(|read| match read {
            Read::Tag {
                tag,
                through_height,
                ..
            } => Some((tag.clone(), *through_height)),
            _ => None,
        })
        .collect();
    assert!(heights.contains(&("room".into(), 10)));
    assert!(heights.contains(&("room".into(), 15)));
    assert!(heights.contains(&("other".into(), 3)));
}

/// A program's name over the inbox's cap is refused here, in a sentence,
/// and nothing is sent.
#[test]
fn a_mute_over_the_name_cap_is_refused_locally() {
    let mut center = ready();
    let long = "p".repeat(notices::MAX_LINK_PROGRAM_BYTES + 1);
    center.set_muted(&long, true);
    assert!(!center.muted(&long));
    assert!(center.refused.as_deref().unwrap().contains("at most"));
}

/// A view's watermark before the inbox is in hand (chat's timeline says
/// `seen` as it renders, before status, login and the first page) goes
/// upstream once the connection is made, and the tag is not stuck: a
/// later watermark goes out too.
#[tokio::test]
async fn a_watermark_before_the_connect_goes_out_on_it() {
    let _seat = seated(27).await;
    let fake = FakeInbox::start().await;
    fake.set_entries(vec![entry(1, "chat", "room", 1)]);
    let center = CenterHandle::default();
    center.lock().set_network("net#0");
    assert!(center.lock().watermark("chat", "room", 7));
    let rpc = crate::backend::RpcClient::new(fake.origin());
    let _follower = super::start(&rpc, "net#0", "Desktop", center.clone());
    until(|| center.lock().state == InboxState::Ready).await;
    until(|| fake.reads().contains(&tag(7))).await;
    assert!(center.lock().watermark("chat", "room", 9));
    until(|| fake.reads().contains(&tag(9))).await;
}
