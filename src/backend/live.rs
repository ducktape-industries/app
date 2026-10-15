//! The live client: one session with the connected node's relay, through
//! which every view's tracks and subscriptions ride, off consensus. QUIC on
//! the node's UDP socket (the port `/v1/live/port` names, ALPN
//! `ducktape-live`, the node's key from `/v1/status` pinned as its
//! certificate's), or the websocket `/v1/live` on its HTTP port where it
//! names none or no UDP answers in [`QUIC_ANSWER`]. A node that answers
//! the port's route 404 serves no live tracks: the request is refused
//! `live_unavailable`, and nothing is dialed. A lost session ends what rode on it: each request's
//! channels close, and the next request dials a new session.
//!
//! The wire is core's `live-wire`, borsh throughout: a length-prefixed
//! `Control` on the first bidirectional stream (the handshake, then requests
//! and their answers), one group per unidirectional stream (`GroupHeader`,
//! then each object as a `u32` little-endian length and its bytes, until FIN
//! or RESET) and one object per datagram (`DatagramHeader`, then the
//! payload). Over the websocket each of them is one binary `WsFrame`.
//!
//! Nothing here knows a view: a track's namespace comes stamped.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};
use live_wire::{
    Alias, Control, DatagramHeader, End, GroupHeader, Hello, KeyScheme, Proof, RequestId, WsFrame,
};
pub(crate) use live_wire::{Code, Mode, Priority, Start, Track};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use view_wire::Error;
use view_wire::methods::refusal;

use super::RpcClient;

/// How long a dial waits for the node's QUIC answer before it takes the
/// websocket.
const QUIC_ANSWER: Duration = Duration::from_secs(2);
/// How long a node whose UDP stayed silent is dialed over the websocket
/// alone, so a node behind a TCP-only tunnel costs no [`QUIC_ANSWER`] per
/// connect.
const QUIC_RETRY: Duration = Duration::from_secs(60);
/// How long the handshake may take: the node's own deadline.
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// Stream-mode objects one subscription holds for its view. Past it the
/// group's QUIC stream waits, which holds the node back by flow control;
/// over the websocket, which carries every track on one socket, the object
/// is dropped and counted instead.
const DATA_BACKLOG: usize = 64;
/// Objects one publication holds: before the node took it, and while its
/// stream is behind. Past it a send is refused `in_flight_limit`.
pub(crate) const OUTBOX: usize = 64;
/// The keep-alive and idle limit of a session, on QUIC and the websocket
/// alike: the node's QUIC transport's.
const KEEP_ALIVE: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(30);
/// How long a quitting app waits for its session's close to leave, so the
/// node hears the quit at once and not at [`IDLE`]: well inside gpui's
/// 200 ms for every quit hook.
const QUIT_FLUSH: Duration = Duration::from_millis(100);
/// The server name of a QUIC dial: the wire's one name, which the pinned
/// verifier does not read.
const SERVER_NAME: &str = match std::str::from_utf8(live_wire::ALPN) {
    Ok(name) => name,
    Err(_) => panic!("the live ALPN is text"),
};
/// An ed25519 key's DER `SubjectPublicKeyInfo`, up to the key itself.
const ED25519_SPKI: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn unavailable(message: impl std::fmt::Display) -> Error {
    Error::new(refusal::LIVE_UNAVAILABLE, message.to_string())
}

/// What a request asks of the node.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ask {
    Publish {
        track: Track,
        mode: Mode,
        priority: Priority,
    },
    Subscribe {
        track: Track,
        from: Start,
    },
    Watch {
        namespace: String,
    },
}

/// What a request hears on the node's control stream.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Heard {
    /// The node took the publication: its objects go out.
    Published,
    Subscribers(u32),
    Subscribed {
        by: u64,
        mode: Mode,
        latest: Option<u64>,
    },
    Announce {
        track: Track,
        by: u64,
        mode: Mode,
        up: bool,
    },
    /// The node dropped one object of this publication; the track goes on.
    Dropped(Code),
    /// The node holds the request until the room grants it (core's
    /// `Waiting` says `NotGranted`, the one reason it waits); it answers on
    /// the block that does, and the request goes on.
    Waiting,
    /// The node ended the request, or the session ended it as a wire
    /// mismatch: the refusal a view reads.
    Closed(Error),
}

/// What a subscription hears on its data path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Data {
    Object {
        group: u64,
        object: u32,
        body: Vec<u8>,
    },
    GroupEnd {
        group: u64,
        complete: bool,
    },
}

/// One object a publication sends: `end` closes its group after it, and an
/// empty body with `end` closes the group adding none.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Object {
    pub(crate) group: u64,
    pub(crate) object: u32,
    pub(crate) body: Vec<u8>,
    pub(crate) end: bool,
}

/// The newest datagram-mode object a subscription has not read. A newer
/// one replaces it, which is the datagram's own rule (the newest wins), and
/// is counted: the drop-oldest park of a view that is behind.
#[derive(Default)]
pub(crate) struct Latest {
    slot: Mutex<Option<Data>>,
    ready: Notify,
    dropped: AtomicU64,
}

impl Latest {
    pub(crate) fn put(&self, data: Data) {
        if lock(&self.slot).replace(data).is_some() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.ready.notify_one();
    }

    pub(crate) async fn take(&self) -> Data {
        loop {
            if let Some(data) = lock(&self.slot).take() {
                return data;
            }
            self.ready.notified().await;
        }
    }

    /// The objects replaced unread since the last call.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }
}

/// One open request: what it hears, and for a publication where its
/// objects go. Dropping it withdraws the request from the node.
pub(crate) struct Request {
    pub(crate) heard: mpsc::UnboundedReceiver<Heard>,
    pub(crate) data: mpsc::Receiver<Data>,
    pub(crate) latest: Arc<Latest>,
    pub(crate) objects: mpsc::Sender<Object>,
    withdraw: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(withdraw) = self.withdraw.take() {
            withdraw();
        }
    }
}

/// The session's ends of a [`Request`].
pub(crate) struct Ends {
    pub(crate) heard: mpsc::UnboundedSender<Heard>,
    pub(crate) data: mpsc::Sender<Data>,
    pub(crate) latest: Arc<Latest>,
    pub(crate) objects: mpsc::Receiver<Object>,
}

/// A request and its session's ends; `withdraw` runs when the request is
/// dropped.
pub(crate) fn request(withdraw: impl FnOnce() + Send + 'static) -> (Request, Ends) {
    request_holding(withdraw, 0)
}

