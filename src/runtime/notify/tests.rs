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
            .arrived(&settings(None), entry(1, "chat", "", 1), now)
            .is_none()
    );
    assert!(center.asking.contains("chat") && center.rows().len() == 1);
    assert!(
        center
            .arrived(
                &settings(Some(Permission::Silent)),
                entry(2, "chat", "", 2),
                now
            )
            .is_none()
    );
    let banner = center
        .arrived(
            &settings(Some(Permission::Allow)),
            entry(3, "chat", "room", 3),
            now,
        )
        .unwrap();
    assert_eq!(
        (banner.title.as_str(), banner.tag.as_str()),
        ("title 3", "chat/room")
    );
    assert_eq!(center.unread(), 3);
    let mut off = settings(Some(Permission::Allow));
    off.banners = false;
    assert!(center.arrived(&off, entry(4, "chat", "", 4), now).is_none());
    // the same entry pushed twice counts once
    assert!(center.arrived(&off, entry(4, "chat", "", 4), now).is_none());
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
            .arrived(&allowed, entry(1, "chat", "", 1), now)
            .is_none()
    );
    allowed.in_front = true;
    assert!(
        center
            .arrived(&allowed, entry(2, "chat", "", 2), now)
            .is_some()
    );
    allowed.in_front = false;
    center.set_front(WindowKey::unique(), false, "");
    assert!(center.front.is_some());
    center.set_front(window, false, "");
    assert!(
        center
            .arrived(&allowed, entry(3, "chat", "", 3), now)
            .is_some()
    );
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
            .arrived(&allowed, entry(id, "chat", "", 1), start)
            .unwrap();
        assert_eq!(banner.title, format!("title {id}"));
    }
    let more = center
        .arrived(&allowed, entry(4, "chat", "", 1), start)
        .unwrap();
    assert_eq!(
        (more.title.as_str(), more.tag.as_str()),
        ("1 more from chat", "more/chat")
    );
    let more = center
        .arrived(&allowed, entry(5, "chat", "", 1), start)
        .unwrap();
    assert_eq!(more.title, "2 more from chat");
    let later = start + Duration::from_secs(21);
    let banner = center
        .arrived(&allowed, entry(6, "chat", "", 1), later)
        .unwrap();
    assert_eq!(banner.title, "title 6");
    assert_eq!(center.unread(), 6);
}

/// A `gone` takes the entry off the page, its count down, and names the
/// banner tag to take down; a `read` elsewhere does the same for the
/// count, and says when it names an entry this page does not have.
#[test]
fn gone_and_read_elsewhere_move_the_page_and_name_the_banner() {
    let mut center = ready();
    center.replace(
        vec![entry(2, "chat", "room", 2), entry(1, "chat", "room", 1)],
        2,
    );
    assert_eq!(center.removed(&[2]), vec!["chat/room".to_owned()]);
    assert_eq!(
        (center.rows().len(), center.rows()[0].count, center.unread()),
        (1, 1, 1)
    );
    assert_eq!(
        center.read_elsewhere(&[1]),
        (false, vec!["chat/room".to_owned()])
    );
    assert_eq!(center.unread(), 0);
    assert!(center.rows()[0].read);
    assert_eq!(center.read_elsewhere(&[9]), (true, Vec::new()));
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

/// A view's watermark marks the tag's rows at or below it read and moves
/// once: the same or a lower height is not a move, a higher one is.
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
                Instant::now()
            )
            .is_none()
    );
    assert_eq!(center.unread(), 1);
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

async fn seated(seed: u64) -> crate::backend::SeatSerial {
    use commonware_cryptography::Signer as _;
    let serial = crate::backend::seat_serial();
    crate::backend::seat_key(commonware_cryptography::ed25519::PrivateKey::from_seed(
        seed,
    ))
    .await;
    serial
}

/// A connect reads the first page and the mutes; a `live` entry moves the
/// bell's count; a `gone` takes its row down.
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
    // the person's open goes upstream through the runner's client
    center.lock().open(1);
    until(|| !fake.reads().is_empty()).await;
    assert_eq!(
        fake.reads()[0],
        Read::Tag {
            program: "chat".into(),
            tag: "room".into(),
            through_height: 1
        }
    );
    // a view's watermark goes upstream once per move
    center.lock().watermark("chat", "room", 1);
    center.lock().watermark("chat", "room", 1);
    center.lock().watermark("chat", "room", 3);
    until(|| fake.reads().len() == 3).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fake.reads().len(), 3, "an unmoved watermark sent nothing");
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
