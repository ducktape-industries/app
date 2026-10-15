//! `live.*`: a view's tracks through the connected node, off consensus, on
//! the one live session of the connection (`backend::live`):
//!
//! - `live.publish` `LivePublish` — a subscription that is the track: items
//!   `Publishing::Waiting` while the room's program has not granted it, then
//!   `Publishing::Subscribers(n)`; an object the node dropped is an `Err`
//!   item that does not end it; the node's refusal ends it.
//! - `live.send` `LiveSend` — one object onto a track this view publishes.
//!   Answered only when the view publishes no such track
//!   (`live_not_published`) or it does not decode: a 60 Hz sender pays no
//!   reply per frame. An object over its mode's cap (`too_large`) or past
//!   the publication's queue (`in_flight_limit`) is refused on the track's
//!   `live.publish` instead, as the node's own refusals of an object are.
//!   Objects sent before the node took the publication wait in that queue.
//! - `live.subscribe` `LiveSubscribe` — items `LiveItem`: `Waiting` while
//!   the room's program has not granted it, then `Opened`, the group the
//!   node holds (with `latest`), then each object and each group's end. A datagram-mode track keeps only its newest unread object
//!   while the view is behind (drop-oldest, counted `live.dropped_behind`);
//!   a stream-mode one holds its group back.
//! - `live.tracks` `<room>` — items `Watching`: `Waiting` while the room's
//!   program has not granted watching it, then an `Announce` per track up
//!   now and per one that comes or goes.
//!
//! A view's first live request puts a line on its window (the consent
//! line, `Notifications::sharing`): it shares live data with the people in
//! its rooms, through the person's node. Nothing waits on it.
//!
//! The host stamps every room `<chain>/<program>/<room>`: the session's chain
//! id and the view's own program, so a view names only its program's rooms.
//! A lost session (the node restarting, the path breaking) is opened again
//! and each request with it: a publication is published again, a
//! subscription subscribes again from the group the node holds and its view
//! hears a fresh `Opened`, a room's list says every track it had up went
//! down and announces again what is up. A connect inside the network does
//! the same on the new connection (`reconnected`).
//!
//! Tick wake: nothing new. An item a live task writes is an answer in
//! [`Replies`] like any other: `Replies::item` fires the seat's `changed`
//! watch and the seat turns at once (`seat/entity.rs`), one tick per draw,
//! so an object that arrives wakes its view at once.
use std::sync::Arc;

use tokio::sync::mpsc;

use super::node::{Follow, Items, connected, spawn_subscription};
use super::*;
use crate::backend::live::{self as client, Ask, Data, Heard, Mode, Object, Session, Start, Track};
use wire::methods::{
    LiveAnnounce, LiveItem, LiveMode, LivePublish, LiveSend, LiveStart, LiveSubscribe, LiveTrack,
    Publishing, Watching,
};

/// The live tracks a view publishes: where its `live.send`s go, by track.
#[derive(Default)]
pub(in crate::runtime) struct Tracks {
    publications: Vec<Publication>,
    /// The view's window has said it went live.
    told: bool,
}

struct Publication {
    id: u64,
    track: LiveTrack,
    mode: LiveMode,
    outbox: mpsc::Sender<Object>,
}

/// What a live request follows, as a reconnect opens it again.
pub(super) enum LiveFollow {
    Publish(LivePublish),
    Subscribe(LiveTrack),
    Tracks(String),
}

impl Tracks {
    /// The view dropped request `id`: if it was a publication, its sends
    /// have nowhere to go.
    pub(in crate::runtime) fn cancel(&mut self, id: u64) {
        self.publications.retain(|publication| publication.id != id);
    }
}

/// The app quits (`runtime::quitting`): the connection's live session
/// closes so the node hears it at once, not at its idle limit, and a quit
/// opponent's tracks end for the others now.
pub(in crate::runtime) fn quit() -> impl Future<Output = ()> {
    let held = super::super::connection()
        .lock()
        .expect("views rpc")
        .live
        .take();
    let closed = super::handle().spawn(async move {
        if let Some(session) = held {
            session.quit().await;
        }
    });
    async move {
        let _ = closed.await;
    }
}

/// `room` as the node names it, `<chain>/<program>/<room>`; `None` with
/// the request refused `malformed_request`.
fn stamped_room(view: &mut View, id: u64, room: &str) -> Option<String> {
    let namespace = format!("{}/{}/{room}", view.connection.chain, view.program);
    let room_holds = !room.is_empty() && room.chars().all(|c| plain(&c));
    if !room_holds || namespace.len() > live_wire::MAX_NAMESPACE {
        view.refuse(
            id,
            refusal::MALFORMED_REQUEST,
            format!(
                "a room is 1 to {} bytes of plain text",
                live_wire::MAX_NAMESPACE
            ),
        );
        return None;
    }
    Some(namespace)
}