/// [`request`], its data path holding `replayed` more objects than
/// [`DATA_BACKLOG`]: the group in flight a later reader is handed.
fn request_holding(withdraw: impl FnOnce() + Send + 'static, replayed: usize) -> (Request, Ends) {
    let (heard, hears) = mpsc::unbounded_channel();
    let (data, datas) = mpsc::channel(DATA_BACKLOG + replayed);
    let (objects, outbox) = mpsc::channel(OUTBOX);
    let latest = Arc::new(Latest::default());
    let request = Request {
        heard: hears,
        data: datas,
        latest: latest.clone(),
        objects,
        withdraw: Some(Box::new(withdraw)),
    };
    let ends = Ends {
        heard,
        data,
        latest,
        objects: outbox,
    };
    (request, ends)
}

/// What the kernel asks of a live session: the node's ([`Client`]), or a
/// test's.
pub(crate) trait Session: Send + Sync {
    /// Opens `ask` on the node, dialing a session first if none is open.
    fn open(&self, ask: Ask) -> BoxFuture<'static, Result<Request, Error>>;

    /// The app quits: the open session closes in a way the node hears at
    /// once. Runs on the app's tokio runtime.
    fn quit(&self) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

/// The live client of one connection: at most one session at a time,
/// dialed when a request needs one.
#[derive(Clone)]
pub(crate) struct Client(Arc<Inner>);

struct Inner {
    rpc: RpcClient,
    conn: tokio::sync::Mutex<Option<Arc<Conn>>>,
    /// Until when the node asked not to be dialed (`GoAway`), or the
    /// client holds off after a failure ([`Inner::hold_off`]).
    not_before: Mutex<Option<Instant>>,
    /// Failures since the last session opened: dials that failed (the
    /// node refused, was busy or was not there) and sessions ended as a
    /// wire mismatch. Each holds the next dial off longer.
    failures: AtomicU32,
}

impl Inner {
    /// The next dial waits [`super::retry_delay`] of the failures so far:
    /// a refusal, a busy node or a wire mismatch is never redialed at once.
    fn hold_off(&self) {
        let failures = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.not_before) = Some(Instant::now() + super::retry_delay(failures));
    }
}

impl Client {
    pub(crate) fn new(rpc: RpcClient) -> Client {
        Client(Arc::new(Inner {
            rpc,
            conn: tokio::sync::Mutex::new(None),
            not_before: Mutex::new(None),
            failures: AtomicU32::new(0),
        }))
    }

    async fn conn(&self) -> Result<Arc<Conn>, Error> {
        let mut held = self.0.conn.lock().await;
        if let Some(conn) = held.as_ref().filter(|conn| !conn.closed()) {
            return Ok(conn.clone());
        }
        let not_before = lock(&self.0.not_before).take();
        if let Some(until) = not_before {
            tokio::time::sleep_until(until.into()).await;
        }
        let conn = match dial(&self.0.rpc, Arc::downgrade(&self.0)).await {
            Ok(conn) => conn,
            Err(refused) => {
                self.0.hold_off();
                return Err(refused);
            }
        };
        self.0.failures.store(0, Ordering::SeqCst);
        *held = Some(conn.clone());
        Ok(conn)
    }
}

impl Session for Client {
    fn open(&self, ask: Ask) -> BoxFuture<'static, Result<Request, Error>> {
        let client = self.clone();
        Box::pin(async move { client.conn().await?.open(ask) })
    }

    /// Over QUIC the close is a CONNECTION_CLOSE the endpoint sends after
    /// this returns: the process waits up to [`QUIT_FLUSH`] for it to
    /// leave. A websocket's TCP closes with the process.
    fn quit(&self) -> BoxFuture<'static, ()> {
        // a dial in flight holds the lock: no session is open to close
        let conn = self.0.conn.try_lock().ok().and_then(|mut held| held.take());
        Box::pin(async move {
            let Some(conn) = conn else {
                return;
            };
            conn.close(Close::Quit);
            if let Some(udp) = &conn.udp {
                let _ = tokio::time::timeout(QUIT_FLUSH, udp.wait_idle()).await;
            }
        })
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.get_mut().take() {
            conn.close(Close::ConnectionChanged);
        }
    }
}

/// When each node's UDP last stayed silent, by its `/v1` address.
fn silent() -> MutexGuard<'static, HashMap<String, Instant>> {
    static SILENT: std::sync::OnceLock<Mutex<HashMap<String, Instant>>> =
        std::sync::OnceLock::new();
    lock(SILENT.get_or_init(Mutex::default))
}

/// The path a session's bytes take.
enum Path {
    Quic(quinn::Connection),
    /// The websocket's writer queue.
    Ws(mpsc::UnboundedSender<WsFrame>),
}

/// One live session with the node.
struct Conn {
    path: Path,
    /// The QUIC endpoint a session over QUIC sends from: a close waits on
    /// it to leave.
    udp: Option<quinn::Endpoint>,
    /// Control messages to the node, in order (the control writer's queue;
    /// over the websocket, the socket's).
    control: mpsc::UnboundedSender<Control>,
    table: Mutex<Table>,
    next: AtomicU64,
    closed: AtomicBool,
    client: Weak<Inner>,
}

#[derive(Default)]
struct Table {
    requests: HashMap<RequestId, Entry>,
    /// The subscription each alias in a data header names.
    aliases: HashMap<Alias, RequestId>,
}

/// One node request. The node holds one subscriber per track and one
/// watcher per room for a session, so every request of the app that follows
/// the same track or room shares one: its readers. A publication has one
/// reader; a second publisher of a track in the app is refused.
struct Entry {
    readers: Vec<Reader>,
    kind: Kind,
}

/// What a node request is, and what the session keeps of it.
enum Kind {
    Publish {
        track: Track,
        /// The alias, handed to the publication's writer once the node
        /// took it; `None` once it did.
        taken: Option<oneshot::Sender<Alias>>,
    },
    Subscribe {
        track: Track,
        alias: Option<Alias>,
        /// Over the websocket, the group the subscription is in and its
        /// next object's number: the stream's implicit count.
        counted: Option<(u64, u32)>,
        /// The publisher and mode, once the node answered: a later
        /// reader's `Subscribed`.
        opened: Option<(u64, Mode)>,
        /// The group in flight, for a later reader that asks `Latest`.
        in_flight: Option<InFlight>,
    },
    Watch {
        namespace: String,
        /// The room's tracks up now, for a later reader of its list.
        up: Vec<Heard>,
    },
}

