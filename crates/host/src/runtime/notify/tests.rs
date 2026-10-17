use std::time::{Duration, Instant};

use std::path::Path;

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

/// The person's word decides first: none yet logs and asks, Block drops
/// without a trace, Silent logs, Allow raises a banner.
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

    // "Not now" keeps the bar away for the run
    center.asking.clear();
    center.not_now.insert("chat".into());
    center.post(&settings(None), "chat", "Chat", post("f", ""), now, 0);
    assert!(!center.asking.contains("chat"));
}

/// The window in front gets no banner unless the person asked for it.
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
    // another window losing focus leaves the front alone; this one's clears it
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

/// A catch-up on a host with no one to ask (pushd's recipients: `unasked`
/// Allow): twenty missed mentions in two rooms, posted in one instant,
/// raise the burst limit's banners and then only the one standing "N more"
/// banner, replaced under one tag, so a device holds two banners, not
/// twenty; the centre folds them into one row per room, and asks no one.
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

/// A burst of three a minute: the fourth is logged and raises one "more"
/// banner keyed by the view, counting up; a token back, a banner again.
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
    // another view has its own bucket
    let mut forge = allowed.clone();
    forge.views.insert("forge".into(), Permission::Allow);
    assert_eq!(
        center
            .post(&forge, "forge", "Forge", post("f", ""), start, 5)
            .0,
        Delivery::Banner
    );
    // twenty seconds refill one token at three a minute
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

/// One tag, one row: it counts what folded into it, comes back to the
/// top unread, and carries the latest words.
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

/// A view reading a tag reads its own rows under it: another tag, an
/// untagged row and another view's row under the same tag stay unread.
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

/// The log keeps every row, however many and however old, until the
/// person clears it.
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
    let read = Settings::of(&serde_json::json!({}));
    assert_eq!((read.banners, read.in_front, read.burst), (true, false, 6));
    let read = Settings::of(&serde_json::json!({
        NOTIFY_PREF: false, FRONT_PREF: "show", BURST_PREF: 12,
        VIEWS_PREF: { "chat": "Silent", "forge": "nonsense" },
    }));
    assert_eq!((read.banners, read.in_front, read.burst), (false, true, 12));
    assert_eq!(read.views.len(), 1);
    assert_eq!(Settings::of(&serde_json::json!({ BURST_PREF: 7 })).burst, 6);
}

/// A post round-trips as borsh and trailing bytes are refused; a post
/// with no words, or a link or end link that is not `duck://`, is
/// refused; long text and a long link are kept whole.
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

/// A view's banners leave in the order they came, however long each
/// takes: the first one holds until every other is queued behind it.
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
                // held until all ten are queued: nothing may pass it
                let _ = gate.recv();
            }
            tell.send(nth).unwrap();
        });
    }
    drop(open);
    drop(tell);
    assert_eq!(told.iter().collect::<Vec<_>>(), (0..10).collect::<Vec<_>>());
}

/// A log cut short is set aside whole as `.bad`, the centre starts with
/// no rows, and the next post is saved as a fresh log, not over the old one.
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

/// A log this device cannot read is not taken for an empty one: the centre
/// starts empty, and the next post does not save over the old rows.
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

/// `center`'s turn over: its log handed to the disk, and landed.
fn flushed(center: &mut Center, path: &Path) {
    center.flush();
    settled(path);
}

/// Every write queued for the log at `path` has landed: they run one after
/// another, so a job queued behind them runs last.
fn settled(path: &Path) {
    let (tell, told) = std::sync::mpsc::channel();
    in_order(&path.to_string_lossy(), move || tell.send(()).unwrap());
    told.recv_timeout(Duration::from_secs(10)).unwrap();
}

/// A log the centre handed over is on disk once the quit drain is done,
/// though the write was still queued behind another job when it began.
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

/// The quit hook's order: the centre's rows are handed over before the
/// drain begins, so the drain waits for them too.
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

/// A view posting 600 notices leaves the other views' rows where they
/// were, and a view the person has not answered keeps every row it posts.
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

/// 256 posts in one redraw: the prefs are read once, not per post, and
/// the log is written once, at the end of the redraw and off the window
/// thread, not per post.
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
    let _on = crate::perf::on_for_test();
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
    // the end of the redraw (`View::redraw`)
    center.lock().flush();
    settled(&path);
    let saves = &crate::perf::snapshot(false)["shell"]["io.notify_save"]["n"];
    assert_eq!(saves.as_u64(), Some(1), "one write for the redraw's posts");
    let kept = read_log(&path).unwrap();
    assert_eq!(kept.len(), 256);
    assert_eq!(kept.last().unwrap().title, "255");
}

/// A banner's request id is new across processes, not only within one:
/// macOS replaces a delivered banner under the id of a new request, so ids
/// that restarted with each process took down, unseen, what an earlier one
/// left standing.
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

/// The consent line: a view's first live track puts it on its window, a
/// second does not draw it again, and once put away it stays away for the
/// run.
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

/// A call from `chat` under `tag`, standing until `until`, whose end from
/// the system's call screen opens `duck://chat/<tag>/leave`.
fn call(tag: &str, until: u64) -> Notification {
    Notification {
        kind: NoticeKind::Call,
        until,
        end_link: format!("duck://chat/{tag}/leave"),
        ..post("Ada is calling", tag)
    }
}

/// A view's `notify.post` payload is what the centre judges: a wire call
/// with a tag and an `until` rings past the burst as a call, keeps its
/// end link for the carrier, and is over at its `until`.
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