/// `name` on the track's stamped room, held to the node's rule; `None`
/// with the request refused `malformed_request`.
fn stamped(view: &mut View, id: u64, room: &str, name: &str) -> Option<Track> {
    let namespace = stamped_room(view, id, room)?;
    if !live_wire::well_named(name) {
        view.refuse(
            id,
            refusal::MALFORMED_REQUEST,
            format!(
                "a track name is 1 to {} bytes of `/`-separated segments, none empty or `..`",
                live_wire::MAX_TRACK_NAME
            ),
        );
        return None;
    }
    Some(Track {
        namespace,
        name: name.to_owned(),
    })
}

/// A view's first live request: its window says it shares live data.
fn went_live(view: &mut View) {
    if std::mem::replace(&mut view.tracks.told, true) {
        return;
    }
    if crate::runtime::notify::center()
        .lock()
        .went_live(view.program)
    {
        // the window's strip redraws with the line
        view.intents.push(Intent::Notified);
    }
}

/// The session to open a request on, or the request refused: the view's
/// connection is stale or gone (`connected`).
fn ready(view: &mut View, id: u64) -> Option<Arc<dyn Session>> {
    went_live(view);
    connected(view, id)?;
    let session = view.connection.live.clone();
    if session.is_none() {
        view.refuse(id, refusal::NOT_CONNECTED, "not connected to a node");
    }
    session
}

fn mode_of(mode: LiveMode) -> Mode {
    match mode {
        LiveMode::Stream => Mode::Stream,
        LiveMode::Datagram => Mode::Datagram,
    }
}

fn live_mode(mode: Mode) -> LiveMode {
    match mode {
        Mode::Stream => LiveMode::Stream,
        Mode::Datagram => LiveMode::Datagram,
    }
}

