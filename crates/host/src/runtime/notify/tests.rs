use std::time::{Duration, Instant};

use std::path::Path;
use std::sync::atomic::Ordering;

use super::center::{Center, CenterHandle, HOST, Notice, NoticeKind, read_log};
use super::settings::{BURST_PREF, FRONT_PREF, NOTIFY_PREF, Permission, Settings, VIEWS_PREF};
use super::{checked, in_order};
use crate::runtime::WindowKey;
use view_wire::methods::{self, Delivery, Notification};

fn post(title: &str, tag: &str) -> Notification {
    Notification {
        title: title.into(),
        body: "b".into(),
        tag: tag.into(),
        link: String::new(),
        ..Default::default()
    }
}

fn settings(permission: Option<Permission>) -> Settings {
    Settings {
        banners: true,
        in_front: false,
        burst: 3,
        views: permission
            .map(|permission| ("chat".to_owned(), permission))
            .into_iter()
            .collect(),
        unasked: None,
    }
}

#[test]
fn a_views_permission_decides_log_bar_or_banner() {
    let now = Instant::now();
    let mut center = Center::default();
    let (posted, banner) = center.post(&settings(None), "chat", "Chat", post("a", ""), now, 0);
    assert_eq!((posted, banner.is_none()), (Delivery::Logged, true));
    assert!(center.asking.contains("chat") && center.entries().count() == 1);

    let blocked = settings(Some(Permission::Block));
    let (posted, banner) = center.post(&blocked, "chat", "Chat", post("b", ""), now, 0);
    assert_eq!((posted, banner), (Delivery::Blocked, None));
    assert_eq!(
        center.entries().count(),
        1,
        "a blocked notice is not logged"
    );

    let silent = settings(Some(Permission::Silent));
    let (posted, banner) = center.post(&silent, "chat", "Chat", post("c", ""), now, 0);
    assert_eq!((posted, banner), (Delivery::Logged, None));

    let allowed = settings(Some(Permission::Allow));
    let (posted, banner) = center.post(&allowed, "chat", "Chat", post("d", "#room"), now, 0);
    assert_eq!(posted, Delivery::Banner);
    assert_eq!(
        banner.unwrap().tag,
        "chat/#room",
        "a view's tags are its own"
    );
    assert_eq!(center.unread(), 3);

    let mut off = allowed.clone();
    off.banners = false;
    assert_eq!(
        center.post(&off, "chat", "Chat", post("e", ""), now, 0).0,
        Delivery::Logged
    );

    center.asking.clear();
    center.not_now.insert("chat".into());
    center.post(&settings(None), "chat", "Chat", post("f", ""), now, 0);
    assert!(!center.asking.contains("chat"));
}

#[test]
fn the_focused_view_is_not_bannered_unless_show_is_set() {
    let now = Instant::now();
    let mut center = Center::default();
    let window = WindowKey::unique();
    let mut allowed = settings(Some(Permission::Allow));
    center.set_front(window, true, "chat");
    assert_eq!(
        center.post(&allowed, "chat", "Chat", post("a", ""), now, 0),
        (Delivery::Logged, None)
    );
    allowed.in_front = true;
    assert_eq!(
        center
            .post(&allowed, "chat", "Chat", post("b", ""), now, 0)
            .0,
        Delivery::Banner
    );
    allowed.in_front = false;
    center.set_front(WindowKey::unique(), false, "");
    assert!(center.front.is_some());
    center.set_front(window, false, "");
    assert_eq!(
        center
            .post(&allowed, "chat", "Chat", post("c", ""), now, 0)
            .0,
        Delivery::Banner
    );
}

#[test]
fn a_catch_up_burst_folds_by_room_and_stops_at_the_burst() {
    let now = Instant::now();
    let mut center = Center::default();
    let unasked = Settings {
        unasked: Some(Permission::Allow),
        ..settings(None)
    };
    let raised: Vec<_> = (0..20)
        .filter_map(|nth| {
            let room = ["#general", "@ada"][nth as usize % 2];
            let post = post(&format!("mention {nth}"), room);
            center.post(&unasked, "chat", "Chat", post, now, nth).1
        })
        .collect();
    let tags: std::collections::BTreeSet<_> = raised.iter().map(|it| it.tag.as_str()).collect();
    assert_eq!(
        tags,
        ["chat/#general", "chat/@ada", "more/chat"].into(),
        "the rooms' own and one more"
    );
    let rooms = raised.iter().filter(|it| it.tag != "more/chat").count();
    assert_eq!(rooms, 3, "the burst limit's worth");
    assert_eq!(raised.last().unwrap().title, "17 more from Chat");
    assert_eq!(center.entries().count(), 2, "one row per room");
    assert_eq!(center.entries().map(|row| row.count).sum::<u32>(), 20);
    assert!(center.asking.is_empty(), "no one to ask");
}

