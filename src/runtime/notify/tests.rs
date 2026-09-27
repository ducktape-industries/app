use std::time::{Duration, Instant};

use super::center::{Center, MAX_AGE, MAX_ENTRIES};
use super::settings::{BURST_PREF, FRONT_PREF, NOTIFY_PREF, Permission, Settings, VIEWS_PREF};
use super::{MAX_TEXT, in_order, shortened};
use crate::runtime::WindowKey;
use view_wire::methods::{self, Delivery, Notification};

fn post(title: &str, tag: &str) -> Notification {
    Notification {
        title: title.into(),
        body: "b".into(),
        tag: tag.into(),
        link: String::new(),
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
    assert!(center.asking("chat") && center.entries().count() == 1);

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
    assert!(!center.asking("chat"));
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

/// The log keeps 500 rows and thirty days, whichever is fewer.
#[test]
fn the_log_is_capped_by_count_and_age() {
    let now = Instant::now();
    let mut center = Center::default();
    let silent = settings(Some(Permission::Silent));
    for nth in 0..(MAX_ENTRIES as i64 + 20) {
        center.post(
            &silent,
            "chat",
            "Chat",
            post(&nth.to_string(), ""),
            now,
            nth,
        );
    }
    assert_eq!(center.entries().count(), MAX_ENTRIES);
    assert_eq!(center.entries().last().unwrap().title, "20");
    // thirty days on from the row at 100: everything before it ages out
    center.post(
        &silent,
        "chat",
        "Chat",
        post("late", ""),
        now,
        100 + MAX_AGE,
    );
    assert_eq!(center.entries().count(), 520 - 100 + 1);
    assert_eq!(center.entries().last().unwrap().title, "100");
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
/// with no words, or a link that is not `duck://`, is refused; long text
/// is cut on a char boundary, and a cut link is dropped.
#[test]
fn a_post_is_words_a_tag_and_a_duck_link() {
    let full = Notification {
        title: "a".into(),
        body: "b".into(),
        tag: "t".into(),
        link: "duck://chat/room".into(),
    };
    let mut bytes = methods::encode(&full);
    assert_eq!(methods::decode::<Notification>(&bytes).unwrap(), full);
    bytes.extend_from_slice(b"icon");
    assert!(methods::decode::<Notification>(&bytes).is_err());
    assert!(shortened(Notification::default()).is_err());
    assert!(
        shortened(Notification {
            link: "https://example.com".into(),
            ..full.clone()
        })
        .is_err()
    );
    let long = shortened(Notification {
        title: "가".repeat(MAX_TEXT),
        body: "b".repeat(MAX_TEXT * 2),
        link: format!("duck://{}", "x".repeat(MAX_TEXT)),
        ..full
    })
    .unwrap();
    assert!(long.title.len() <= MAX_TEXT && long.title.chars().all(|c| c == '가'));
    assert_eq!(long.body.len(), MAX_TEXT);
    assert!(long.link.is_empty(), "a cut link goes nowhere");
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