/// One request of the app that a node request serves.
struct Reader {
    id: u64,
    heard: mpsc::UnboundedSender<Heard>,
    data: mpsc::Sender<Data>,
    latest: Arc<Latest>,
    /// Groups up to this one are not its: it asked `Next` while the group
    /// was in flight.
    after: Option<u64>,
    /// Where it asked to start: with `Next` the group the node replays
    /// first (`Subscribed.latest`) is not its.
    from: Start,
}

impl Reader {
    fn new(id: u64, ends: &Ends, from: Start) -> Reader {
        Reader {
            id,
            heard: ends.heard.clone(),
            data: ends.data.clone(),
            latest: ends.latest.clone(),
            after: None,
            from,
        }
    }

    fn follows(&self, data: &Data) -> bool {
        let group = match data {
            Data::Object { group, .. } | Data::GroupEnd { group, .. } => *group,
        };
        self.after.is_none_or(|after| group > after)
    }
}

/// A shared subscription's group in flight, from its first object: what a
/// later reader that asks `Latest` is handed. Kept while it fits a group's
/// cap ([`live_wire::MAX_GROUP_BYTES`]); dropped when the group ends or a
/// newer one starts. A datagram track keeps its newest object only.
struct InFlight {
    group: u64,
    objects: Vec<Data>,
    bytes: usize,
    /// Every object since the group's first: a later reader can be handed
    /// it.
    whole: bool,
}

impl Kind {
    /// Whether `ask` asks what this request asks of the node: the same
    /// track published or followed, or the same room's list.
    fn serves(&self, ask: &Ask) -> bool {
        match (self, ask) {
            (Kind::Publish { track: held, .. }, Ask::Publish { track, .. })
            | (Kind::Subscribe { track: held, .. }, Ask::Subscribe { track, .. }) => held == track,
            (
                Kind::Watch {
                    namespace: held, ..
                },
                Ask::Watch { namespace },
            ) => held == namespace,
            _ => false,
        }
    }

    fn alias(&self) -> Option<Alias> {
        match self {
            Kind::Subscribe { alias, .. } => *alias,
            Kind::Publish { .. } | Kind::Watch { .. } => None,
        }
    }
}

impl Entry {
    /// `heard` to every reader.
    fn tell(&self, heard: Heard) {
        for reader in &self.readers {
            let _ = reader.heard.send(heard.clone());
        }
    }

    /// One item of the subscription's data: kept in the group in flight,
    /// and the readers it is for.
    fn keep(&mut self, data: &Data) -> impl Iterator<Item = &Reader> {
        if let Kind::Subscribe {
            opened, in_flight, ..
        } = &mut self.kind
        {
            match data {
                Data::Object {
                    group,
                    object,
                    body,
                } => {
                    let datagram = matches!(opened, Some((_, Mode::Datagram)));
                    let fresh = in_flight
                        .as_ref()
                        .is_none_or(|in_flight| in_flight.group != *group);
                    if fresh || datagram {
                        *in_flight = Some(InFlight {
                            group: *group,
                            objects: Vec::new(),
                            bytes: 0,
                            whole: datagram || *object == 0,
                        });
                    }
                    let in_flight = in_flight.as_mut().expect("kept just now");
                    let fits = in_flight.bytes + body.len() <= live_wire::MAX_GROUP_BYTES;
                    match in_flight.whole && fits {
                        true => {
                            in_flight.bytes += body.len();
                            in_flight.objects.push(data.clone());
                        }
                        false => {
                            in_flight.whole = false;
                            in_flight.objects = Vec::new();
                        }
                    }
                }
                Data::GroupEnd { .. } => *in_flight = None,
            }
        }
        self.readers.iter().filter(|reader| reader.follows(data))
    }

    /// A later request that follows this subscription: it hears what the
    /// node answered, and with `Latest` the group in flight first.
    fn join_subscription(&mut self, reader: Reader, ends: &Ends) {
        let mut reader = reader;
        let Kind::Subscribe {
            opened, in_flight, ..
        } = &self.kind
        else {
            return;
        };
        if let Some((by, mode)) = *opened {
            let replay = match reader.from {
                Start::Latest => in_flight.as_ref().filter(|in_flight| in_flight.whole),
                Start::Next => {
                    reader.after = in_flight.as_ref().map(|in_flight| in_flight.group);
                    None
                }
            };
            let latest = replay.map(|in_flight| in_flight.group);
            let _ = reader.heard.send(Heard::Subscribed { by, mode, latest });
            for data in replay.iter().flat_map(|in_flight| &in_flight.objects) {
                match mode {
                    Mode::Datagram => ends.latest.put(data.clone()),
                    Mode::Stream => {
                        // the data path holds the replay (`request_holding`)
                        let _ = ends.data.try_send(data.clone());
                    }
                }
            }
        }
        self.readers.push(reader);
    }

    /// A later request that follows this room's list: the tracks up now
    /// first.
    fn join_list(&mut self, reader: Reader) {
        if let Kind::Watch { up, .. } = &self.kind {
            for announce in up {
                let _ = reader.heard.send(announce.clone());
            }
        }
        self.readers.push(reader);
    }
}

/// Why a session closed: the log's `reason`, and the QUIC close's.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Close {
    Quit,
    ConnectionChanged,
    GoAway,
    ControlClosed,
    ConnectionClosed,
    SocketClosed,
    SocketQuiet,
    WireMismatch,
    ClientOnlyMessage,
}

impl Close {
    fn as_str(self) -> &'static str {
        match self {
            Close::Quit => "quit",
            Close::ConnectionChanged => "connection_changed",
            Close::GoAway => "go_away",
            Close::ControlClosed => "control_closed",
            Close::ConnectionClosed => "connection_closed",
            Close::SocketClosed => "socket_closed",
            Close::SocketQuiet => "socket_quiet",
            Close::WireMismatch => "wire_mismatch",
            Close::ClientOnlyMessage => "client_only_message",
        }
    }
}