#[test]
fn a_burst_past_the_limit_coalesces_into_one_more_banner() {
    let start = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    for nth in 0..3 {
        let (posted, _) = center.post(&allowed, "chat", "Chat", post("m", ""), start, nth);
        assert_eq!(posted, Delivery::Banner);
    }
    let (posted, more) = center.post(&allowed, "chat", "Chat", post("m", ""), start, 3);
    let more = more.unwrap();
    assert_eq!(posted, Delivery::Logged);
    assert_eq!(
        (more.title.as_str(), more.tag.as_str()),
        ("1 more from Chat", "more/chat")
    );
    let (_, more) = center.post(&allowed, "chat", "Chat", post("m", ""), start, 4);
    assert_eq!(more.unwrap().title, "2 more from Chat");
    assert_eq!(center.entries().count(), 5, "everything is still logged");
    let mut forge = allowed.clone();
    forge.views.insert("forge".into(), Permission::Allow);
    assert_eq!(
        center
            .post(&forge, "forge", "Forge", post("f", ""), start, 5)
            .0,
        Delivery::Banner
    );
    let later = start + Duration::from_secs(20);
    assert_eq!(
        center
            .post(&allowed, "chat", "Chat", post("m", ""), later, 6)
            .0,
        Delivery::Banner
    );
    assert_eq!(
        center
            .post(&allowed, "chat", "Chat", post("m", ""), later, 7)
            .1
            .unwrap()
            .title,
        "1 more from Chat",
        "the count starts over after a banner"
    );
}

#[test]
fn notices_under_one_tag_fold_into_one_row() {
    let now = Instant::now();
    let mut center = Center::default();
    let silent = settings(Some(Permission::Silent));
    center.post(&silent, "chat", "Chat", post("first", "#room"), now, 1);
    center.post(&silent, "chat", "Chat", post("other", ""), now, 2);
    let id = center.entries().last().unwrap().id;
    center.open(id);
    center.post(&silent, "chat", "Chat", post("second", "#room"), now, 3);
    center.post(&silent, "forge", "Forge", post("theirs", "#room"), now, 4);
    let rows: Vec<_> = center
        .entries()
        .map(|entry| (entry.title.as_str(), entry.count, entry.read))
        .collect();
    assert_eq!(
        rows,
        [
            ("theirs", 1, false),
            ("second", 2, false),
            ("other", 1, false)
        ]
    );
    center.mark_all_read();
    assert_eq!(center.unread(), 0);
    center.post(&silent, "chat", "Chat", post("new", ""), now, 5);
    center.clear_read();
    assert_eq!(center.entries().count(), 1);
}

#[test]
fn reading_a_tag_reads_only_the_views_own_rows_under_it() {
    let now = Instant::now();
    let mut center = Center::default();
    let silent = settings(Some(Permission::Silent));
    center.post(&silent, "chat", "Chat", post("a", "#design"), now, 1);
    center.post(&silent, "chat", "Chat", post("b", "#design"), now, 2);
    center.post(&silent, "chat", "Chat", post("c", "@alice"), now, 3);
    center.post(&silent, "chat", "Chat", post("d", ""), now, 4);
    center.post(&silent, "forge", "Forge", post("theirs", "#design"), now, 5);
    assert!(center.read_tag("chat", "#design"));
    let unread: Vec<_> = center
        .entries()
        .filter(|entry| !entry.read)
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(unread, ["theirs", "d", "c"]);
    assert!(!center.read_tag("chat", "#design"), "nothing left to read");
    assert!(!center.read_tag("chat", ""), "an empty tag reads nothing");
    assert_eq!(center.unread(), 3);
}

#[test]
fn the_log_keeps_every_row_until_cleared() {
    let now = Instant::now();
    let mut center = Center::default();
    let silent = settings(Some(Permission::Silent));
    for nth in 0..600 {
        center.post(
            &silent,
            "chat",
            "Chat",
            post(&nth.to_string(), ""),
            now,
            nth,
        );
    }
    center.post(&silent, "chat", "Chat", post("late", ""), now, 1 << 40);
    assert_eq!(center.entries().count(), 601);
    assert_eq!(center.entries().last().unwrap().title, "0");
}

#[test]
fn prefs_read_back_as_settings() {
    let read = Settings::of(&serde_json::json!({}), false);
    assert_eq!((read.banners, read.in_front, read.burst), (true, false, 6));
    assert_eq!(read.unasked, None, "a desktop asks per program");
    let read = Settings::of(
        &serde_json::json!({
            NOTIFY_PREF: false, FRONT_PREF: "show", BURST_PREF: 12,
            VIEWS_PREF: { "chat": "Silent", "forge": "nonsense" },
        }),
        false,
    );
    assert_eq!((read.banners, read.in_front, read.burst), (false, true, 12));
    assert_eq!(read.views.len(), 1);
    assert_eq!(
        Settings::of(&serde_json::json!({ BURST_PREF: 7 }), false).burst,
        6
    );
    let phone = Settings::of(&serde_json::json!({}), true);
    assert_eq!(
        phone.unasked,
        Some(Permission::Allow),
        "the OS asked for the whole app, and a program unasked here was held back"
    );
}

