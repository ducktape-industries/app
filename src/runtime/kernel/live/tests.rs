use super::*;
use crate::backend::live::{Code, Ends, Request};
use crate::runtime::kernel::tests::view;
use std::sync::mpsc as std_mpsc;
use wire::methods::{LiveItem, LiveSend, LiveSubscribe};

/// A session the test speaks for: every request it opens, in order, with
/// the ends the node would hold; the refusals in `refusing` come first.
struct Fake(
    Mutex<std_mpsc::Sender<(Ask, Ends)>>,
    Mutex<Vec<wire::Error>>,
);

impl Session for Fake {
    fn open(&self, ask: Ask) -> futures::future::BoxFuture<'static, Result<Request, wire::Error>> {
        if let Some(refused) = self.1.lock().unwrap().pop() {
            return Box::pin(async move { Err(refused) });
        }
        let (request, ends) = client::request(|| {});
        self.0.lock().unwrap().send((ask, ends)).unwrap();
        Box::pin(async move { Ok(request) })
    }
}

/// The live tests share the one session slot: one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A view on a connection to chain `net#1`, its live session a [`Fake`]:
/// the view, and each request it opens as the fake sees it.
fn live_guest() -> (View, std_mpsc::Receiver<(Ask, Ends)>) {
    let mut view = view();
    view.connection = super::super::super::Connection {
        client: Some(RpcClient::new("http://127.0.0.1:1")),
        network: "net".into(),
        chain: "net#1".into(),
        rev: view.connection.rev,
    };
    let (opened, opens) = std_mpsc::channel();
    install(
        view.connection.rev,
        Arc::new(Fake(Mutex::new(opened), Mutex::new(Vec::new()))),
    );
    (view, opens)
}

fn ask(view: &mut View, id: u64, kind: &str, payload: Vec<u8>) {
    view.answer(
        wire::Request {
            id,
            kind: kind.into(),
            payload,
        },
        &None,
    );
}

fn opened(opens: &std_mpsc::Receiver<(Ask, Ends)>) -> (Ask, Ends) {
    opens
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the kernel opened a request")
}

/// The next answer the view's tasks hand it, waited for on the replies'
/// own signal.
fn next(view: &View, changes: &mut tokio::sync::watch::Receiver<()>) -> wire::Event {
    loop {
        let mut events = Vec::new();
        view.replies.drain_into(&mut events).unwrap();
        if let Some(event) = events.into_iter().next() {
            return event;
        }
        handle()
            .block_on(changes.changed())
            .expect("the replies are alive");
    }
}

fn item<T: borsh::BorshDeserialize>(event: wire::Event) -> (u64, Result<T, String>, bool) {
    let wire::Event::Response { id, result, done } = event else {
        panic!("an answer")
    };
    let result = result
        .map(|bytes| methods::decode::<T>(&bytes).unwrap())
        .map_err(|refusal| refusal.code);
    (id, result, done)
}

fn track(name: &str) -> LiveTrack {
    LiveTrack {
        room: "lobby".into(),
        name: name.into(),
    }
}

fn send(view: &mut View, id: u64, object: u32) {
    let send = LiveSend {
        track: track("presence/7"),
        group: 1,
        object,
        body: vec![object as u8],
        end: false,
    };
    ask(view, id, "live.send", methods::encode(&send));
}

/// The headline of the kernel's side: a publication is stamped
/// `<chain>/<module>/<room>`, a send before the node took it waits and goes
/// out once it did, the count reaches the view, an object the node dropped
/// is an item that does not end the track, the node's refusal ends it, and
/// a send after that is refused `live_not_published`. No send is answered
/// otherwise.
#[test]
fn a_publication_is_stamped_and_its_sends_wait_for_the_node() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Datagram,
        priority: 9,
    };
    ask(&mut view, 1, "live.publish", methods::encode(&publish));
    let (asked, mut ends) = opened(&opens);
    assert_eq!(
        asked,
        Ask::Publish {
            track: Track {
                namespace: "net#1/request-test/lobby".into(),
                name: "presence/7".into(),
            },
            mode: Mode::Datagram,
            priority: 9,
        }
    );
    send(&mut view, 2, 0);
    assert_eq!(view.pending, [], "a send is not answered");
    assert!(
        ends.objects.try_recv().is_err(),
        "nothing goes out before the node took it"
    );
    ends.heard.send(Heard::Published).unwrap();
    let out = ends.objects.blocking_recv().unwrap();
    assert_eq!((out.group, out.object, out.body), (1, 0, vec![0]));
    ends.heard.send(Heard::Subscribers(2)).unwrap();
    let (id, count, done) = item::<Publishing>(next(&view, &mut changes));
    assert_eq!(
        (id, count, done),
        (1, Ok(Publishing::Subscribers(2)), false)
    );
    ends.heard.send(Heard::Dropped(Code::TooLarge)).unwrap();
    let (_, dropped, done) = item::<Publishing>(next(&view, &mut changes));
    assert_eq!((dropped, done), (Err(refusal::TOO_LARGE.into()), false));
    ends.heard
        .send(Heard::Closed(wire::Error::new(
            Code::NotGranted.as_str(),
            "the node ended this track",
        )))
        .unwrap();
    let (_, ended, done) = item::<Publishing>(next(&view, &mut changes));
    assert_eq!((ended, done), (Err(refusal::NOT_GRANTED.into()), true));
    // the publication's queue went with its task
    while !ends.objects.is_closed() {
        std::thread::yield_now();
    }
    send(&mut view, 3, 1);
    assert_eq!(
        refusal_of(&mut view, 3),
        Some(refusal::LIVE_NOT_PUBLISHED.into())
    );
}

