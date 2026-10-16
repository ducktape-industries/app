use super::*;
use crate::backend::live::{Code, Ends, Out, Request};
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
        let (request, ends) = crate::backend::live::request(|| {});
        self.0.lock().unwrap().send((ask, ends)).unwrap();
        Box::pin(async move { Ok(request) })
    }
}

/// A view on a connection to chain `net#1`, its live session a [`Fake`]:
/// the view, and each request it opens as the fake sees it.
fn live_guest() -> (View, std_mpsc::Receiver<(Ask, Ends)>) {
    live_guest_refusing(Vec::new())
}

/// [`live_guest`], its fake refusing the first opens with `refusing`.
fn live_guest_refusing(refusing: Vec<wire::Error>) -> (View, std_mpsc::Receiver<(Ask, Ends)>) {
    let mut view = view();
    let (opened, opens) = std_mpsc::channel();
    view.connection = fake_connection(view.connection.rev, opened, refusing);
    (view, opens)
}

fn fake_connection(
    rev: u64,
    opened: std_mpsc::Sender<(Ask, Ends)>,
    refusing: Vec<wire::Error>,
) -> super::super::super::Connection {
    super::super::super::Connection {
        client: Some(RpcClient::new("http://127.0.0.1:1")),
        network: "net".into(),
        chain: "net#1".into(),
        rev,
        live: Some(Arc::new(Fake(Mutex::new(opened), Mutex::new(refusing)))),
    }
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
        view.replies.drain_into(&mut events);
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
/// `<chain>/<program>/<room>`, a send before the node took it waits and goes
/// out once it did, the count reaches the view, an object the node dropped
/// is an item that does not end the track, the node's refusal ends it, and
/// a send after that is refused `live_not_published`. No send is answered
/// otherwise.
#[test]
fn a_publication_is_stamped_and_its_sends_wait_for_the_node() {
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Datagram,
        priority: 9,
    };
    ask(&mut view, 1, "live.publish", methods::encode(&publish));
    let (asked, ends) = opened(&opens);
    let Ask::Publish {
        track: stamped,
        mode,
        priority,
        objects,
    } = asked
    else {
        panic!("a publication")
    };
    assert_eq!(
        (stamped, mode, priority),
        (
            Track {
                namespace: "net#1/request-test/lobby".into(),
                name: "presence/7".into(),
            },
            Mode::Datagram,
            9
        )
    );
    send(&mut view, 2, 0);
    assert_eq!(view.pending, [], "a send is not answered");
    let Some(Out::Object(out)) = handle().block_on(objects.take()) else {
        panic!("the send is in the publication's queue")
    };
    assert_eq!((out.group, out.object, out.body), (1, 0, vec![0]));
    ends.heard.send(Heard::Published).unwrap();
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
    // the publication's queue closed with its task
    assert!(handle().block_on(objects.take()).is_none());
    send(&mut view, 3, 1);
    assert_eq!(
        refusal_of(&mut view, 3),
        Some(refusal::LIVE_NOT_PUBLISHED.into())
    );
}