#[test]
fn a_post_is_words_a_tag_and_a_duck_link() {
    let full = Notification {
        title: "a".into(),
        body: "b".into(),
        tag: "t".into(),
        link: "duck://chat/room".into(),
        ..Default::default()
    };
    let mut bytes = methods::encode(&full);
    assert_eq!(methods::decode::<Notification>(&bytes).unwrap(), full);
    bytes.extend_from_slice(b"icon");
    assert!(methods::decode::<Notification>(&bytes).is_err());
    assert!(checked(Notification::default()).is_err());
    assert!(
        checked(Notification {
            link: "https://example.com".into(),
            ..full.clone()
        })
        .is_err()
    );
    assert!(
        checked(Notification {
            end_link: "https://example.com/leave".into(),
            ..full.clone()
        })
        .is_err()
    );
    let long = Notification {
        title: "가".repeat(4096),
        body: "b".repeat(1 << 16),
        link: format!("duck://{}", "x".repeat(4096)),
        ..full
    };
    assert_eq!(checked(long.clone()).unwrap(), long);
}

#[test]
fn a_views_banners_leave_in_order() {
    let (tell, told) = std::sync::mpsc::channel();
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let mut gate = Some(gate);
    for nth in 0..10u64 {
        let tell = tell.clone();
        let gate = gate.take();
        in_order("order-test", move || {
            if let Some(gate) = gate {
                let _ = gate.recv();
            }
            tell.send(nth).unwrap();
        });
    }
    drop(open);
    drop(tell);
    assert_eq!(told.iter().collect::<Vec<_>>(), (0..10).collect::<Vec<_>>());
}

#[test]
fn a_log_cut_short_is_set_aside_and_the_next_post_writes_a_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.json");
    let silent = settings(Some(Permission::Silent));
    let _off = crate::perf::off_for_test();
    let mut center = Center::default();
    center.open_log("dev", Some(path.clone()));
    center.post(&silent, "chat", "Chat", post("old", ""), Instant::now(), 1);
    flushed(&mut center, &path);
    let whole = std::fs::read(&path).unwrap();
    let cut = &whole[..whole.len() - 3];
    std::fs::write(&path, cut).unwrap();

    assert!(read_log(&path).unwrap().is_empty());
    assert_eq!(std::fs::read(dir.path().join("dev.json.bad")).unwrap(), cut);

    let mut reopened = Center::default();
    reopened.open_log("dev", Some(path.clone()));
    reopened.post(&silent, "chat", "Chat", post("new", ""), Instant::now(), 2);
    flushed(&mut reopened, &path);
    let kept = read_log(&path).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].title, "new");
}

#[test]
fn an_unreadable_log_is_not_saved_over() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.json");
    let silent = settings(Some(Permission::Silent));
    let _off = crate::perf::off_for_test();
    let mut center = Center::default();
    center.open_log("dev", Some(path.clone()));
    center.post(&silent, "chat", "Chat", post("old", ""), Instant::now(), 1);
    flushed(&mut center, &path);
    if !crate::backend::unreadable_for_test(&path) {
        return;
    }

    let mut reopened = Center::default();
    reopened.open_log("dev", Some(path.clone()));
    reopened.post(&silent, "chat", "Chat", post("new", ""), Instant::now(), 2);
    flushed(&mut reopened, &path);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(reopened.entries().count(), 1);
    let kept = read_log(&path).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].title, "old");
}

fn flushed(center: &mut Center, path: &Path) {
    center.flush();
    settled(path);
}

fn settled(path: &Path) {
    let (tell, told) = std::sync::mpsc::channel();
    in_order(&path.to_string_lossy(), move || tell.send(()).unwrap());
    told.recv().unwrap();
}

#[test]
fn the_quit_drain_waits_for_a_queued_log_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.json");
    let _off = crate::perf::off_for_test();
    let mut center = Center::default();
    center.open_log("dev", Some(path.clone()));
    let release = crate::runtime::kernel::tests::held(&path.to_string_lossy());
    let silent = settings(Some(Permission::Silent));
    center.post(&silent, "chat", "Chat", post("last", ""), Instant::now(), 1);
    center.flush();
    assert!(!path.exists(), "the write waits behind the held job");

    let mut drained = std::pin::pin!(crate::runtime::kernel::drain());
    assert!(
        crate::runtime::kernel::tests::waiting(drained.as_mut()),
        "the drain is done while the log write is still queued"
    );
    release.send(()).unwrap();
    futures::executor::block_on(drained);
    let kept = read_log(&path).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].title, "last");
}