fn refusal_of(view: &mut View, wanted: u64) -> Option<String> {
    view.pending.iter().find_map(|event| match event {
        wire::Event::Response {
            id,
            result: Err(refusal),
            ..
        } if *id == wanted => Some(refusal.code.clone()),
        _ => None,
    })
}

/// An object over its mode's cap is refused on the publication, which goes
/// on; a send to a track the view does not publish is refused at once.
#[test]
fn a_send_too_large_or_to_no_track_is_refused() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Datagram,
        priority: 128,
    };
    ask(&mut view, 1, "live.publish", methods::encode(&publish));
    let (_, _ends) = opened(&opens);
    let big = LiveSend {
        track: track("presence/7"),
        group: 0,
        object: 0,
        body: vec![0; live_wire::MAX_DATAGRAM_OBJECT + 1],
        end: false,
    };
    ask(&mut view, 2, "live.send", methods::encode(&big));
    let (id, refused, done) = item::<Publishing>(next(&view, &mut changes));
    assert_eq!(
        (id, refused, done),
        (1, Err(refusal::TOO_LARGE.into()), false)
    );
    let elsewhere = LiveSend {
        track: track("cursor/7"),
        ..big
    };
    ask(&mut view, 3, "live.send", methods::encode(&elsewhere));
    assert_eq!(
        refusal_of(&mut view, 3),
        Some(refusal::LIVE_NOT_PUBLISHED.into())
    );
}

/// A subscription hears `Opened` with who publishes, each object and each
/// group's end, a datagram-mode object through the newest slot, and the
/// node's end in the view's word.
#[test]
fn a_subscription_hears_the_track_and_its_end() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let subscribe = LiveSubscribe {
        track: track("doc"),
        from: LiveStart::Latest,
    };
    ask(&mut view, 1, "live.subscribe", methods::encode(&subscribe));
    let (asked, ends) = opened(&opens);
    assert!(
        matches!(asked, Ask::Subscribe { from: Start::Latest, ref track } if track.namespace == "net#1/request-test/lobby" && track.name == "doc"),
        "{asked:?}"
    );
    ends.heard
        .send(Heard::Subscribed {
            by: 7,
            mode: Mode::Datagram,
            latest: Some(3),
        })
        .unwrap();
    let opened_item = LiveItem::Opened {
        by: 7,
        mode: LiveMode::Datagram,
        latest: Some(3),
    };
    assert_eq!(item(next(&view, &mut changes)), (1, Ok(opened_item), false));
    ends.latest.put(Data::Object {
        group: 3,
        object: 0,
        body: vec![5],
    });
    let object = LiveItem::Object {
        group: 3,
        object: 0,
        body: vec![5],
    };
    assert_eq!(item(next(&view, &mut changes)), (1, Ok(object), false));
    handle()
        .block_on(ends.data.send(Data::GroupEnd {
            group: 3,
            complete: false,
        }))
        .unwrap();
    let end = LiveItem::GroupEnd {
        group: 3,
        complete: false,
    };
    assert_eq!(item(next(&view, &mut changes)), (1, Ok(end), false));
    ends.heard
        .send(Heard::Closed(wire::Error::new(
            refusal::PUBLISHER_GONE,
            "the node ended this subscription",
        )))
        .unwrap();
    assert_eq!(
        item::<LiveItem>(next(&view, &mut changes)),
        (1, Err(refusal::PUBLISHER_GONE.into()), true)
    );
}

