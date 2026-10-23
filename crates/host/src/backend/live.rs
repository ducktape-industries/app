//! What the kernel asks of a live session and hears back (requests, objects, a publication's
//! queue), and the session each connection holds: the node's client (`client.rs`, feature
//! `live-client`), or in a build without it one that refuses every request `live_unavailable`.

use super::RpcClient;
use futures::future::BoxFuture;
pub(crate) use live_wire::{Code, Mode, Priority, Start, Track};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{Notify, mpsc};
use view_wire::Error;
use view_wire::methods::refusal;

/// Stream-mode objects one subscription holds for its view. Past it the
/// group's QUIC stream waits, which holds the node back by flow control;
/// over the websocket, which carries every track on one socket, the
/// reader's group is cut instead ([`Reader::offer`]).
const DATA_BACKLOG: usize = 64;
/// Objects one publication holds: before the node took it, and while its
/// path is behind. What happens at it is the mode's rule ([`Outbox::push`]).
pub(crate) const OUTBOX: usize = 64;

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
        /// The publication's objects, as the view sends them.
        objects: Arc<Outbox>,
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

/// A publication's one queue to the node, the kernel's: the view's sends
/// go in on its thread, the session's writer takes them out, and it
/// outlives a session (a new session's writer takes over where the last
/// left). At [`OUTBOX`] the mode's rule holds, as the node's does for a
/// group it cannot take: in stream mode the group is abandoned, never a
/// single object (the queued rest of it goes, the writer resets its
/// stream, the view hears [`Pushed::Cut`] once and its later sends of the
/// group are dropped; a newer group starts clean); in datagram mode the
/// oldest queued datagram goes (the newest wins), nothing told.
pub(crate) struct Outbox {
    mode: Mode,
    state: Mutex<Queued>,
    ready: Notify,
}

#[derive(Default)]
struct Queued {
    out: VecDeque<Out>,
    /// The stream group abandoned, while the view still sends onto it.
    cut: Option<u64>,
    closed: bool,
}

/// What the writer takes from an [`Outbox`].
#[derive(Debug, PartialEq)]
pub(crate) enum Out {
    Object(Object),
    /// The group was abandoned: its stream is reset.
    Cut(u64),
}

/// What became of a pushed object.
#[derive(Debug, PartialEq)]
pub(crate) enum Pushed {
    Queued,
    /// The queue was full: the object's stream group is abandoned, this
    /// once.
    Cut(u64),
    /// Onto a group abandoned before: dropped, as told then.
    Dropped,
    /// The publication ended: nothing takes objects any more.
    Closed,
}

impl Outbox {
    pub(crate) fn new(mode: Mode) -> Arc<Outbox> {
        Arc::new(Outbox {
            mode,
            state: Mutex::new(Queued::default()),
            ready: Notify::new(),
        })
    }

    pub(crate) fn push(&self, object: Object) -> Pushed {
        let mut state = lock(&self.state);
        if state.closed {
            return Pushed::Closed;
        }
        let group = object.group;
        let queued = state
            .out
            .iter()
            .filter(|out| matches!(out, Out::Object(_)))
            .count();
        let pushed = match self.mode {
            Mode::Stream if state.cut == Some(group) => return Pushed::Dropped,
            Mode::Stream if queued >= OUTBOX => {
                state
                    .out
                    .retain(|out| !matches!(out, Out::Object(held) if held.group == group));
                state.out.push_back(Out::Cut(group));
                state.cut = Some(group);
                Pushed::Cut(group)
            }
            Mode::Stream => {
                state.cut = None;
                state.out.push_back(Out::Object(object));
                Pushed::Queued
            }
            Mode::Datagram => {
                if queued >= OUTBOX {
                    state.out.pop_front();
                }
                state.out.push_back(Out::Object(object));
                Pushed::Queued
            }
        };
        self.ready.notify_one();
        pushed
    }

    /// The next thing to write; `None` once the publication ended and the
    /// queue is empty.
    pub(crate) async fn take(&self) -> Option<Out> {
        loop {
            {
                let mut state = lock(&self.state);
                if let Some(out) = state.out.pop_front() {
                    return Some(out);
                }
                if state.closed {
                    return None;
                }
            }
            self.ready.notified().await;
        }
    }