impl Conn {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Ends the session: every request's channels close, which its reader
    /// hears as the session gone.
    fn close(&self, why: Close) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!(target: "ducktape::app", event = "live_session", reason = why.as_str(), "a live session closed");
        *lock(&self.table) = Table::default();
        if let Path::Quic(connection) = &self.path {
            connection.close(0u32.into(), why.as_str().as_bytes());
        }
    }

    /// Ends the session as a wire mismatch: every request hears
    /// `refusal` and ends, rather than opening again on a session that
    /// would fail the same way.
    fn fail(&self, refusal: Error) {
        for (_, entry) in lock(&self.table).requests.drain() {
            entry.tell(Heard::Closed(refusal.clone()));
        }
        if let Some(client) = self.client.upgrade() {
            client.hold_off();
        }
        self.close(Close::WireMismatch);
    }

    /// `ask` as a request of the session: a new node request, or a share
    /// of the one that follows the same track or room already. A second
    /// publisher of a track in the app is refused `duplicate_track`: one
    /// publisher per track.
    fn open(self: &Arc<Self>, ask: Ask) -> Result<Request, Error> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let weak = Arc::downgrade(self);
        let withdraw = move || {
            if let Some(conn) = weak.upgrade() {
                conn.withdraw(id);
            }
        };
        let mut table = lock(&self.table);
        let held = table
            .requests
            .values_mut()
            .find(|held| held.kind.serves(&ask));
        if let Some(held) = held {
            return match ask {
                Ask::Publish { .. } => Err(Error::new(
                    refusal::DUPLICATE_TRACK,
                    "another window of this app publishes this track",
                )),
                Ask::Subscribe { from, .. } => {
                    let replayed = match &held.kind {
                        Kind::Subscribe {
                            in_flight: Some(in_flight),
                            ..
                        } => in_flight.objects.len(),
                        _ => 0,
                    };
                    let (request, ends) = request_holding(withdraw, replayed);
                    held.join_subscription(Reader::new(id, &ends, from), &ends);
                    Ok(request)
                }
                Ask::Watch { .. } => {
                    let (request, ends) = request(withdraw);
                    held.join_list(Reader::new(id, &ends, Start::Latest));
                    Ok(request)
                }
            };
        }
        let (request, ends) = request(withdraw);
        let from = match ask {
            Ask::Subscribe { from, .. } => from,
            Ask::Publish { .. } | Ask::Watch { .. } => Start::Latest,
        };
        let reader = Reader::new(id, &ends, from);
        let (kind, control) = match ask {
            Ask::Publish {
                track,
                mode,
                priority,
            } => {
                let (taken, alias) = oneshot::channel();
                tokio::spawn(write(self.clone(), alias, ends.objects, mode));
                (
                    Kind::Publish {
                        track: track.clone(),
                        taken: Some(taken),
                    },
                    Control::Publish {
                        request: id,
                        track,
                        mode,
                        priority,
                    },
                )
            }
            Ask::Subscribe { track, from } => (
                Kind::Subscribe {
                    track: track.clone(),
                    alias: None,
                    counted: None,
                    opened: None,
                    in_flight: None,
                },
                Control::Subscribe {
                    request: id,
                    track,
                    from,
                },
            ),
            Ask::Watch { namespace } => (
                Kind::Watch {
                    namespace: namespace.clone(),
                    up: Vec::new(),
                },
                Control::Watch {
                    request: id,
                    namespace,
                },
            ),
        };
        table.requests.insert(
            id,
            Entry {
                readers: vec![reader],
                kind,
            },
        );
        drop(table);
        let _ = self.control.send(control);
        Ok(request)
    }

    /// Request `id` of the app was dropped: it leaves the node request it
    /// shares, and the node is told once its last reader went, unless the
    /// node ended it first.
    fn withdraw(&self, id: u64) {
        let gone = {
            let mut table = lock(&self.table);
            let Some((&request, entry)) = table
                .requests
                .iter_mut()
                .find(|(_, entry)| entry.readers.iter().any(|reader| reader.id == id))
            else {
                return;
            };
            entry.readers.retain(|reader| reader.id != id);
            if !entry.readers.is_empty() {
                return;
            }
            let entry = table.requests.remove(&request).expect("found just now");
            if let Some(alias) = entry.kind.alias() {
                table.aliases.remove(&alias);
            }
            (request, entry.kind)
        };
        let control = match gone {
            (request, Kind::Publish { .. }) => Control::Unpublish { request },
            (request, Kind::Subscribe { .. }) => Control::Unsubscribe { request },
            (request, Kind::Watch { .. }) => Control::Unwatch { request },
        };
        let _ = self.control.send(control);
    }

    /// One control message from the node.
    fn heard(&self, control: Control) -> Result<(), Close> {
        let mut table = lock(&self.table);
        match control {
            Control::Published { request, alias } => {
                if let Some(Entry {
                    kind: Kind::Publish { taken, .. },
                    ..
                }) = table.requests.get_mut(&request)
                {
                    if let Some(taken) = taken.take() {
                        let _ = taken.send(alias);
                    }
                }
                if let Some(entry) = table.requests.get(&request) {
                    entry.tell(Heard::Published);
                }
            }
            Control::Subscribed {
                request,
                alias,
                by,
                mode,
                latest,
            } => {
                if let Some(Entry {
                    readers,
                    kind:
                        Kind::Subscribe {
                            alias: held,
                            opened,
                            ..
                        },
                }) = table.requests.get_mut(&request)
                {
                    *held = Some(alias);
                    *opened = Some((by, mode));
                    for reader in readers {
                        // the group the node replays is not a `Next` reader's
                        let latest = match reader.from {
                            Start::Next => {
                                reader.after = latest;
                                None
                            }
                            Start::Latest => latest,
                        };
                        let _ = reader.heard.send(Heard::Subscribed { by, mode, latest });
                    }
                    table.aliases.insert(alias, request);
                }
            }
            Control::Subscribers { request, count } => {
                if let Some(entry) = table.requests.get(&request) {
                    entry.tell(Heard::Subscribers(count));
                }
            }
            Control::Announce {
                request,
                track,
                by,
                mode,
                up,
            } => {
                if let Some(entry) = table.requests.get_mut(&request) {
                    let announce = Heard::Announce {
                        track: track.clone(),
                        by,
                        mode,
                        up,
                    };
                    if let Kind::Watch { up: held, .. } = &mut entry.kind {
                        held.retain(
                            |held| !matches!(held, Heard::Announce { track: held, .. } if *held == track),
                        );
                        if up {
                            held.push(announce.clone());
                        }
                    }
                    entry.tell(announce);
                }
            }
            Control::Ended { request, reason } => {
                let word = match reason {
                    End::TrackEnded => refusal::TRACK_ENDED,
                    End::PublisherGone => refusal::PUBLISHER_GONE,
                    End::GrantRevoked => refusal::NOT_GRANTED,
                };
                end(&mut table, request, word);
            }
            Control::Waiting { request, .. } => {
                if let Some(entry) = table.requests.get(&request) {
                    entry.tell(Heard::Waiting);
                }
            }
            Control::Refused { request, code } => {
                // a publication the node took: one object of it refused
                let dropped = matches!(
                    table.requests.get(&request),
                    Some(Entry {
                        kind: Kind::Publish { taken: None, .. },
                        ..
                    })
                );
                match dropped {
                    true => table.requests[&request].tell(Heard::Dropped(code)),
                    false => end(&mut table, request, code.as_str()),
                }
            }
            Control::GoAway { in_ms } => {
                drop(table);
                if let Some(client) = self.client.upgrade() {
                    *lock(&client.not_before) =
                        Some(Instant::now() + Duration::from_millis(u64::from(in_ms)));
                }
                return Err(Close::GoAway);
            }
            Control::Hello(_)
            | Control::Challenge(_)
            | Control::Proof(_)
            | Control::Welcome(_)
            | Control::Publish { .. }
            | Control::Unpublish { .. }
            | Control::Subscribe { .. }
            | Control::Unsubscribe { .. }
            | Control::Watch { .. }
            | Control::Unwatch { .. } => return Err(Close::ClientOnlyMessage),
        }
        Ok(())
    }

    /// One stream-mode item of the subscription `alias` names, kept in
    /// its group in flight: the data paths of the readers it is for, to
    /// send to outside the lock. `None` when nothing follows `alias`.
    fn deliver(&self, alias: Alias, data: &Data) -> Option<Vec<mpsc::Sender<Data>>> {
        let mut table = lock(&self.table);
        let request = *table.aliases.get(&alias)?;
        let entry = table.requests.get_mut(&request)?;
        Some(entry.keep(data).map(|reader| reader.data.clone()).collect())
    }

    /// One datagram-mode object of the subscription `alias` names: the
    /// newest for each reader it is for.
    fn deliver_latest(&self, alias: Alias, data: Data) {
        let mut table = lock(&self.table);
        let Some(request) = table.aliases.get(&alias).copied() else {
            return;
        };
        if let Some(entry) = table.requests.get_mut(&request) {
            for reader in entry.keep(&data) {
                reader.latest.put(data.clone());
            }
        }
    }
}