#[test]
fn quitting_hands_the_log_over_before_it_drains() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.json");
    let _off = crate::perf::off_for_test();
    let center = CenterHandle::default();
    center.lock().open_log("dev", Some(path.clone()));
    let silent = settings(Some(Permission::Silent));
    center
        .lock()
        .post(&silent, "chat", "Chat", post("last", ""), Instant::now(), 1);
    let release = crate::runtime::kernel::tests::held(&path.to_string_lossy());

    let mut drained = std::pin::pin!(crate::runtime::quitting(&center, ()));
    assert!(crate::runtime::kernel::tests::waiting(drained.as_mut()));
    release.send(()).unwrap();
    futures::executor::block_on(drained);
    let kept = read_log(&path).unwrap();
    assert_eq!(kept.len(), 1, "the redraw's row is on disk when quit ends");
    assert_eq!(kept[0].title, "last");
}

#[test]
fn one_views_flood_leaves_the_other_views_rows() {
    let now = Instant::now();
    let mut center = Center::default();
    let mut answered = settings(Some(Permission::Silent));
    answered.views.insert("forge".into(), Permission::Allow);
    for nth in 0..3 {
        center.post(
            &answered,
            "forge",
            "Forge",
            post(&format!("forge {nth}"), ""),
            now,
            nth,
        );
    }
    for nth in 0..600 {
        center.post(
            &answered,
            "chat",
            "Chat",
            post(&nth.to_string(), ""),
            now,
            10 + nth,
        );
    }
    let rows = |center: &Center, program: &str| {
        center
            .entries()
            .filter(|entry| entry.program == program)
            .map(|entry| entry.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&center, "forge"), ["forge 2", "forge 1", "forge 0"]);
    assert_eq!(rows(&center, "chat").len(), 600);
    let unanswered = settings(None);
    for nth in 0..600 {
        center.post(
            &unanswered,
            "spam",
            "Spam",
            post(&nth.to_string(), ""),
            now,
            700 + nth,
        );
    }
    assert!(center.asking.contains("spam"), "the bar still asks");
    assert_eq!(rows(&center, "spam").len(), 600);
    assert_eq!(rows(&center, "forge").len(), 3);
}

#[test]
fn a_redraws_posts_read_the_prefs_once_and_write_the_log_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.json");
    let center = CenterHandle::default();
    center.lock().open_log("dev", Some(path.clone()));
    crate::backend::edit_prefs(|prefs| {
        prefs[VIEWS_PREF] = serde_json::json!({ "request-test": "Silent" });
    });
    let mut view = crate::runtime::kernel::tests::view();
    view.connection.chain = "dev".into();
    let reads = crate::backend::prefs_reads();
    for id in 0..256 {
        let notice = methods::encode(&post(&id.to_string(), ""));
        super::post(&mut view, &center, id, &notice);
    }
    assert_eq!(
        crate::backend::prefs_reads() - reads,
        1,
        "prefs read per post"
    );
    assert!(
        !path.exists(),
        "the log was written before the redraw ended"
    );
    center.lock().flush();
    settled(&path);
    // Another center's worker also saves while this redraw is measured.
    let other_path = dir.path().join("@device.json");
    let mut other = Center::default();
    other.open_device_log(other_path.clone());
    other.post_host("unrelated", "", "", 0);
    settled(&other_path);
    assert_eq!(
        center.lock().saves.load(Ordering::Relaxed),
        1,
        "one write for the redraw's posts"
    );
    let kept = read_log(&path).unwrap();
    assert_eq!(kept.len(), 256);
    assert_eq!(kept.last().unwrap().title, "255");
}

#[test]
fn request_ids_differ_within_and_across_processes() {
    let (first, second) = (super::request_id(), super::request_id());
    assert_ne!(first, second);
    let token = |id: &str| id.rsplit_once('-').unwrap().0.to_owned();
    assert_eq!(token(&first), token(&second), "one process, one token");
    assert_ne!(token(&first), "ducktape", "a token, not a bare count");
    let at = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let token_of = super::process_token;
    assert_ne!(token_of(7, at), token_of(7, at + Duration::from_millis(1)));
    assert_ne!(token_of(7, at), token_of(8, at));
}

#[test]
fn a_views_live_line_shows_once_a_run() {
    let mut center = Center::default();
    assert!(center.went_live("chat"));
    assert!(center.sharing.contains("chat"));
    assert!(
        !center.went_live("chat"),
        "a second track drew the line again"
    );
    center.put_away_live("chat");
    assert!(!center.sharing.contains("chat"));
    assert!(!center.went_live("chat"), "a line put away came back");
    assert!(!center.sharing.contains("chat"));
}

fn call(tag: &str, until: u64) -> Notification {
    Notification {
        kind: NoticeKind::Call,
        until,
        end_link: format!("duck://chat/{tag}/leave"),
        ..post("Ada is calling", tag)
    }
}