    /// The publication ended: a push is refused, and the writer stops
    /// once it took what was queued.
    pub(crate) fn close(&self) {
        lock(&self.state).closed = true;
        self.ready.notify_one();
    }

    pub(crate) fn is_closed(&self) -> bool {
        lock(&self.state).closed
    }
}

impl std::fmt::Debug for Outbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Outbox({:?})", self.mode)
    }
}

/// Two asks of one publication name the one queue.
impl PartialEq for Outbox {
    fn eq(&self, other: &Outbox) -> bool {
        std::ptr::eq(self, other)
    }
}

/// The datagram-mode objects a subscription has not read: the newest one
/// for a view, which is the datagram's own rule (the newest wins), a
/// newer one replacing it; up to a queue's worth for a reader that keeps
/// up on its own thread ([`Latest::queue`]), the oldest going when it is
/// full. What goes unread is counted: the drop-oldest park of a reader
/// that is behind. An older one than the last kept (datagrams reorder on
/// the path) is nothing.
pub(crate) struct Latest {
    slot: Mutex<VecDeque<Data>>,
    /// How many are held at most: 1 for a view.
    holds: AtomicUsize,
    ready: Notify,
    dropped: AtomicU64,
}

impl Default for Latest {
    fn default() -> Self {
        Self {
            slot: Mutex::new(VecDeque::with_capacity(1)),
            holds: AtomicUsize::new(1),
            ready: Notify::new(),
            dropped: AtomicU64::new(0),
        }
    }
}

/// A datagram's place in its track: the publisher's numbering, exact.
fn numbered(data: &Data) -> (u64, u32) {
    match data {
        Data::Object { group, object, .. } => (*group, *object),
        Data::GroupEnd { group, .. } => (*group, u32::MAX),
    }
}

impl Latest {
    /// Holds up to `depth` unread objects from now, in order, instead of
    /// the newest one.
    pub(crate) fn queue(&self, depth: usize) {
        self.holds.store(depth.max(1), Ordering::Relaxed);
    }

    pub(crate) fn put(&self, data: Data) {
        let mut slot = lock(&self.slot);
        if let Some(last) = slot.back()
            && numbered(last) >= numbered(&data)
        {
            return;
        }
        if slot.len() >= self.holds.load(Ordering::Relaxed) {
            slot.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        slot.push_back(data);
        drop(slot);
        self.ready.notify_one();
    }

    pub(crate) async fn take(&self) -> Data {
        loop {
            if let Some(data) = lock(&self.slot).pop_front() {
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

/// One open request: what it hears. Dropping it withdraws the request
/// from the node.
pub(crate) struct Request {
    pub(crate) heard: mpsc::UnboundedReceiver<Heard>,
    pub(crate) data: mpsc::Receiver<Data>,
    pub(crate) latest: Arc<Latest>,
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
    let latest = Arc::new(Latest::default());
    let request = Request {
        heard: hears,
        data: datas,
        latest: latest.clone(),
        withdraw: Some(Box::new(withdraw)),
    };
    let ends = Ends {
        heard,
        data,
        latest,
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

#[cfg(feature = "live-client")]
mod client;

#[cfg(feature = "live-client")]
pub(crate) fn session(rpc: &RpcClient) -> Arc<dyn Session> {
    Arc::new(client::Client::new(rpc.clone()))
}

#[cfg(not(feature = "live-client"))]
pub(crate) fn session(_: &RpcClient) -> Arc<dyn Session> {
    Arc::new(NoClient)
}

/// The live session of a build without the client: each request is refused `live_unavailable`.
#[cfg(not(feature = "live-client"))]
struct NoClient;

#[cfg(not(feature = "live-client"))]
impl Session for NoClient {
    fn open(&self, _: Ask) -> BoxFuture<'static, Result<Request, Error>> {
        Box::pin(std::future::ready(Err(unavailable(
            "this build of the app has no live client",
        ))))
    }
}

#[cfg(any(feature = "call", test))]
#[cfg_attr(not(feature = "call"), allow(dead_code))]
pub(crate) mod media;

#[cfg(all(test, not(feature = "live-client")))]
mod tests;