/// Request `id` ends with `word`: its entry goes, and its reader hears it.
fn end(table: &mut Table, id: RequestId, word: &'static str) {
    if let Some(entry) = table.requests.remove(&id) {
        if let Some(alias) = entry.kind.alias() {
            table.aliases.remove(&alias);
        }
        let what = match entry.kind {
            Kind::Publish { .. } => "the node ended this track",
            Kind::Subscribe { .. } => "the node ended this subscription",
            Kind::Watch { .. } => "the node ended this list",
        };
        entry.tell(Heard::Closed(Error::new(word, what)));
    }
}

/// Why a control message was not read.
#[derive(Debug)]
enum Unread {
    /// The stream ended or broke: the session is gone, and a new one may
    /// serve.
    Closed(String),
    /// Bytes came that are not the wire this client speaks: a new session
    /// would fail the same way.
    Mismatch(String),
}

impl Unread {
    /// The refusal of a request that was opening when it happened.
    fn refusal(self) -> Error {
        match self {
            Unread::Closed(why) => unavailable(why),
            Unread::Mismatch(what) => mismatch(what),
        }
    }
}

/// The node speaks another live wire than this client (`what` names the
/// message and the decode error): said in the log, and the refusal that
/// ends the requests.
fn mismatch(what: String) -> Error {
    tracing::warn!(target: "ducktape::app", reason = "wire_mismatch", %what, "the node's live message does not decode: it speaks another live wire");
    Error::new(
        refusal::NODE_FAILED,
        format!("the node speaks another live wire: {what}"),
    )
}

/// A control message or a stream-mode object on the wire: its `u32`
/// little-endian length, then the bytes.
fn framed(bytes: Vec<u8>) -> Vec<u8> {
    let mut framed = (bytes.len() as u32).to_le_bytes().to_vec();
    framed.extend(bytes);
    framed
}

/// `bytes` did not decode as a `kind`: which message it was (its borsh
/// variant tag, in live-wire's order), its size, and the error.
fn undecoded(kind: &str, bytes: &[u8], error: impl std::fmt::Display) -> String {
    match bytes.first() {
        Some(tag) => format!("{kind} #{tag} of {} bytes: {error}", bytes.len()),
        None => format!("an empty {kind}: {error}"),
    }
}