/// A request the node holds for its grant: each kind hears one `Waiting`
/// item, and goes on when the node answers. A publication's sends wait for
/// it.
#[test]
fn a_request_the_node_holds_hears_not_granted_and_goes_on() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Stream,
        priority: 128,
    };
    ask(&mut view, 1, "live.publish", methods::encode(&publish));
    let (_, mut published) = opened(&opens);
    send(&mut view, 2, 0);
    published.heard.send(Heard::Waiting).unwrap();
    assert_eq!(
        item(next(&view, &mut changes)),
        (1, Ok(Publishing::Waiting), false)
    );
    assert!(published.objects.try_recv().is_err(), "a send waits too");
    published.heard.send(Heard::Published).unwrap();
    let out = published.objects.blocking_recv().unwrap();
    assert_eq!((out.group, out.object), (1, 0));
    published.heard.send(Heard::Subscribers(1)).unwrap();
    assert_eq!(
        item(next(&view, &mut changes)),
        (1, Ok(Publishing::Subscribers(1)), false)
    );

    let subscribe = LiveSubscribe {
        track: track("doc"),
        from: LiveStart::Next,
    };
    ask(&mut view, 3, "live.subscribe", methods::encode(&subscribe));
    let (_, subscribed) = opened(&opens);
    subscribed.heard.send(Heard::Waiting).unwrap();
    assert_eq!(
        item(next(&view, &mut changes)),
        (3, Ok(LiveItem::Waiting), false)
    );
    subscribed
        .heard
        .send(Heard::Subscribed {
            by: 7,
            mode: Mode::Stream,
            latest: None,
        })
        .unwrap();
    let opened_item = LiveItem::Opened {
        by: 7,
        mode: LiveMode::Stream,
        latest: None,
    };
    assert_eq!(item(next(&view, &mut changes)), (3, Ok(opened_item), false));

    ask(
        &mut view,
        4,
        "live.tracks",
        methods::encode(&"lobby".to_owned()),
    );
    let (_, watched) = opened(&opens);
    watched.heard.send(Heard::Waiting).unwrap();
    assert_eq!(
        item(next(&view, &mut changes)),
        (4, Ok(Watching::Waiting), false)
    );
    watched
        .heard
        .send(Heard::Announce {
            track: Track {
                namespace: "net#1/request-test/lobby".into(),
                name: "doc".into(),
            },
            by: 7,
            mode: Mode::Stream,
            up: true,
        })
        .unwrap();
    let announce = LiveAnnounce {
        name: "doc".into(),
        by: 7,
        mode: LiveMode::Stream,
        up: true,
    };
    assert_eq!(
        item(next(&view, &mut changes)),
        (4, Ok(Watching::Announce(announce)), false)
    );
}

/// Every word the node can end a request with is a word a view reads: a
/// view-wire refusal, or `no_account`, which the host asks again
/// (`opened`) and no view hears. A code the live wire gains fails this
/// match until it is given its word.
#[test]
fn every_code_the_node_says_is_a_word_a_view_knows() {
    let codes = [
        Code::NoAccount,
        Code::BadProof,
        Code::NotGranted,
        Code::Unknown,
        Code::Duplicate,
        Code::TooLarge,
        Code::Malformed,
    ];
    for code in codes {
        let word = match code {
            Code::NoAccount => continue,
            Code::BadProof => refusal::BAD_PROOF,
            Code::NotGranted => refusal::NOT_GRANTED,
            Code::Unknown => refusal::UNKNOWN_TRACK,
            Code::Duplicate => refusal::DUPLICATE_TRACK,
            Code::TooLarge => refusal::TOO_LARGE,
            Code::Malformed => refusal::MALFORMED_REQUEST,
        };
        assert_eq!(code.as_str(), word);
    }
}

/// A room's list: the tracks it hears come and go, and a lost session takes
/// down every track it had up before it opens the list again.
#[test]
fn a_lost_session_takes_a_rooms_tracks_down_and_opens_the_list_again() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    ask(
        &mut view,
        1,
        "live.tracks",
        methods::encode(&"lobby".to_owned()),
    );
    let (asked, ends) = opened(&opens);
    assert_eq!(
        asked,
        Ask::Watch {
            namespace: "net#1/request-test/lobby".into()
        }
    );
    let up = |name: &str, up| Heard::Announce {
        track: Track {
            namespace: "net#1/request-test/lobby".into(),
            name: name.into(),
        },
        by: 7,
        mode: Mode::Stream,
        up,
    };
    let announce = |name: &str, up| {
        Watching::Announce(LiveAnnounce {
            name: name.into(),
            by: 7,
            mode: LiveMode::Stream,
            up,
        })
    };
    ends.heard.send(up("echo/7", true)).unwrap();
    assert_eq!(
        item(next(&view, &mut changes)),
        (1, Ok(announce("echo/7", true)), false)
    );
    // the session is lost: what was up goes down, and the list opens again
    drop(ends);
    assert_eq!(
        item(next(&view, &mut changes)),
        (1, Ok(announce("echo/7", false)), false)
    );
    let (again, _ends) = opened(&opens);
    assert_eq!(again, asked);
}