/// A call rings past the burst, never as "N more"; before the person said
/// anything about its view every call rings, with the permission bar.
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
    // while it stands, the program's next call is logged
    let (posted, banner) = center.post(&unasked, "chat", "Chat", call("call/1b", 100), now, 50);
    assert_eq!((posted, banner), (Delivery::Logged, None));
    // the first call is over: the next one rings too, the bar still asking
    let (posted, banner) = center.post(&unasked, "chat", "Chat", call("call/2", 200), now, 101);
    assert_eq!(
        (posted, banner.map(|notice| notice.tag)),
        (Delivery::Banner, Some("chat/call/2".to_owned())),
        "a second call rang unasked"
    );
    assert!(center.asking.contains("chat"));
    // "Not now" puts the bar away, not the ring
    super::settings::not_now(&mut center, "chat");
    let (posted, _) = center.post(&unasked, "chat", "Chat", call("call/2b", 300), now, 201);
    assert_eq!(posted, Delivery::Banner);
    assert!(!center.asking.contains("chat"));
    // a plain notice of a view not yet asked about stays in the list
    let (posted, banner) = center.post(&unasked, "chat", "Chat", post("m", ""), now, 102);
    assert_eq!((posted, banner), (Delivery::Logged, None));
    let (posted, banner) = center.post(&unasked, "forge", "Forge", call("call/3", 100), now, 2);
    assert_eq!(posted, Delivery::Banner, "another view rings its own");
    assert!(banner.is_some());
    // the person's word still decides: Silent logs, Block drops
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

/// A notice past its `until` is logged and raises nothing: a call that is
/// over never rings.
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

/// A banner with an `until` is over at it, once, unless a later banner
/// took its tag (a call rung again) or the view said it was seen; a plain
/// notice under the tag ends the standing one's timer too.
#[test]
fn a_banner_is_over_at_its_until_unless_its_tag_moved_on() {
    let now = Instant::now();
    let mut center = Center::default();
    let allowed = settings(Some(Permission::Allow));
    center.post(&allowed, "chat", "Chat", call("call/1", 100), now, 0);
    assert!(center.ring_over("chat/call/1", 100));
    assert!(!center.ring_over("chat/call/1", 100), "over twice");

    // rung again past its until, before the first ring's timer ran
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

/// One call stands per program: another of its calls while it rings,
/// under any tag, is logged only; once it is seen or past its `until` the
/// next rings. A call with no `until` or no tag is a plain notice.
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
    // raised past the standing call/3, as notices go: through the burst
    let plain = |(_, banner): (Delivery, Option<Notice>)| banner.map(|notice| notice.kind);
    assert_eq!(
        plain(ring("chat", "call/4", 0, 101)),
        Some(NoticeKind::Notice)
    );
    assert_eq!(plain(ring("chat", "", 300, 101)), Some(NoticeKind::Notice));
}

/// A call declined on its banner stands no more: its program's next call
/// rings at once, not at the declined one's `until`, and the declined
/// one's timer takes nothing down.
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

/// An `until` past any time the clock can name sets no timer: adding it
/// panicked, and a phone's panic aborts the app.
#[test]
fn an_until_past_the_clock_sets_no_timer() {
    let timer = super::withdraw_at(
        CenterHandle::default(),
        None,
        "chat",
        "chat/call/1".into(),
        u64::MAX,
    );
    crate::runtime::kernel::handle()
        .block_on(timer)
        .expect("the timer panicked");
}

/// The host's timer takes a call's banner down at its `until`, and only
/// the banner it was set for.
#[test]
fn a_calls_banner_is_withdrawn_at_its_until() {
    let center = CenterHandle::default();
    let allowed = settings(Some(Permission::Allow));
    // whole seconds: at least one second off, whenever in this one the test runs
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
    let timer = super::withdraw_at(center.clone(), None, "chat", banner.tag.clone(), until);
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

/// The host's own rows are the device's: there before any network is in
/// hand and through a switch to another, merged into the chain's by when
/// they came. A newer one under the tag takes the row's place, unread and
/// counted once; a row is picked by an id no chain row has; they are saved
/// to their own file, never the chain's, and read back off it; withdrawn,
/// the row goes.
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
    center.post_host("Ducktape 2 is ready.", "Restart to update.", "update", 10);
    assert_eq!(
        titles(&center),
        ["Ducktape 2 is ready."],
        "before any network"
    );
    let host = center.entries().next().unwrap().clone();
    assert!(host.host());
    assert_eq!((host.program.as_str(), host.tag.as_str()), (HOST, "update"));

    center.open_log("dev", Some(chain.clone()));
    let silent = settings(Some(Permission::Silent));
    center.post(&silent, "chat", "Chat", post("hi", ""), Instant::now(), 20);
    assert_eq!(titles(&center), ["hi", "Ducktape 2 is ready."]);
    center.post_host(
        "Ducktape 2 is installed.",
        "It runs from now on.",
        "update",
        30,
    );
    assert_eq!(titles(&center), ["Ducktape 2 is installed.", "hi"]);
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
        Some("Ducktape 2 is installed.".into())
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
    assert_eq!(kept(&device), ["Ducktape 2 is installed."]);
    assert_eq!(kept(&chain), ["hi"]);
    center.open_log("other", None);
    assert_eq!(
        titles(&center),
        ["Ducktape 2 is installed."],
        "another network"
    );
    let mut next = Center::default();
    next.open_device_log(device.clone());
    assert_eq!(
        next.entries().map(|entry| entry.read).collect::<Vec<_>>(),
        [true],
        "the next start reads them back"
    );

    center.withdraw_host("update");
    assert_eq!(center.entries().count(), 0);
    flushed(&mut center, &device);
    assert_eq!(kept(&device), Vec::<String>::new());
}