#[test]
fn a_wire_call_rings_as_a_call_and_is_over_at_its_until() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    for nth in 0..3 {
        center.post(&allowed, "chat", "Chat", post("m", ""), now, nth);
    }
    let payload = methods::encode(&call("call/1", 100));
    let wire = methods::decode::<Notification>(&payload).unwrap();
    let (posted, banner) = center.post(&allowed, "chat", "Chat", wire, now, 3);
    let banner = banner.unwrap();
    assert_eq!(posted, Delivery::Banner);
    assert_eq!(
        (banner.kind, banner.until, banner.tag.as_str()),
        (NoticeKind::Call, 100, "chat/call/1")
    );
    assert_eq!(banner.end_link, "duck://chat/call/1/leave");
    assert!(center.ring_over("chat/call/1", 100));
}

#[test]
fn a_call_rings_past_the_burst_and_every_time_before_the_persons_word() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    for nth in 0..3 {
        center.post(&allowed, "chat", "Chat", post("m", ""), now, nth);
    }
    assert!(
        center
            .post(&allowed, "chat", "Chat", post("m", ""), now, 3)
            .1
            .unwrap()
            .title
            .ends_with("more from Chat")
    );
    let (posted, banner) = center.post(&allowed, "chat", "Chat", call("call/1", 100), now, 4);
    let banner = banner.unwrap();
    assert_eq!(posted, Delivery::Banner);
    assert_eq!(
        (banner.kind, banner.tag.as_str()),
        (NoticeKind::Call, "chat/call/1")
    );

    let unasked = settings(None);
    let mut center = Center::default();
    let (posted, banner) = center.post(&unasked, "chat", "Chat", call("call/1", 100), now, 0);
    assert_eq!(
        (posted, banner.map(|notice| notice.kind)),
        (Delivery::Banner, Some(NoticeKind::Call))
    );
    assert!(center.asking.contains("chat"), "the bar asks too");
    let (posted, banner) = center.post(&unasked, "chat", "Chat", call("call/1b", 100), now, 50);
    assert_eq!((posted, banner), (Delivery::Logged, None));
    let (posted, banner) = center.post(&unasked, "chat", "Chat", call("call/2", 200), now, 101);
    assert_eq!(
        (posted, banner.map(|notice| notice.tag)),
        (Delivery::Banner, Some("chat/call/2".to_owned())),
        "a second call rang unasked"
    );
    assert!(center.asking.contains("chat"));
    super::settings::not_now(&mut center, "chat");
    let (posted, _) = center.post(&unasked, "chat", "Chat", call("call/2b", 300), now, 201);
    assert_eq!(posted, Delivery::Banner);
    assert!(!center.asking.contains("chat"));
    let (posted, banner) = center.post(&unasked, "chat", "Chat", post("m", ""), now, 102);
    assert_eq!((posted, banner), (Delivery::Logged, None));
    let (posted, banner) = center.post(&unasked, "forge", "Forge", call("call/3", 100), now, 2);
    assert_eq!(posted, Delivery::Banner, "another view rings its own");
    assert!(banner.is_some());
    let silent = settings(Some(Permission::Silent));
    assert_eq!(
        center
            .post(&silent, "chat", "Chat", call("call/4", 100), now, 3)
            .0,
        Delivery::Logged
    );
    let blocked = settings(Some(Permission::Block));
    assert_eq!(
        center
            .post(&blocked, "chat", "Chat", call("call/5", 100), now, 4)
            .0,
        Delivery::Blocked
    );
}

#[test]
fn a_notice_past_its_until_is_logged_only() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    let (posted, banner) = center.post(&allowed, "chat", "Chat", call("call/1", 100), now, 100);
    assert_eq!((posted, banner), (Delivery::Logged, None));
    assert_eq!(center.entries().count(), 1, "still logged");
    let (posted, banner) = center.post(&allowed, "chat", "Chat", call("call/1", 101), now, 100);
    assert_eq!(posted, Delivery::Banner);
    assert_eq!(banner.unwrap().until, 101);
}

#[test]
fn a_banner_is_over_at_its_until_unless_its_tag_moved_on() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    center.post(&allowed, "chat", "Chat", call("call/1", 100), now, 0);
    assert!(center.ring_over("chat/call/1", 100));
    assert!(!center.ring_over("chat/call/1", 100), "over twice");

    center.post(&allowed, "chat", "Chat", call("call/1", 100), now, 0);
    center.post(&allowed, "chat", "Chat", call("call/1", 160), now, 100);
    assert!(
        !center.ring_over("chat/call/1", 100),
        "the first ring's timer took the second down"
    );
    assert!(center.ring_over("chat/call/1", 160));

    center.post(&allowed, "chat", "Chat", call("call/2", 100), now, 0);
    center.read_tag("chat", "call/2");
    assert!(
        !center.ring_over("chat/call/2", 100),
        "seen, it is down already"
    );

    center.post(&allowed, "chat", "Chat", call("call/3", 100), now, 0);
    center.post(&allowed, "chat", "Chat", post("plain", "call/3"), now, 0);
    assert!(
        !center.ring_over("chat/call/3", 100),
        "a plain notice took the tag"
    );
}

