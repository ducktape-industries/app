use super::*;
use crate::backend::live::{Code, Ends, Request};
use crate::runtime::kernel::tests::guest;
use std::sync::mpsc as std_mpsc;
use wire::methods::{LiveItem, LiveSend, LiveSubscribe};

/// A session the test speaks for: every request it opens, in order, with
/// the ends the node would hold.
struct Fake(Mutex<std_mpsc::Sender<(Ask, Ends)>>);

impl Session for Fake {
    fn open(&self, ask: Ask) -> futures::future::BoxFuture<'static, Result<Request, wire::Error>> {
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

/// A guest on a connection to chain `net#1`, its live session a [`Fake`]:
/// the guest, and each request it opens as the fake sees it.
fn live_guest() -> (Guest, std_mpsc::Receiver<(Ask, Ends)>) {
    let mut guest = guest();
    guest.connection = super::super::super::Connection {
        client: Some(RpcClient::new("http://127.0.0.1:1")),
        network: "net".into(),
        chain: "net#1".into(),
        rev: guest.connection.rev,
    };
    let (opened, opens) = std_mpsc::channel();
    install(guest.connection.rev, Arc::new(Fake(Mutex::new(opened))));
    (guest, opens)
}

fn ask(guest: &mut Guest, id: u64, kind: &str, payload: Vec<u8>) {
    guest.answer(
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

/// The next answer the guest's tasks hand it, waited for on the replies'
/// own signal.
fn next(guest: &Guest, changes: &mut tokio::sync::watch::Receiver<()>) -> wire::Event {
    loop {
        let mut events = Vec::new();
        guest.replies.drain_into(&mut events).unwrap();
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

fn send(guest: &mut Guest, id: u64, object: u32) {
    let send = LiveSend {
        track: track("presence/7"),
        group: 1,
        object,
        body: vec![object as u8],
        end: false,
    };
    ask(guest, id, "live.send", methods::encode(&send));
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
    let (mut guest, opens) = live_guest();
    let mut changes = guest.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Datagram,
        priority: 9,
    };
    ask(&mut guest, 1, "live.publish", methods::encode(&publish));
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
    send(&mut guest, 2, 0);
    assert_eq!(guest.pending, [], "a send is not answered");
    assert!(
        ends.objects.try_recv().is_err(),
        "nothing goes out before the node took it"
    );
    ends.heard.send(Heard::Published).unwrap();
    let out = ends.objects.blocking_recv().unwrap();
    assert_eq!((out.group, out.object, out.body), (1, 0, vec![0]));
    ends.heard.send(Heard::Subscribers(2)).unwrap();
    let (id, count, done) = item::<Publishing>(next(&guest, &mut changes));
    assert_eq!(
        (id, count, done),
        (1, Ok(Publishing::Subscribers(2)), false)
    );
    ends.heard.send(Heard::Dropped(Code::Rate)).unwrap();
    let (_, dropped, done) = item::<Publishing>(next(&guest, &mut changes));
    assert_eq!((dropped, done), (Err(refusal::RATE_LIMIT.into()), false));
    ends.heard
        .send(Heard::Closed(wire::Error::new(
            Code::NotGranted.as_str(),
            "the node ended this track",
        )))
        .unwrap();
    let (_, ended, done) = item::<Publishing>(next(&guest, &mut changes));
    assert_eq!((ended, done), (Err(refusal::NOT_GRANTED.into()), true));
    // the publication's queue went with its task
    while !ends.objects.is_closed() {
        std::thread::yield_now();
    }
    send(&mut guest, 3, 1);
    assert_eq!(
        refusal_of(&mut guest, 3),
        Some(refusal::LIVE_NOT_PUBLISHED.into())
    );
}

fn refusal_of(guest: &mut Guest, wanted: u64) -> Option<String> {
    guest.pending.iter().find_map(|event| match event {
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
    let (mut guest, opens) = live_guest();
    let mut changes = guest.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Datagram,
        priority: 128,
    };
    ask(&mut guest, 1, "live.publish", methods::encode(&publish));
    let (_, _ends) = opened(&opens);
    let big = LiveSend {
        track: track("presence/7"),
        group: 0,
        object: 0,
        body: vec![0; live_wire::MAX_DATAGRAM_OBJECT + 1],
        end: false,
    };
    ask(&mut guest, 2, "live.send", methods::encode(&big));
    let (id, refused, done) = item::<Publishing>(next(&guest, &mut changes));
    assert_eq!(
        (id, refused, done),
        (1, Err(refusal::TOO_LARGE.into()), false)
    );
    let elsewhere = LiveSend {
        track: track("cursor/7"),
        ..big
    };
    ask(&mut guest, 3, "live.send", methods::encode(&elsewhere));
    assert_eq!(
        refusal_of(&mut guest, 3),
        Some(refusal::LIVE_NOT_PUBLISHED.into())
    );
}

/// A subscription hears `Opened` with who publishes, each object and each
/// group's end, a datagram-mode object through the newest slot, and the
/// node's end in the view's word.
#[test]
fn a_subscription_hears_the_track_and_its_end() {
    let _serial = serial();
    let (mut guest, opens) = live_guest();
    let mut changes = guest.replies.changes();
    let subscribe = LiveSubscribe {
        track: track("doc"),
        latest: true,
    };
    ask(&mut guest, 1, "live.subscribe", methods::encode(&subscribe));
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
    assert_eq!(
        item(next(&guest, &mut changes)),
        (1, Ok(opened_item), false)
    );
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
    assert_eq!(item(next(&guest, &mut changes)), (1, Ok(object), false));
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
    assert_eq!(item(next(&guest, &mut changes)), (1, Ok(end), false));
    ends.heard
        .send(Heard::Closed(wire::Error::new(
            refusal::PUBLISHER_GONE,
            "the node ended this subscription",
        )))
        .unwrap();
    assert_eq!(
        item::<LiveItem>(next(&guest, &mut changes)),
        (1, Err(refusal::PUBLISHER_GONE.into()), true)
    );
}

/// A room's list: the tracks it hears come and go, and a lost session takes
/// down every track it had up before it opens the list again.
#[test]
fn a_lost_session_takes_a_rooms_tracks_down_and_opens_the_list_again() {
    let _serial = serial();
    let (mut guest, opens) = live_guest();
    let mut changes = guest.replies.changes();
    ask(
        &mut guest,
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
    let announce = |name: &str, up| LiveAnnounce {
        name: name.into(),
        by: 7,
        mode: LiveMode::Stream,
        up,
    };
    ends.heard.send(up("echo/7", true)).unwrap();
    assert_eq!(
        item(next(&guest, &mut changes)),
        (1, Ok(announce("echo/7", true)), false)
    );
    // the session is lost: what was up goes down, and the list opens again
    drop(ends);
    assert_eq!(
        item(next(&guest, &mut changes)),
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
    let (mut guest, opens) = live_guest();
    let subscribe = LiveSubscribe {
        track: track("doc"),
        latest: false,
    };
    ask(&mut guest, 1, "live.subscribe", methods::encode(&subscribe));
    let (first, _ends) = opened(&opens);
    assert!(matches!(
        first,
        Ask::Subscribe {
            from: Start::Next,
            ..
        }
    ));
    let rev = guest.connection.rev + 1;
    let (reopened, reopens) = std_mpsc::channel();
    install(rev, Arc::new(Fake(Mutex::new(reopened))));
    let connection = super::super::super::Connection {
        rev,
        ..guest.connection.clone()
    };
    guest.reconnect(&connection);
    assert_eq!(guest.pending, [], "the subscription was not refused");
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
    let (mut guest, _opens) = live_guest();
    for (id, room, name) in [(1, "", "doc"), (2, "lobby", "a//b"), (3, "lob\nby", "doc")] {
        let subscribe = LiveSubscribe {
            track: LiveTrack {
                room: room.into(),
                name: name.into(),
            },
            latest: true,
        };
        ask(
            &mut guest,
            id,
            "live.subscribe",
            methods::encode(&subscribe),
        );
        assert_eq!(
            refusal_of(&mut guest, id),
            Some(refusal::MALFORMED_REQUEST.into()),
            "{room:?} {name:?}"
        );
    }
}