/// A stream publication's queue at its capacity: the group is abandoned
/// whole, never an object. The view hears `Publishing::Cut` once, the
/// queued rest of the group goes and the writer is told to reset its
/// stream, later sends of the group are dropped quiet, and a newer group
/// starts clean. A second `live.publish` of the track is refused
/// `duplicate_track` by the view's own host.
#[test]
fn a_full_stream_queue_cuts_the_group_whole_and_a_newer_one_starts_clean() {
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Stream,
        priority: 128,
    };
    ask(&mut view, 1, "live.publish", methods::encode(&publish));
    let (asked, _ends) = opened(&opens);
    let Ask::Publish { objects, .. } = asked else {
        panic!("a publication")
    };
    ask(&mut view, 2, "live.publish", methods::encode(&publish));
    assert_eq!(
        refusal_of(&mut view, 2),
        Some(refusal::DUPLICATE_TRACK.into())
    );
    for object in 0..=crate::backend::live::OUTBOX as u32 + 1 {
        send(&mut view, 10 + u64::from(object), object);
    }
    assert_eq!(
        item(next(&view, &mut changes)),
        (1, Ok(Publishing::Cut { group: 1 }), false)
    );
    let mut rest = Vec::new();
    view.replies.drain_into(&mut rest);
    assert_eq!(rest, [], "the cut is told once");
    let next_send = LiveSend {
        track: track("presence/7"),
        group: 2,
        object: 0,
        body: vec![9],
        end: false,
    };
    ask(&mut view, 99, "live.send", methods::encode(&next_send));
    assert_eq!(
        handle().block_on(objects.take()),
        Some(Out::Cut(1)),
        "the queued rest of the cut group goes with it"
    );
    let Some(Out::Object(out)) = handle().block_on(objects.take()) else {
        panic!("the newer group goes out")
    };
    assert_eq!((out.group, out.body), (2, vec![9]));
    assert!(
        view.pending
            .iter()
            .all(|event| matches!(event, wire::Event::Response { id: 2, .. })),
        "a send was answered: {:?}",
        view.pending
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
fn a_request_the_node_holds_hears_it_waits_and_goes_on() {
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    let publish = LivePublish {
        track: track("presence/7"),
        mode: LiveMode::Stream,
        priority: 128,
    };
    ask(&mut view, 1, "live.publish", methods::encode(&publish));
    let (_, published) = opened(&opens);
    send(&mut view, 2, 0);
    published.heard.send(Heard::Waiting).unwrap();
    assert_eq!(
        item(next(&view, &mut changes)),
        (1, Ok(Publishing::Waiting), false)
    );
    published.heard.send(Heard::Published).unwrap();
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
/// view-wire refusal. A code the live wire gains fails this match until it
/// is given its word.
#[test]
fn every_code_the_node_says_is_a_word_a_view_knows() {
    let codes = [
        Code::BadProof,
        Code::NotGranted,
        Code::Unknown,
        Code::Duplicate,
        Code::TooLarge,
        Code::Malformed,
    ];
    for code in codes {
        let word = match code {
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

/// A connect inside the network: a room's list says every track it had up
/// went down before it opens again on the new connection's session, as it
/// does when the session is lost.
#[test]
fn a_reconnect_inside_the_network_takes_a_rooms_tracks_down_first() {
    let (mut view, opens) = live_guest();
    let mut changes = view.replies.changes();
    ask(
        &mut view,
        1,
        "live.tracks",
        methods::encode(&"lobby".to_owned()),
    );
    let (_asked, ends) = opened(&opens);
    ends.heard
        .send(Heard::Announce {
            track: Track {
                namespace: "net#1/request-test/lobby".into(),
                name: "echo/7".into(),
            },
            by: 7,
            mode: Mode::Stream,
            up: true,
        })
        .unwrap();
    let (_, got, _) = item::<Watching>(next(&view, &mut changes));
    assert!(matches!(
        got,
        Ok(Watching::Announce(LiveAnnounce { up: true, .. }))
    ));
    let (reopened, reopens) = std_mpsc::channel();
    let connection = fake_connection(view.connection.rev + 1, reopened, Vec::new());
    view.reconnect(&connection);
    let (again, _ends) = opened(&reopens);
    assert!(matches!(again, Ask::Watch { .. }));
    let mut events = Vec::new();
    view.replies.drain_into(&mut events);
    let downs: Vec<String> = events
        .into_iter()
        .filter_map(|event| match item::<Watching>(event) {
            (
                1,
                Ok(Watching::Announce(LiveAnnounce {
                    up: false, name, ..
                })),
                false,
            ) => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(downs, ["echo/7"], "echo/7 went down on the reconnect");
}

/// A connect inside the network: a live subscription opens again on the
/// new connection's session, from the group the node holds, and is not
/// refused.
#[test]
fn a_reconnect_opens_a_live_subscription_again_on_the_new_session() {
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
    let (reopened, reopens) = std_mpsc::channel();
    let connection = fake_connection(view.connection.rev + 1, reopened, Vec::new());
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

// ---------- the tracks the host runs for a view's pieces ----------

/// A [`HostLive`] on a fake session for program `huddle` on chain `net#1`:
/// it, and each request it opens as the fake sees it.
fn host_live() -> (HostLive, std_mpsc::Receiver<(Ask, Ends)>) {
    let (opened, opens) = std_mpsc::channel();
    let session: Arc<dyn Session> = Arc::new(Fake(Mutex::new(opened), Mutex::new(Vec::new())));
    (HostLive::on(session, "net#1", "huddle"), opens)
}

fn heard_soon(publication: &mut HostPublication) -> TrackHeard {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(heard) = publication.heard() {
            return heard;
        }
        assert!(std::time::Instant::now() < deadline, "nothing heard");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A host-run publication is stamped as a view's, in the mode and at the
/// priority the host picks; its objects go into the ask's queue; what the
/// node says reaches the piece as its track's state (`Published` is `Up`
/// with no subscribers); the node's refusal ends it, and it is published
/// again after the hold.
#[test]
fn a_host_publication_is_stamped_fed_and_published_again_after_a_refusal() {
    let (live, opens) = host_live();
    let mut publication = live.publish(&track("cam/7"), Mode::Stream, 128);
    let (asked, ends) = opened(&opens);
    let Ask::Publish {
        track: stamped,
        mode,
        priority,
        objects,
    } = asked
    else {
        panic!("a publication")
    };
    assert_eq!(stamped.namespace, "net#1/huddle/lobby");
    assert_eq!(stamped.name, "cam/7");
    assert_eq!((mode, priority), (Mode::Stream, 128));
    let pushed = publication.push(Object {
        group: 0,
        object: 0,
        body: vec![1, 2, 3],
        end: false,
    });
    assert_eq!(pushed, Pushed::Queued);
    let Some(Out::Object(out)) = handle().block_on(objects.take()) else {
        panic!("the object queued")
    };
    assert_eq!(out.body, vec![1, 2, 3]);
    ends.heard.send(Heard::Waiting).unwrap();
    assert_eq!(heard_soon(&mut publication), TrackHeard::Waiting);
    ends.heard.send(Heard::Published).unwrap();
    assert_eq!(
        heard_soon(&mut publication),
        TrackHeard::Up { subscribers: 0 }
    );
    ends.heard.send(Heard::Subscribers(3)).unwrap();
    assert_eq!(
        heard_soon(&mut publication),
        TrackHeard::Up { subscribers: 3 }
    );
    ends.heard
        .send(Heard::Closed(wire::Error::new(
            refusal::NOT_GRANTED,
            "no row",
        )))
        .unwrap();
    assert_eq!(
        heard_soon(&mut publication),
        TrackHeard::Ended {
            code: refusal::NOT_GRANTED.into()
        }
    );
    // published again, on the same queue, after the hold
    let (again, _ends) = opens
        .recv_timeout(HOLD + std::time::Duration::from_secs(3))
        .expect("published again");
    assert!(matches!(again, Ask::Publish { objects: same, .. } if Arc::ptr_eq(&same, &objects)));
    drop(publication);
    assert!(
        handle().block_on(objects.take()).is_none(),
        "dropped: the queue closes"
    );
}

/// A host-run subscription is stamped as a view's and starts from the
/// group the node holds; what it hears and its objects come as one
/// stream; ended by the node for the track unknown, it watches the room
/// and subscribes again on the node's announce of the track.
#[test]
fn a_host_subscription_streams_items_and_subscribes_again_on_the_announce() {
    let (live, opens) = host_live();
    let mut subscription = live.subscribe(&track("cam/7"));
    let (asked, ends) = opened(&opens);
    let Ask::Subscribe {
        track: stamped,
        from,
    } = asked
    else {
        panic!("a subscription")
    };
    assert_eq!(stamped.namespace, "net#1/huddle/lobby");
    assert_eq!(from, Start::Latest);
    ends.heard
        .send(Heard::Subscribed {
            by: 9,
            mode: Mode::Stream,
            latest: None,
        })
        .unwrap();
    assert_eq!(
        subscription.recv(),
        Some(Subscribed::Heard(TrackHeard::Up { subscribers: 0 }))
    );
    let object = Data::Object {
        group: 0,
        object: 0,
        body: vec![7],
    };
    handle().block_on(ends.data.send(object.clone())).unwrap();
    assert_eq!(subscription.recv(), Some(Subscribed::Data(object)));
    ends.heard
        .send(Heard::Closed(wire::Error::new(
            refusal::UNKNOWN_TRACK,
            "no such track",
        )))
        .unwrap();
    assert_eq!(
        subscription.recv(),
        Some(Subscribed::Heard(TrackHeard::Ended {
            code: refusal::UNKNOWN_TRACK.into()
        }))
    );
    let (watch, watch_ends) = opened(&opens);
    assert!(matches!(watch, Ask::Watch { namespace } if namespace == "net#1/huddle/lobby"));
    watch_ends
        .heard
        .send(Heard::Announce {
            track: stamped,
            by: 9,
            mode: Mode::Stream,
            up: true,
        })
        .unwrap();
    let (again, _ends) = opened(&opens);
    assert!(matches!(again, Ask::Subscribe { .. }));
    drop(subscription);
}