#[test]
fn a_program_rings_one_call_at_a_time() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    let mut ring = |program: &str, tag: &str, until: u64, wall: i64| {
        center.post(&allowed, program, program, call(tag, until), now, wall)
    };
    assert_eq!(ring("chat", "call/1", 100, 0).0, Delivery::Banner);
    assert_eq!(
        ring("chat", "call/2", 100, 1),
        (Delivery::Logged, None),
        "a second call rang over the first"
    );
    assert_eq!(
        ring("chat", "call/1", 160, 2),
        (Delivery::Logged, None),
        "the standing call rang again"
    );
    assert_eq!(
        ring("forge", "call/1", 100, 3).0,
        Delivery::Banner,
        "another program's call"
    );
    center.read_tag("chat", "call/1");
    let mut ring = |program: &str, tag: &str, until: u64, wall: i64| {
        center.post(&allowed, program, program, call(tag, until), now, wall)
    };
    assert_eq!(
        ring("chat", "call/2", 100, 4).0,
        Delivery::Banner,
        "seen, the next rings"
    );
    assert_eq!(
        ring("chat", "call/3", 200, 100).0,
        Delivery::Banner,
        "past its until, the next rings"
    );
    let plain = |(_, banner): (Delivery, Option<Notice>)| banner.map(|notice| notice.kind);
    assert_eq!(
        plain(ring("chat", "call/4", 0, 101)),
        Some(NoticeKind::Notice)
    );
    assert_eq!(plain(ring("chat", "", 300, 101)), Some(NoticeKind::Notice));
}

#[test]
fn a_declined_call_lets_its_program_ring_again() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    center.post(&allowed, "chat", "Chat", call("call/1", 100), now, 0);
    assert_eq!(
        center
            .post(&allowed, "chat", "Chat", call("call/2", 100), now, 1)
            .0,
        Delivery::Logged
    );
    center.declined("chat/call/1");
    assert_eq!(
        center
            .post(&allowed, "chat", "Chat", call("call/2", 100), now, 2)
            .0,
        Delivery::Banner,
        "the declined call still held its program"
    );
    assert!(
        !center.ring_over("chat/call/1", 100),
        "declined, it is down"
    );
}

#[test]
fn an_until_past_the_clock_sets_no_timer() {
    let timer = super::withdraw_at(
        CenterHandle::default(),
        None,
        "chat",
        (String::new(), "chat/call/1".into()),
        u64::MAX,
    );
    crate::runtime::kernel::handle()
        .block_on(timer)
        .expect("the timer panicked");
}

#[test]
fn a_calls_banner_is_withdrawn_at_its_until() {
    let center = CenterHandle::default();
    let allowed = settings(Some(Permission::Allow));
    let until = super::wall() as u64 + 2;
    let (_, banner) = center.lock().post(
        &allowed,
        "chat",
        "Chat",
        call("call/1", until),
        Instant::now(),
        0,
    );
    let banner = banner.unwrap();
    let timer = super::withdraw_at(
        center.clone(),
        None,
        "chat",
        (banner.chain.clone(), banner.tag.clone()),
        until,
    );
    let started = Instant::now();
    crate::runtime::kernel::handle().block_on(timer).unwrap();
    assert!(
        started.elapsed() > Duration::from_millis(50),
        "taken down before its until"
    );
    assert!(
        !center.lock().ring_over(&banner.tag, until),
        "the timer left it standing"
    );
}

fn carried(title: &str, tag: &str, link: &str) -> Notice {
    Notice {
        title: title.into(),
        body: "b".into(),
        tag: match tag.is_empty() {
            true => String::new(),
            false => format!("chat/{tag}"),
        },
        program: "chat".into(),
        link: link.into(),
        ..Notice::default()
    }
}

fn rows(center: &Center) -> Vec<(String, u32, bool, i64)> {
    center
        .entries()
        .map(|entry| (entry.title.clone(), entry.count, entry.read, entry.at))
        .collect()
}