/// A session with the node `rpc` reaches: its status for the key and the
/// chain, its live port, QUIC where it names one and it answers, the
/// websocket otherwise, then the handshake.
async fn dial(rpc: &RpcClient, client: Weak<Inner>) -> Result<Arc<Conn>, Error> {
    let status = rpc.status().await.map_err(super::refused)?;
    let port = match rpc.live_port().await {
        Ok(port) => port,
        Err(noded_client::Error::Failed { status: 404, .. }) => {
            return Err(unavailable("this node serves no live tracks"));
        }
        Err(error) => return Err(super::refused(error)),
    };
    let key = super::seated_key().await?;
    let endpoint = rpc.endpoint().to_owned();
    let udp_silent = silent()
        .get(&endpoint)
        .is_some_and(|at| at.elapsed() < QUIC_RETRY);
    let quic = match port {
        Some(port) if !udp_silent => match quic(&endpoint, port, &status.identity).await {
            Ok(dialed) => Some(dialed),
            Err(error) => {
                tracing::info!(target: "ducktape::app", reason = "udp_silent", %error, "the node's live UDP did not answer; the websocket instead");
                silent().insert(endpoint.clone(), Instant::now());
                None
            }
        },
        _ => None,
    };
    let hello = Hello {
        chain: status.genesis,
        scheme: KeyScheme::Ed25519,
        key: key.clone(),
    };
    let udp = quic.as_ref().map(|(udp, _)| udp.clone());
    let (path, control, mut link) = match quic {
        Some((_, connection)) => {
            let (send, recv) = connection
                .open_bi()
                .await
                .map_err(|error| unavailable(format!("no control stream: {error}")))?;
            let (control, queue) = mpsc::unbounded_channel();
            (
                Path::Quic(connection.clone()),
                control,
                Link::Quic {
                    connection,
                    send,
                    recv,
                    queue,
                },
            )
        }
        None => {
            let url = format!("{}{}", endpoint.replacen("http", "ws", 1), live_wire::ROUTE);
            let (socket, _) =
                tokio::time::timeout(HELLO_WAIT, tokio_tungstenite::connect_async(url))
                    .await
                    .map_err(|_| unavailable("the node's live socket did not open in time"))?
                    .map_err(|error| {
                        unavailable(format!("the node serves no live socket: {error}"))
                    })?;
            let (frames, queue) = mpsc::unbounded_channel();
            let (control, controls) = mpsc::unbounded_channel();
            (
                Path::Ws(frames),
                control,
                Link::Ws {
                    socket: Box::new(socket),
                    queue,
                    controls,
                },
            )
        }
    };
    let node = &status.identity;
    let datagrams = tokio::time::timeout(HELLO_WAIT, handshake(&mut link, hello, node))
        .await
        .map_err(|_| unavailable("the node did not finish the live handshake in time"))??;
    let conn = Arc::new(Conn {
        path,
        udp,
        control,
        table: Mutex::new(Table::default()),
        next: AtomicU64::new(1),
        closed: AtomicBool::new(false),
        client,
    });
    tracing::info!(
        target: "ducktape::app",
        event = "live_session",
        path = if matches!(conn.path, Path::Quic(_)) { "quic" } else { "ws" },
        datagrams,
        "a live session opened"
    );
    link.run(conn.clone());
    Ok(conn)
}

/// The control side of a session as the handshake and the readers see it.
enum Link {
    Quic {
        connection: quinn::Connection,
        send: quinn::SendStream,
        recv: quinn::RecvStream,
        queue: mpsc::UnboundedReceiver<Control>,
    },
    Ws {
        socket: Box<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        /// Frames to the node (data and control alike), in order.
        queue: mpsc::UnboundedReceiver<WsFrame>,
        /// Control messages, wrapped into frames on the way out.
        controls: mpsc::UnboundedReceiver<Control>,
    },
}

/// `Hello` → `Challenge` → `Proof` → `Welcome`: whether the path carries
/// datagrams, or the node's refusal. The proof signs the node's key too,
/// the one the client pinned (the QUIC certificate's, which is the key
/// `/v1/status` names, or that key over the websocket): a proof made for
/// one node is none to another.
async fn handshake(link: &mut Link, hello: Hello, node: &[u8]) -> Result<bool, Error> {
    let (chain, key) = (hello.chain, hello.key.clone());
    link.send(&Control::Hello(hello)).await?;
    let heard = link.recv().await.map_err(Unread::refusal)?;
    let Control::Challenge(challenge) = heard else {
        return Err(unavailable("the node did not challenge the hello"));
    };
    let preimage = live_wire::proof_preimage(&chain, node, &challenge.nonce, &key);
    let (signer, signature) = super::seated_sign(live_wire::NAMESPACE, &preimage).await?;
    if signer != key {
        return Err(Error::new(
            refusal::STALE_CONNECTION,
            "the seated key changed during the live handshake",
        ));
    }
    link.send(&Control::Proof(Proof { signature })).await?;
    match link.recv().await.map_err(Unread::refusal)? {
        Control::Welcome(welcome) => Ok(welcome.datagrams),
        Control::Refused { code, .. } => Err(Error::new(
            code.as_str(),
            "the node refused this key a live session",
        )),
        Control::Hello(_)
        | Control::Challenge(_)
        | Control::Proof(_)
        | Control::Publish { .. }
        | Control::Unpublish { .. }
        | Control::Subscribe { .. }
        | Control::Unsubscribe { .. }
        | Control::Watch { .. }
        | Control::Unwatch { .. }
        | Control::Published { .. }
        | Control::Subscribed { .. }
        | Control::Subscribers { .. }
        | Control::Announce { .. }
        | Control::Ended { .. }
        | Control::Waiting { .. }
        | Control::GoAway { .. } => Err(unavailable("the node did not welcome the proof")),
    }
}

impl Link {
    async fn send(&mut self, control: &Control) -> Result<(), Error> {
        match self {
            Link::Quic { send, .. } => send
                .write_all(&framed(abi::encode(control)))
                .await
                .map_err(|error| unavailable(format!("the control stream closed: {error}"))),
            Link::Ws { socket, .. } => socket
                .send(Message::Binary(abi::encode(&WsFrame::Control(
                    control.clone(),
                ))))
                .await
                .map_err(|error| unavailable(format!("the live socket closed: {error}"))),
        }
    }

    async fn recv(&mut self) -> Result<Control, Unread> {
        match self {
            Link::Quic { recv, .. } => read_control(recv).await,
            Link::Ws { socket, .. } => loop {
                match socket.next().await {
                    Some(Ok(Message::Binary(bytes))) => match abi::decode(&bytes) {
                        Ok(WsFrame::Control(control)) => return Ok(control),
                        Ok(_) => {
                            return Err(Unread::Mismatch(
                                "a data frame before the session opened".into(),
                            ));
                        }
                        Err(error) => {
                            return Err(Unread::Mismatch(undecoded("frame", &bytes, error)));
                        }
                    },
                    Some(Ok(_)) => continue,
                    _ => return Err(Unread::Closed("the live socket closed".into())),
                }
            },
        }
    }