/// `live.publish`.
pub(super) fn publish(view: &mut View, id: u64, payload: &[u8]) {
    let ask = match methods::decode::<LivePublish>(payload) {
        Ok(ask) => ask,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    let Some(session) = ready(view, id) else {
        return;
    };
    open_publication(view, id, session, ask);
}

fn open_publication(view: &mut View, id: u64, session: Arc<dyn Session>, ask: LivePublish) {
    let Some(track) = stamped(view, id, &ask.track.room, &ask.track.name) else {
        return;
    };
    let (outbox, objects) = mpsc::channel(client::OUTBOX);
    view.tracks.publications.push(Publication {
        id,
        track: ask.track.clone(),
        mode: ask.mode,
        outbox,
    });
    let open = Ask::Publish {
        track,
        mode: mode_of(ask.mode),
        priority: ask.priority,
    };
    let key = view.perf_key();
    spawn_subscription(
        view,
        id,
        Follow::Live(LiveFollow::Publish(ask)),
        move |items| publishing(session, open, objects, items, key),
    );
}

/// A publication for as long as the view holds it: published again on each
/// new session, its queued objects going out once the node took it.
async fn publishing(
    session: Arc<dyn Session>,
    open: Ask,
    mut objects: mpsc::Receiver<Object>,
    mut items: Items,
    key: crate::perf::Key,
) {
    let mut lost = 0;
    loop {
        let mut request = match opened(&session, open.clone()).await {
            Ok(request) => request,
            Err(refusal) => return items.last(refusal),
        };
        let mut published = false;
        loop {
            tokio::select! {
                biased;
                heard = request.heard.recv() => match heard {
                    Some(Heard::Published) => {
                        published = true;
                        lost = 0;
                        crate::perf::count(key, "live.tracks", 1);
                    }
                    Some(Heard::Subscribers(count)) => {
                        if !items.send(Ok(methods::encode(&Publishing::Subscribers(count)))).await {
                            return;
                        }
                    }
                    Some(Heard::Dropped(code)) => {
                        let refused = wire::Error::new(code.as_str(), "the node dropped an object of this track");
                        if !items.send(Err(refused)).await {
                            return;
                        }
                    }
                    Some(Heard::Waiting) => {
                        lost = 0;
                        if !items.send(Ok(methods::encode(&Publishing::Waiting))).await {
                            return;
                        }
                    }
                    Some(Heard::Closed(refusal)) => return items.last(refusal),
                    Some(Heard::Subscribed { .. } | Heard::Announce { .. }) => {}
                    None => break,
                },
                Some(object) = objects.recv(), if published => {
                    if request.objects.send(object).await.is_err() {
                        break;
                    }
                }
            }
        }
        lost += 1;
        tokio::time::sleep(backend::retry_delay(lost)).await;
    }
}

/// `ask` opened on the session. The node reads accounts on its confirmed
/// layer, so a session for an account whose creating block has not applied
/// there yet is refused `no_account`: that is asked again, a block or so
/// on and longer each time (`retry_delay`, the client's own hold-off
/// alike), for as long as the view holds the request. Any other refusal
/// is the request's last word.
async fn opened(session: &Arc<dyn Session>, ask: Ask) -> Result<client::Request, wire::Error> {
    let mut refused = 0;
    loop {
        match session.open(ask.clone()).await {
            Err(refusal) if refusal.code == client::Code::NoAccount.as_str() => {
                refused += 1;
                tracing::debug!(target: "ducktape::app", reason = "no_account", attempt = refused, "the node has no account for this key yet; the live session waits a block");
                tokio::time::sleep(backend::retry_delay(refused)).await;
            }
            opened => return opened,
        }
    }
}

/// `live.send`: into the publication's queue, refused on the publication
/// when it does not fit.
pub(super) fn send(view: &mut View, id: u64, payload: &[u8]) {
    let send = match methods::decode::<LiveSend>(payload) {
        Ok(send) => send,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    let Some(publication) = view
        .tracks
        .publications
        .iter()
        .rev()
        .find(|publication| publication.track == send.track)
    else {
        return view.refuse(
            id,
            refusal::LIVE_NOT_PUBLISHED,
            "this view publishes no such track",
        );
    };
    let cap = match publication.mode {
        LiveMode::Stream => live_wire::MAX_STREAM_OBJECT,
        LiveMode::Datagram => live_wire::MAX_DATAGRAM_OBJECT,
    };
    let publication_id = publication.id;
    if send.body.len() > cap {
        let refused = wire::Error::new(
            refusal::TOO_LARGE,
            format!("an object of this track is at most {cap} bytes"),
        );
        view.replies.item(publication_id, Err(refused), false);
        return;
    }
    let object = Object {
        group: send.group,
        object: send.object,
        body: send.body,
        end: send.end,
    };
    match publication.outbox.try_send(object) {
        Ok(()) => crate::perf::count(view.perf_key(), "live.objects_out", 1),
        Err(mpsc::error::TrySendError::Full(_)) => {
            let refused = wire::Error::new(
                refusal::IN_FLIGHT_LIMIT,
                format!(
                    "{} objects of this track wait to go out already",
                    client::OUTBOX
                ),
            );
            view.replies.item(publication_id, Err(refused), false);
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            view.tracks.cancel(publication_id);
            view.refuse(
                id,
                refusal::LIVE_NOT_PUBLISHED,
                "this view's publication of the track ended",
            );
        }
    }
}

/// `live.subscribe`.
pub(super) fn subscribe(view: &mut View, id: u64, payload: &[u8]) {
    let ask = match methods::decode::<LiveSubscribe>(payload) {
        Ok(ask) => ask,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    let Some(session) = ready(view, id) else {
        return;
    };
    let from = match ask.from {
        LiveStart::Latest => Start::Latest,
        LiveStart::Next => Start::Next,
    };
    open_subscription(view, id, session, ask.track, from);
}

fn open_subscription(
    view: &mut View,
    id: u64,
    session: Arc<dyn Session>,
    track: LiveTrack,
    from: Start,
) {
    let Some(stamped) = stamped(view, id, &track.room, &track.name) else {
        return;
    };
    let key = view.perf_key();
    spawn_subscription(
        view,
        id,
        Follow::Live(LiveFollow::Subscribe(track)),
        move |items| subscribing(session, stamped, from, items, key),
    );
}

/// A subscription for as long as the view holds it: subscribed again on
/// each new session, from the group the node holds.
async fn subscribing(
    session: Arc<dyn Session>,
    track: Track,
    mut from: Start,
    mut items: Items,
    key: crate::perf::Key,
) {
    let mut lost = 0;
    loop {
        let open = Ask::Subscribe {
            track: track.clone(),
            from,
        };
        let mut request = match opened(&session, open).await {
            Ok(request) => request,
            Err(refusal) => return items.last(refusal),
        };
        loop {
            let item = tokio::select! {
                biased;
                heard = request.heard.recv() => match heard {
                    Some(Heard::Subscribed { by, mode, latest }) => {
                        lost = 0;
                        LiveItem::Opened { by, mode: live_mode(mode), latest }
                    }
                    Some(Heard::Waiting) => {
                        lost = 0;
                        LiveItem::Waiting
                    }
                    Some(Heard::Closed(refusal)) => return items.last(refusal),
                    Some(
                        Heard::Published
                        | Heard::Subscribers(_)
                        | Heard::Announce { .. }
                        | Heard::Dropped(_),
                    ) => continue,
                    None => break,
                },
                Some(data) = request.data.recv() => live_item(data),
                data = request.latest.take() => {
                    let dropped = request.latest.dropped();
                    if dropped > 0 {
                        crate::perf::count(key, "live.dropped_behind", dropped);
                    }
                    live_item(data)
                }
            };
            if matches!(item, LiveItem::Object { .. }) {
                crate::perf::count(key, "live.objects_in", 1);
            }
            if !items.send(Ok(methods::encode(&item))).await {
                return;
            }
        }
        from = Start::Latest;
        lost += 1;
        tokio::time::sleep(backend::retry_delay(lost)).await;
    }
}

fn live_item(data: Data) -> LiveItem {
    match data {
        Data::Object {
            group,
            object,
            body,
        } => LiveItem::Object {
            group,
            object,
            body,
        },
        Data::GroupEnd { group, complete } => LiveItem::GroupEnd { group, complete },
    }
}

/// `live.tracks`.
pub(super) fn tracks(view: &mut View, id: u64, payload: &[u8]) {
    let room = match methods::decode::<String>(payload) {
        Ok(room) => room,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    let Some(session) = ready(view, id) else {
        return;
    };
    open_watch(view, id, session, room);
}

fn open_watch(view: &mut View, id: u64, session: Arc<dyn Session>, room: String) {
    let Some(namespace) = stamped_room(view, id, &room) else {
        return;
    };
    spawn_subscription(
        view,
        id,
        Follow::Live(LiveFollow::Tracks(room)),
        move |items| watching(session, namespace, items),
    );
}

/// A room's track list for as long as the view holds it. On a new session
/// every track it had up goes down first: the node announces again what is
/// up, and what went down meanwhile is not left standing.
async fn watching(session: Arc<dyn Session>, namespace: String, mut items: Items) {
    let mut up: Vec<LiveAnnounce> = Vec::new();
    let mut lost = 0;
    loop {
        let open = Ask::Watch {
            namespace: namespace.clone(),
        };
        let mut heard = match opened(&session, open).await {
            Ok(request) => request,
            Err(refusal) => return items.last(refusal),
        };
        while let Some(heard) = heard.heard.recv().await {
            let announce = match heard {
                Heard::Announce {
                    track,
                    by,
                    mode,
                    up,
                } => LiveAnnounce {
                    name: track.name,
                    by,
                    mode: live_mode(mode),
                    up,
                },
                Heard::Waiting => {
                    lost = 0;
                    if !items.send(Ok(methods::encode(&Watching::Waiting))).await {
                        return;
                    }
                    continue;
                }
                Heard::Closed(refusal) => return items.last(refusal),
                Heard::Published
                | Heard::Subscribers(_)
                | Heard::Subscribed { .. }
                | Heard::Dropped(_) => continue,
            };
            lost = 0;
            up.retain(|held| held.name != announce.name);
            if announce.up {
                up.push(announce.clone());
            }
            if !items
                .send(Ok(methods::encode(&Watching::Announce(announce))))
                .await
            {
                return;
            }
        }
        for gone in std::mem::take(&mut up) {
            let down = LiveAnnounce { up: false, ..gone };
            if !items
                .send(Ok(methods::encode(&Watching::Announce(down))))
                .await
            {
                return;
            }
        }
        lost += 1;
        tokio::time::sleep(backend::retry_delay(lost)).await;
    }
}

/// A connect inside the network: the request opened again on the new
/// connection's session, as it was asked (a subscription from the group the
/// node holds).
pub(super) fn reopen(view: &mut View, id: u64, follows: LiveFollow) {
    let Some(session) = view.connection.live.clone() else {
        return view.refuse(id, refusal::NOT_CONNECTED, "not connected to a node");
    };
    match follows {
        LiveFollow::Publish(ask) => {
            view.tracks.cancel(id);
            open_publication(view, id, session, ask);
        }
        LiveFollow::Subscribe(track) => open_subscription(view, id, session, track, Start::Latest),
        LiveFollow::Tracks(room) => open_watch(view, id, session, room),
    }
}

#[cfg(test)]
mod tests;