#[test]
fn a_carried_notice_is_one_row_whoever_logs_it_again() {
    let now = Instant::now();
    let silent = settings(Some(Permission::Silent));
    let mut center = Center::default();
    let notice = carried("ada mentioned you", "#room", "duck://c/chat/room/7");
    assert!(center.carried(&silent, &notice, 100, false));
    let entry = center.entries().next().unwrap().clone();
    assert_eq!(
        (entry.program.as_str(), entry.tag.as_str()),
        ("chat", "#room"),
        "the row is under the view's own tag"
    );
    assert!(
        !center.carried(&silent, &notice, 100, false),
        "read again, logged again"
    );
    assert_eq!(rows(&center), [("ada mentioned you".into(), 1, false, 100)]);

    let same = Notification {
        link: "duck://c/chat/room/7".into(),
        ..post("ada mentioned you", "#room")
    };
    center.post(&silent, "chat", "Chat", same.clone(), now, 160);
    assert_eq!(rows(&center), [("ada mentioned you".into(), 1, false, 100)]);
    center.mark_all_read();
    assert!(
        !center.carried(&silent, &notice, 100, false),
        "read, then unread again"
    );

    let opened = carried("bob mentioned you", "#other", "");
    assert!(center.carried(&silent, &opened, 120, true));
    center.post(
        &silent,
        "chat",
        "Chat",
        post("bob mentioned you", "#other"),
        now,
        170,
    );
    assert!(
        center.entries().next().unwrap().read,
        "opened, then unread again"
    );

    center.post(&silent, "chat", "Chat", same, now, 200);
    assert_eq!(
        rows(&center)[0],
        ("ada mentioned you".into(), 2, false, 200),
        "a repeat of this device's own was taken for a copy"
    );
}

#[test]
fn a_carried_notice_raises_no_second_banner() {
    let now = Instant::now();
    let allow = settings(Some(Permission::Allow));
    let mut center = Center::default();
    assert!(center.carried(&allow, &carried("ada", "#room", ""), 100, false));
    let (posted, banner) = center.post(&allow, "chat", "Chat", post("ada", "#room"), now, 160);
    assert_eq!((posted, banner), (Delivery::Logged, None), "bannered twice");
    let (posted, banner) = center.post(&allow, "chat", "Chat", post("bob", "#room"), now, 170);
    assert_eq!(posted, Delivery::Banner);
    assert_eq!(banner.map(|banner| banner.title), Some("bob".to_owned()));
}

#[test]
fn the_os_copy_of_a_posted_notice_is_not_logged_twice() {
    let now = Instant::now();
    let silent = settings(Some(Permission::Silent));
    let mut center = Center::default();
    center.post(&silent, "chat", "Chat", post("first", "#room"), now, 100);
    assert!(!center.carried(&silent, &carried("first", "#room", ""), 101, false));
    assert_eq!(rows(&center), [("first".into(), 1, false, 100)]);

    center.mark_all_read();
    assert!(center.carried(&silent, &carried("second", "#room", ""), 102, false));
    assert!(center.carried(&silent, &carried("early", "", ""), 50, false));
    assert_eq!(
        rows(&center),
        [
            ("second".into(), 2, false, 102),
            ("early".into(), 1, false, 50)
        ]
    );
    assert!(center.carried(&silent, &carried("second", "#room", ""), 102, true));
    assert_eq!(
        center.unread(),
        1,
        "opening it from the OS left the row unread"
    );

    let blocked = settings(Some(Permission::Block));
    assert!(!center.carried(&blocked, &carried("no", "#x", ""), 300, false));
    let elsewhere = carried("no", "#y", "https://example.com");
    assert!(!center.carried(&silent, &elsewhere, 300, false));
    let more = Notice {
        tag: "more/chat".into(),
        ..carried("3 more from Chat", "", "")
    };
    assert!(!center.carried(&silent, &more, 300, false));
    assert_eq!(center.entries().count(), 2);
}

#[test]
fn a_carried_notice_is_on_disk_for_the_next_launch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dev.json");
    let _off = crate::perf::off_for_test();
    let mut center = Center::default();
    center.open_log("dev", Some(path.clone()));
    let silent = settings(Some(Permission::Silent));
    center.carried(&silent, &carried("kept", "#room", ""), 100, true);
    settled(&path);
    settled(&dir.path().join("dev@carried.json"));
    let mut relaunched = Center::default();
    relaunched.open_log("dev", Some(path));
    assert_eq!(rows(&relaunched), [("kept".into(), 1, true, 100)]);
    let now = Instant::now();
    relaunched.post(&silent, "chat", "Chat", post("kept", "#room"), now, 160);
    assert_eq!(
        rows(&relaunched),
        [("kept".into(), 1, true, 100)],
        "opened from the OS, then unread again after a relaunch"
    );
    flushed(&mut relaunched, &dir.path().join("dev@carried.json"));
    let mut again = Center::default();
    again.open_log("dev", Some(dir.path().join("dev.json")));
    again.post(&silent, "chat", "Chat", post("kept", "#room"), now, 170);
    assert_eq!(rows(&again), [("kept".into(), 2, false, 170)]);
}