    /// The session's tasks: the control writer and reader, and the data
    /// readers. Any of them failing closes the session.
    fn run(self, conn: Arc<Conn>) {
        match self {
            Link::Quic {
                connection,
                mut send,
                mut recv,
                mut queue,
            } => {
                let writer = conn.clone();
                tokio::spawn(async move {
                    while let Some(control) = queue.recv().await {
                        if send
                            .write_all(&framed(abi::encode(&control)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    writer.close(Close::ControlClosed);
                });
                let reader = conn.clone();
                tokio::spawn(async move {
                    let why = loop {
                        match read_control(&mut recv).await {
                            Ok(control) => {
                                if let Err(why) = reader.heard(control) {
                                    break why;
                                }
                            }
                            Err(Unread::Closed(_)) => break Close::ControlClosed,
                            Err(Unread::Mismatch(what)) => {
                                return reader.fail(mismatch(what));
                            }
                        }
                    };
                    reader.close(why);
                });
                let groups = conn.clone();
                let streams = connection.clone();
                tokio::spawn(async move {
                    while let Ok(stream) = streams.accept_uni().await {
                        tokio::spawn(read_group(groups.clone(), stream));
                    }
                    groups.close(Close::ConnectionClosed);
                });
                tokio::spawn(async move {
                    while let Ok(bytes) = connection.read_datagram().await {
                        heard_datagram(&conn, &bytes);
                    }
                });
            }
            Link::Ws {
                socket,
                queue,
                controls,
            } => run_ws(*socket, queue, controls, conn),
        }
    }
}

/// The websocket's two halves for a session. Out: frames, control first,
/// and a ping every [`KEEP_ALIVE`], so no proxy on the way sees the socket
/// idle (Cloudflare's tunnel closes one after about two minutes). In:
/// frames, and the session ends after [`IDLE`] with nothing heard, the
/// node's pongs included: the QUIC path's keep-alive and idle limit. The
/// writer stops at its first tick after the session closed.
fn run_ws<S>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    mut queue: mpsc::UnboundedReceiver<WsFrame>,
    mut controls: mpsc::UnboundedReceiver<Control>,
    conn: Arc<Conn>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = socket.split();
    let writer = conn.clone();
    tokio::spawn(async move {
        let mut ping = tokio::time::interval(KEEP_ALIVE);
        loop {
            // control first: an answer never waits behind a group
            let message = tokio::select! {
                biased;
                Some(control) = controls.recv() => {
                    Message::Binary(abi::encode(&WsFrame::Control(control)))
                }
                Some(frame) = queue.recv() => Message::Binary(abi::encode(&frame)),
                _ = ping.tick() => match writer.closed.load(Ordering::SeqCst) {
                    true => break,
                    false => Message::Ping(Vec::new()),
                },
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
        writer.close(Close::SocketClosed);
    });
    tokio::spawn(async move {
        let why = loop {
            let bytes = match tokio::time::timeout(IDLE, stream.next()).await {
                Ok(Some(Ok(Message::Binary(bytes)))) => bytes,
                Ok(Some(Ok(_))) => continue,
                Ok(_) => break Close::SocketClosed,
                Err(_) => break Close::SocketQuiet,
            };
            match abi::decode::<WsFrame>(&bytes) {
                Ok(WsFrame::Control(control)) => {
                    if let Err(why) = conn.heard(control) {
                        break why;
                    }
                }
                Ok(WsFrame::Group {
                    header,
                    objects,
                    end,
                }) => heard_slice(&conn, header, objects, end),
                Ok(WsFrame::Datagram { header, payload }) => {
                    let object = Data::Object {
                        group: header.group,
                        object: header.object,
                        body: payload,
                    };
                    conn.deliver_latest(header.alias, object);
                }
                Err(error) => {
                    let what = undecoded("frame", &bytes, error);
                    return conn.fail(mismatch(what));
                }
            }
        };
        conn.close(why);
    });
}

/// One length-prefixed control message: a stream that ends or breaks is
/// [`Unread::Closed`], bytes that are not a `Control` are
/// [`Unread::Mismatch`].
async fn read_control(recv: &mut quinn::RecvStream) -> Result<Control, Unread> {
    let closed = |error: quinn::ReadExactError| {
        Unread::Closed(format!("the control stream closed: {error}"))
    };
    let mut length = [0u8; 4];
    recv.read_exact(&mut length).await.map_err(closed)?;
    let length = u32::from_le_bytes(length) as usize;
    // Past the wire's own cap the node is not speaking the wire, and the
    // session ends.
    if length > live_wire::CONTROL_BYTES as usize {
        return Err(Unread::Mismatch(format!(
            "a control message of {length} bytes, over {}",
            live_wire::CONTROL_BYTES
        )));
    }
    let mut bytes = vec![0; length];
    recv.read_exact(&mut bytes).await.map_err(closed)?;
    abi::decode(&bytes)
        .map_err(|error| Unread::Mismatch(undecoded("control message", &bytes, error)))
}

/// One group stream from the node: its objects in order, then whether it
/// ended whole (FIN) or was cut short (RESET, or a stream that broke).
async fn read_group(conn: Arc<Conn>, mut stream: quinn::RecvStream) {
    let mut head = [0u8; GroupHeader::LEN];
    if stream.read_exact(&mut head).await.is_err() {
        return;
    }
    let Ok(header) = abi::decode::<GroupHeader>(&head) else {
        return;
    };
    let group = header.group;
    let mut object = 0u32;
    let complete = loop {
        let mut length = [0u8; 4];
        match stream.read_exact(&mut length).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(0)) => break true,
            Err(_) => break false,
        }
        let length = u32::from_le_bytes(length) as usize;
        if length > live_wire::MAX_STREAM_OBJECT {
            let _ = stream.stop(0u32.into());
            break false;
        }
        let mut body = vec![0; length];
        if stream.read_exact(&mut body).await.is_err() {
            break false;
        }
        let item = Data::Object {
            group,
            object,
            body,
        };
        // nothing follows the track any more: the node is told to stop
        let Some(readers) = conn.deliver(header.alias, &item) else {
            let _ = stream.stop(0u32.into());
            return;
        };
        for reader in readers {
            let _ = reader.send(item.clone()).await;
        }
        object += 1;
    };
    let end = Data::GroupEnd { group, complete };
    for reader in conn.deliver(header.alias, &end).unwrap_or_default() {
        let _ = reader.send(end.clone()).await;
    }
}

fn heard_datagram(conn: &Conn, bytes: &[u8]) {
    let (Some(head), Some(payload)) = (
        bytes.get(..DatagramHeader::LEN),
        bytes.get(DatagramHeader::LEN..),
    ) else {
        return;
    };
    let Ok(header) = abi::decode::<DatagramHeader>(head) else {
        return;
    };
    let object = Data::Object {
        group: header.group,
        object: header.object,
        body: payload.to_vec(),
    };
    conn.deliver_latest(header.alias, object);
}

/// A slice of a group over the websocket: its objects numbered on from the
/// slice before, and the group's end if it carries it. One socket carries
/// every track, so a subscription that is behind loses the object (counted
/// in its `Latest`) rather than holding the socket.
fn heard_slice(conn: &Conn, header: GroupHeader, objects: Vec<Vec<u8>>, end: bool) {
    let mut table = lock(&conn.table);
    let Some(id) = table.aliases.get(&header.alias).copied() else {
        return;
    };
    let Some(entry) = table.requests.get_mut(&id) else {
        return;
    };
    let hand = |entry: &mut Entry, data: Data| {
        for reader in entry.keep(&data) {
            if reader.data.try_send(data.clone()).is_err() {
                reader.latest.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
    let Kind::Subscribe { counted, .. } = &mut entry.kind else {
        return;
    };
    let mut next = match *counted {
        Some((group, next)) if group == header.group => next,
        // the node starts the next group without ending this one: it was
        // cut short (an object refused, the track ended), not malformed
        Some((group, _)) => {
            let cut = Data::GroupEnd {
                group,
                complete: false,
            };
            hand(entry, cut);
            0
        }
        None => 0,
    };
    for body in objects {
        let item = Data::Object {
            group: header.group,
            object: next,
            body,
        };
        hand(entry, item);
        next += 1;
    }
    if let Kind::Subscribe { counted, .. } = &mut entry.kind {
        *counted = (!end).then_some((header.group, next));
    }
    if end {
        let item = Data::GroupEnd {
            group: header.group,
            complete: true,
        };
        hand(entry, item);
    }
}

/// A publication's writer: once the node took it (`alias`), each object
/// out as its mode says. Over QUIC a stream-mode group is one stream, ended
/// (FIN) by `end` or by the first object of a newer group; a datagram-mode
/// object is one datagram, dropped when the path will not take it. Over the
/// websocket each is one frame.
async fn write(
    conn: Arc<Conn>,
    alias: oneshot::Receiver<Alias>,
    mut objects: mpsc::Receiver<Object>,
    mode: Mode,
) {
    let Ok(alias) = alias.await else {
        return;
    };
    let mut open: Option<(u64, Option<quinn::SendStream>)> = None;
    while let Some(Object {
        group,
        object,
        body,
        end,
    }) = objects.recv().await
    {
        if conn.closed() {
            return;
        }
        match (&conn.path, mode) {
            (Path::Quic(connection), Mode::Datagram) => {
                let mut datagram = abi::encode(&DatagramHeader {
                    alias,
                    group,
                    object,
                });
                datagram.extend(body);
                let _ = connection.send_datagram(datagram.into());
            }
            (Path::Ws(frames), Mode::Datagram) => {
                let header = DatagramHeader {
                    alias,
                    group,
                    object,
                };
                let _ = frames.send(WsFrame::Datagram {
                    header,
                    payload: body,
                });
            }
            (Path::Quic(connection), Mode::Stream) => {
                if open.as_ref().is_none_or(|(at, _)| *at != group) {
                    if let Some((_, Some(mut stream))) = open.take() {
                        let _ = stream.finish();
                    }
                    let header = abi::encode(&GroupHeader { alias, group });
                    let opened = match connection.open_uni().await {
                        Ok(mut stream) => stream.write_all(&header).await.ok().map(|_| stream),
                        Err(_) => return,
                    };
                    open = Some((group, opened));
                }
                let Some((_, Some(stream))) = open.as_mut() else {
                    continue;
                };
                if !body.is_empty() || !end {
                    if stream.write_all(&framed(body)).await.is_err() {
                        open = None;
                        continue;
                    }
                }
                if end {
                    let _ = stream.finish();
                    open = None;
                }
            }
            (Path::Ws(frames), Mode::Stream) => {
                let header = |group| GroupHeader { alias, group };
                if let Some((before, _)) = open.take_if(|(at, _)| *at != group) {
                    let _ = frames.send(WsFrame::Group {
                        header: header(before),
                        objects: Vec::new(),
                        end: true,
                    });
                }
                let objects = match body.is_empty() && end {
                    true => Vec::new(),
                    false => vec![body],
                };
                let _ = frames.send(WsFrame::Group {
                    header: header(group),
                    objects,
                    end,
                });
                open = (!end).then_some((group, None));
            }
        }
    }
    if let Some((_, Some(mut stream))) = open {
        let _ = stream.finish();
    }
}

/// QUIC to the node's live port at the host of its `/v1` address, its key
/// pinned, within [`QUIC_ANSWER`].
async fn quic(
    endpoint: &str,
    port: u16,
    identity: &[u8],
) -> Result<(quinn::Endpoint, quinn::Connection), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|error| error.to_string())?;
    let host = url.host_str().ok_or("the node's address names no host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let at = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| error.to_string())?
        .next()
        .ok_or("the node's host resolves to no address")?;
    let key: [u8; 32] = identity
        .try_into()
        .map_err(|_| "the node's identity is not an ed25519 key")?;
    let local: SocketAddr = match at {
        SocketAddr::V4(_) => (std::net::Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let udp = quinn::Endpoint::client(local).map_err(|error| error.to_string())?;
    let connecting = udp
        .connect_with(client_config(key)?, at, SERVER_NAME)
        .map_err(|error| error.to_string())?;
    let connection = tokio::time::timeout(QUIC_ANSWER, connecting)
        .await
        .map_err(|_| "no QUIC answer".to_owned())?
        .map_err(|error| error.to_string())?;
    Ok((udp, connection))
}

fn client_config(key: [u8; 32]) -> Result<quinn::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let pinned = Pinned {
        key,
        algorithms: provider.signature_verification_algorithms,
    };
    let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(pinned))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![live_wire::ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
        .map_err(|error| error.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEP_ALIVE));
    transport.max_idle_timeout(Some(
        quinn::IdleTimeout::try_from(IDLE).expect("thirty seconds is a valid idle timeout"),
    ));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// The node's certificate holds the key `/v1/status` named, and its
/// handshake is signed by it: the node's self-signed certificate is its
/// node key, as halo's are.
#[derive(Debug)]
struct Pinned {
    key: [u8; 32],
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
}

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let parsed = webpki::EndEntityCert::try_from(cert)
            .map_err(|error| rustls::Error::General(format!("the node's certificate: {error}")))?;
        let pinned = [ED25519_SPKI.as_slice(), &self.key].concat();
        match parsed.subject_public_key_info().as_ref() == pinned.as_slice() {
            true => Ok(ServerCertVerified::assertion()),
            false => Err(rustls::Error::General(
                "the node's certificate holds another key than its status names".into(),
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("live speaks only tls 1.3".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signed: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, signed, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }
}

#[cfg(test)]
mod tests;