/// A connect inside the network: a live subscription opens again on the
/// new connection's session, from the group the node holds, and is not
/// refused.
#[test]
fn a_reconnect_opens_a_live_subscription_again_on_the_new_session() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let subscribe = LiveSubscribe {
        track: track("doc"),
        from: LiveStart::Next,
    };
    ask(&mut view, 1, "live.subscribe", methods::encode(&subscribe));
    let (first, _ends) = opened(&opens);
    assert!(matches!(
        first,
        Ask::Subscribe {
            from: Start::Next,
            ..
        }
    ));
    let rev = view.connection.rev + 1;
    let (reopened, reopens) = std_mpsc::channel();
    install(
        rev,
        Arc::new(Fake(Mutex::new(reopened), Mutex::new(Vec::new()))),
    );
    let connection = super::super::super::Connection {
        rev,
        ..view.connection.clone()
    };
    view.reconnect(&connection);
    assert_eq!(view.pending, [], "the subscription was not refused");
    let (again, _ends) = opened(&reopens);
    assert!(
        matches!(
            again,
            Ask::Subscribe {
                from: Start::Latest,
                ..
            }
        ),
        "{again:?}"
    );
}

/// A room or a track name the node would not take is refused here.
#[test]
fn a_room_or_a_name_outside_the_rule_is_malformed() {
    let _serial = serial();
    let (mut view, _opens) = live_guest();
    for (id, room, name) in [(1, "", "doc"), (2, "lobby", "a//b"), (3, "lob\nby", "doc")] {
        let subscribe = LiveSubscribe {
            track: LiveTrack {
                room: room.into(),
                name: name.into(),
            },
            from: LiveStart::Latest,
        };
        ask(&mut view, id, "live.subscribe", methods::encode(&subscribe));
        assert_eq!(
            refusal_of(&mut view, id),
            Some(refusal::MALFORMED_REQUEST.into()),
            "{room:?} {name:?}"
        );
    }
}

/// A view's first live request puts the consent line on its window and
/// asks nothing: the request goes on to the node.
#[test]
fn a_first_live_request_puts_the_line_on_the_window() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    ask(
        &mut view,
        1,
        "live.tracks",
        methods::encode(&"lobby".to_owned()),
    );
    assert!(
        crate::runtime::notify::center()
            .lock()
            .sharing
            .contains("request-test"),
        "the view went live and its window said nothing"
    );
    assert!(matches!(opened(&opens).0, Ask::Watch { .. }));
}

/// A key whose account the node has not confirmed yet is refused
/// `no_account`: the subscription waits and asks again rather than ending,
/// and opens once the node knows the account.
#[test]
fn a_session_refused_no_account_is_asked_again() {
    let _serial = serial();
    let (mut view, opens) = live_guest();
    let (opened_again, opens_again) = std_mpsc::channel();
    let fake = Fake(
        Mutex::new(opened_again),
        Mutex::new(vec![wire::Error::new(
            Code::NoAccount.as_str(),
            "the node refused this key a live session",
        )]),
    );
    install(view.connection.rev, Arc::new(fake));
    drop(opens);
    let mut changes = view.replies.changes();
    let subscribe = LiveSubscribe {
        track: track("presence/7"),
        from: LiveStart::Latest,
    };
    ask(&mut view, 1, "live.subscribe", methods::encode(&subscribe));
    let (asked, ends) = opened(&opens_again);
    assert!(matches!(asked, Ask::Subscribe { .. }));
    let mut pending = Vec::new();
    view.replies.drain_into(&mut pending).unwrap();
    assert!(
        pending.is_empty(),
        "the refusal reached the view: {pending:?}"
    );
    ends.heard
        .send(Heard::Subscribed {
            by: 7,
            mode: Mode::Datagram,
            latest: None,
        })
        .unwrap();
    let (id, opened, done) = item::<LiveItem>(next(&view, &mut changes));
    assert_eq!(id, 1);
    assert!(matches!(opened, Ok(LiveItem::Opened { by: 7, .. })));
    assert!(!done);
}