#[test]
fn the_copies_waiting_for_this_devices_run_are_bounded() {
    let silent = settings(Some(Permission::Silent));
    let mut center = Center::default();
    for nth in 0..=super::center::CARRIED_KEPT {
        let notice = carried(&format!("n{nth}"), &format!("#{nth}"), "");
        center.carried(&silent, &notice, 100, false);
    }
    let now = Instant::now();
    center.post(&silent, "chat", "Chat", post("n0", "#0"), now, 200);
    let last = super::center::CARRIED_KEPT;
    center.post(
        &silent,
        "chat",
        "Chat",
        post(&format!("n{last}"), &format!("#{last}")),
        now,
        200,
    );
    let count = |title: &str| {
        let entry = center.entries().find(|entry| entry.title == title).unwrap();
        entry.count
    };
    assert_eq!(count("n0"), 2, "the oldest copy outlived the bound");
    assert_eq!(count(&format!("n{last}")), 1);
}

#[test]
fn the_hosts_rows_stand_beside_every_chains_in_a_file_of_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let (device, chain) = (dir.path().join("@device.json"), dir.path().join("dev.json"));
    let _off = crate::perf::off_for_test();
    let titles = |center: &Center| {
        center
            .entries()
            .map(|entry| entry.title.clone())
            .collect::<Vec<_>>()
    };
    let mut center = Center::default();
    center.open_device_log(device.clone());
    center.post_host(
        "Ducktape 2 is ready.",
        "Restart to update.",
        "update/ready",
        10,
    );
    assert_eq!(
        titles(&center),
        ["Ducktape 2 is ready."],
        "before any network"
    );
    let host = center.entries().next().unwrap().clone();
    assert!(host.host());
    assert_eq!(
        (host.program.as_str(), host.tag.as_str()),
        (HOST, "update/ready")
    );

    center.open_log("dev", Some(chain.clone()));
    let silent = settings(Some(Permission::Silent));
    center.post(&silent, "chat", "Chat", post("hi", ""), Instant::now(), 20);
    assert_eq!(titles(&center), ["hi", "Ducktape 2 is ready."]);
    center.post_host(
        "Ducktape 3 is ready.",
        "Restart to update.",
        "update/ready",
        30,
    );
    assert_eq!(titles(&center), ["Ducktape 3 is ready.", "hi"]);
    let rows: Vec<_> = center.entries().cloned().collect();
    assert_eq!(
        (rows[0].id, rows[0].count),
        (host.id, 1),
        "in the row's place"
    );
    assert_ne!(rows[0].id, rows[1].id);
    assert!(!rows[1].host());
    assert_eq!(center.unread(), 2);
    assert_eq!(
        center.open(host.id).map(|entry| entry.title),
        Some("Ducktape 3 is ready.".into())
    );
    assert_eq!(center.unread(), 1, "the chain's row is still unread");

    flushed(&mut center, &device);
    settled(&chain);
    let kept = |path: &Path| {
        read_log(path)
            .unwrap()
            .into_iter()
            .map(|entry| entry.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(kept(&device), ["Ducktape 3 is ready."]);
    assert_eq!(kept(&chain), ["hi"]);
    center.open_log("other", None);
    assert_eq!(titles(&center), ["Ducktape 3 is ready."], "another network");
    let mut next = Center::default();
    next.open_device_log(device.clone());
    assert_eq!(
        next.entries().map(|entry| entry.read).collect::<Vec<_>>(),
        [true],
        "the next start reads them back"
    );

    center.withdraw_host("update/ready");
    assert_eq!(center.entries().count(), 0);
    flushed(&mut center, &device);
    assert_eq!(kept(&device), Vec::<String>::new());
}

#[test]
fn a_post_into_another_chains_centre_is_refused() {
    let center = CenterHandle::default();
    center.lock().open_log("held-net#1", None);
    let mut view = crate::runtime::kernel::tests::view();
    view.program = "chat";
    view.connection.chain = "left-net#2".into();
    let payload = methods::encode(&post("ada", "#general"));
    super::post(&mut view, &center, 1, &payload);
    match view.pending.pop() {
        Some(view_wire::Event::Response {
            id: 1,
            result: Err(refusal),
            ..
        }) => assert_eq!(refusal.code, methods::refusal::STALE_CONNECTION),
        other => panic!("{other:?}"),
    }
    assert_eq!(center.lock().entries().count(), 0, "logged anyway");
    view.connection.chain = "held-net#1".into();
    super::post(&mut view, &center, 2, &payload);
    assert_eq!(center.lock().entries().count(), 1);
}

#[test]
fn another_chains_row_click_asks_the_shell() {
    let opened = super::opened(super::Click::Row {
        chain: "never-in-hand#7".into(),
        row: 4,
    });
    assert!(
        matches!(&opened, Some(super::Opened::Row { chain, row: 4 }) if chain == "never-in-hand#7"),
        "opened here"
    );
    let gone = super::opened(super::Click::Row {
        chain: String::new(),
        row: u64::MAX,
    });
    assert!(gone.is_none(), "a row the centre in hand does not hold");
}
