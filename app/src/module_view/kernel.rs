//! The kernel contract: what EVERY view may ask of the app, with no
//! per-program code on this side of the wire. A view that speaks only this
//! contract is replaced by a program deployment alone — the app binary
//! never changes for it.
//!
//! - `rpc.query` `{target, query}` — one query on the connected node: the
//!   JSON `query` is the payload of a signed frame to `target`, and the
//!   bytes the program `Respond`ed come back as they are. `rpc.view` is the
//!   same door (a program answers its own views).
//! - `op.submit` `{target, payload}` — one JSON op, signed with the SEATED
//!   key at the signer's next sequence and submitted; answered with the
//!   receipt's output, or the program's refusal. `op.submit_bytes`
//!   `{target, body_b64}` — the same with an exact binary payload.
//! - `rpc.live` `<program>` — a subscription that gets one item per block
//!   that wrote to `program` (`/v1/changes/<program>`), so the view re-reads
//!   what moved.
//! - `blob.get` `{id}` — a blob by `sha256:<hex>` or `sha1:<hex>` id, unframed.
//! - `host.visible`, `host.badge`, `host.open_link`, `host.chord`,
//!   `host.id`, `clock.ticks`, `host.log`, `host.widget` — the app's own
//!   doors: visibility, the tab badge, the one way out (a `duck://` link),
//!   a claimed command chord, a minted id, a clock, the log, a widget
//!   command.
//! - `fs.*`, `clipboard.*` — device files and the clipboard, in `filesystem`.
//!
//! A query and a submit go to the node off the window thread, on the
//! kernel's own runtime, and their answers wait in [`Replies`] for the
//! view's next redraw; reply notifications wake the native presenter.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{Guest, ModuleViewEvent, wire};
use crate::backend::{self, RpcClient, refused};

/// The longest `host.id` prefix: a word naming the kind of record, not a
/// payload of its own.
const MAX_ID_PREFIX: usize = 32;
/// The most a `blob.get` may pull into a view.
const MAX_BLOB_BYTES: usize = 16 << 20;
const MAX_IN_FLIGHT: usize = 256;
const MAX_SUBSCRIPTIONS: usize = 256;
const MAX_REPLY_EVENTS: usize = 1024;
const MAX_REPLY_BYTES: usize = 32 << 20;
/// The share of the reply budget SUBSCRIPTIONS may fill before they stop
/// reading their sources: a request answers once, a subscription forever,
/// against a queue only a redraw empties.
const MAX_STREAM_BACKLOG_EVENTS: usize = MAX_REPLY_EVENTS / 2;
const MAX_STREAM_BACKLOG_BYTES: usize = MAX_REPLY_BYTES / 2;

fn host_fault(error: impl std::fmt::Display) -> wire::Refusal {
    wire::Refusal::new("host_fault", error.to_string())
}

fn malformed(error: impl std::fmt::Display) -> wire::Refusal {
    wire::Refusal::new("malformed_request", error.to_string())
}

/// A refusal that is the node's word ends the retry loop; one the transport
/// produced is retried.
fn unanswered(refusal: wire::Refusal) -> Result<String, wire::Refusal> {
    match refusal.reason.as_str() {
        "rpc_client" | "node_failed" => Ok(refusal.sentence),
        _ => Err(refusal),
    }
}

/// One answer to a guest: the bytes it asked for, or the refusal that names
/// why not. Every door in this file hands back exactly this.
pub(super) type Answer = Result<Vec<u8>, wire::Refusal>;

/// What one answer holds against the reply budget.
fn result_bytes(result: &Answer) -> usize {
    match result {
        Ok(bytes) => bytes.len(),
        Err(refusal) => refusal.reason.len() + refusal.sentence.len(),
    }
}

/// What the queue holds against it.
fn queued_bytes(events: &[wire::Event]) -> usize {
    events
        .iter()
        .map(|event| match event {
            wire::Event::Response { result, .. } => result_bytes(result),
            _ => 0,
        })
        .sum()
}

/// The kernel's answers to a view's requests, written off-thread and
/// drained into the guest's pending events at its next redraw.
pub(super) struct Replies {
    events: Mutex<Vec<wire::Event>>,
    in_flight: AtomicUsize,
    /// Told on every answer delivered: a test waits here for the node
    /// calls in flight, never on a clock.
    landed: std::sync::Condvar,
    changed: tokio::sync::watch::Sender<()>,
    /// Told on every redraw that takes the queue: a subscription parked on
    /// [`Replies::backlogged`] wakes here and reads its socket again.
    drained: tokio::sync::watch::Sender<()>,
    fault: Mutex<Option<String>>,
}

impl Default for Replies {
    fn default() -> Self {
        Self {
            events: Mutex::default(),
            in_flight: AtomicUsize::new(0),
            landed: std::sync::Condvar::new(),
            changed: tokio::sync::watch::channel(()).0,
            drained: tokio::sync::watch::channel(()).0,
            fault: Mutex::default(),
        }
    }
}

impl Replies {
    /// Coalesced notifications wake each native presenter independently. The
    /// answer remains in the queue, including when no window is presenting it.
    pub(super) fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub(super) fn drain_into(&self, pending: &mut Vec<wire::Event>) -> Result<(), String> {
        let mut events = self.events.lock().expect("kernel replies");
        if let Some(fault) = self.fault() {
            return Err(fault);
        }
        pending.append(&mut events);
        self.drained.send_replace(());
        Ok(())
    }

    /// Told on every drain: what a parked subscription waits on.
    fn drains(&self) -> tokio::sync::watch::Receiver<()> {
        self.drained.subscribe()
    }

    /// Whether ONE subscription's share of the queue is spoken for, in
    /// either budget — a frame past this waits for a redraw rather than
    /// growing the queue toward the fault in [`Replies::item`].
    fn backlogged(&self) -> bool {
        let events = self.events.lock().expect("kernel replies");
        events.len() >= MAX_STREAM_BACKLOG_EVENTS
            || queued_bytes(&events) >= MAX_STREAM_BACKLOG_BYTES
    }

    /// One item from a SUBSCRIPTION, which is the only producer that can
    /// outrun the redraw: it answers for as long as the view holds it,
    /// against a queue only a redraw empties. It PARKS here while its share
    /// is spoken for, so its own source — a node socket, a companion
    /// session — holds the backlog instead of the queue growing into the
    /// fault. `false` when the view is gone and the subscription should end.
    async fn subscription_item(
        &self,
        drained: &mut tokio::sync::watch::Receiver<()>,
        id: u64,
        result: Answer,
    ) -> bool {
        while self.backlogged() {
            if drained.changed().await.is_err() {
                return false;
            }
        }
        self.item(id, result, false);
        self.fault().is_none()
    }

    pub(super) fn fault(&self) -> Option<String> {
        self.fault.lock().expect("kernel reply fault").clone()
    }

    fn admit(self: &std::sync::Arc<Self>) -> Option<InFlight> {
        if self.fault().is_some() {
            return None;
        }
        self.in_flight
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < MAX_IN_FLIGHT).then_some(count + 1)
            })
            .ok()?;
        Some(InFlight(self.clone()))
    }

    /// Whether a query or a submit is still on its way.
    #[cfg(test)]
    pub(super) fn any_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
    }

    /// WHETHER THE VIEW IS STILL OWED A FRAME, which is the in-flight count
    /// AND the answers already lying here. The two are one fact to a caller
    /// and reading only the count loses a race it loses often: a request is
    /// spawned inside a redraw, and a node that answers before that redraw
    /// returns has already given the count back — leaving an answer nobody
    /// is coming back for. The widget then stops polling and the view sits
    /// on "Loading…" until an unrelated event wakes it; a test's pump
    /// returns and reads a screen that never got its rows.
    ///
    /// Under the events lock, because that is the lock [`Replies::settled`]
    /// takes to give a count back: with it held, empty and zero together
    /// mean nothing can arrive that no one is waiting for.
    pub(super) fn answer_owed(&self) -> bool {
        let events = self.events.lock().expect("kernel replies");
        !events.is_empty() || self.in_flight.load(Ordering::SeqCst) > 0
    }

    /// One item for a request the kernel is running; `done` ends it for the
    /// guest. The in-flight count is [`Replies::settled`]'s to give back —
    /// a subscription's last item and its count are not the same moment.
    fn item(&self, id: u64, result: Answer, done: bool) {
        let mut events = self.events.lock().expect("kernel replies");
        if self.fault().is_some() {
            return;
        }
        let queued = queued_bytes(&events);
        let exceeds_budget = events.len() >= MAX_REPLY_EVENTS
            || result_bytes(&result) > MAX_REPLY_BYTES.saturating_sub(queued);
        if exceeds_budget {
            *self.fault.lock().expect("kernel reply fault") =
                Some("view reply backlog limit exceeded; view stopped".into());
            self.landed.notify_all();
            self.changed.send_replace(());
            return;
        }
        events.push(wire::Event::Response { id, result, done });
        self.landed.notify_all();
        self.changed.send_replace(());
    }

    /// One request off the in-flight count, under the lock a waiter holds.
    fn settled(&self) {
        let _events = self.events.lock().expect("kernel replies");
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.landed.notify_all();
        self.changed.send_replace(());
    }
}

/// The in-flight count one subscription took, given back when its task
/// ends — INCLUDING THE ABORT a cancel or a replaced view fires, which is
/// the only way a socket waiting on the node stops waiting. Without this
/// the count would outlive the socket and the widget would poll forever.
struct InFlight(std::sync::Arc<Replies>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.settled();
    }
}

#[cfg(test)]
#[test]
fn reply_notifications_wake_each_presenter_and_keep_the_answer() {
    let replies = Replies::default();
    let mut first = replies.changes();
    let mut second = replies.changes();
    replies.item(7, Ok(vec![1, 2]), true);
    futures::executor::block_on(async {
        first.changed().await.expect("first presenter notified");
        second.changed().await.expect("second presenter notified");
    });
    let mut pending = Vec::new();
    replies.drain_into(&mut pending).expect("reply budget");
    assert!(
        matches!(pending.as_slice(), [wire::Event::Response { id: 7, result: Ok(bytes), done: true }] if bytes == &[1, 2])
    );
    assert!(!replies.answer_owed());
}

#[cfg(test)]
#[test]
fn request_admission_is_bounded_and_drop_returns_capacity() {
    let replies = std::sync::Arc::new(Replies::default());
    let mut admitted: Vec<_> = (0..MAX_IN_FLIGHT)
        .map(|_| replies.admit().expect("within budget"))
        .collect();
    assert!(replies.admit().is_none());
    admitted.pop();
    let replacement = replies.admit().expect("dropped request returns capacity");
    drop(replacement);
    drop(admitted);
    assert!(!replies.any_in_flight());
}

#[cfg(test)]
#[test]
fn reply_overflow_stops_the_view_instead_of_losing_an_answer_silently() {
    let replies = std::sync::Arc::new(Replies::default());
    for id in 0..MAX_REPLY_EVENTS {
        replies.item(id as u64, Ok(Vec::new()), false);
    }
    assert!(replies.fault().is_none());
    replies.item(MAX_REPLY_EVENTS as u64, Ok(Vec::new()), true);
    assert!(replies.fault().is_some());
    assert!(replies.admit().is_none());
    let mut pending = Vec::new();
    assert!(replies.drain_into(&mut pending).is_err());
    assert!(pending.is_empty());
    assert_eq!(replies.events.lock().unwrap().len(), MAX_REPLY_EVENTS);
}

#[cfg(test)]
#[test]
fn queued_reply_bytes_are_bounded_across_individually_valid_items() {
    let replies = Replies::default();
    replies.item(1, Ok(vec![0; MAX_REPLY_BYTES]), false);
    assert!(replies.fault().is_none());
    replies.item(2, Ok(vec![1]), true);
    assert!(replies.fault().is_some());
    assert_eq!(replies.events.lock().unwrap().len(), 1);
}

/// The kernel's own runtime, on its own thread: the window thread never
/// blocks on the node, and the app's executor is not this module's to use.
pub(super) fn runtime() -> tokio::runtime::Handle {
    static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let (send, recv) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("views-kernel".into())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("the views kernel runtime");
                    send.send(runtime.handle().clone())
                        .expect("the kernel handle is taken");
                    runtime.block_on(std::future::pending::<()>());
                })
                .expect("the views kernel thread");
            recv.recv().expect("the kernel runtime came up")
        })
        .clone()
}

/// Routes one kernel request; `false` when the kind is not the kernel's.
pub(super) fn answer(
    guest: &mut Guest,
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    if super::filesystem::answer(guest, capability, operation, id, payload) {
        return true;
    }
    match (capability, operation) {
        ("host", "visible") => {
            if !payload.is_empty() {
                guest.refuse(
                    id,
                    "malformed_request",
                    "visibility subscription takes no payload",
                );
                return true;
            }
            guest.visibility_subscriptions.push(id);
            guest.pending.push(wire::Event::Response {
                id,
                result: Ok(guest.visible.to_string().into_bytes()),
                done: false,
            });
        }
        ("rpc", "query" | "view") => spawn(guest, id, payload, query),
        ("op", "submit") => spawn(guest, id, payload, submit),
        ("op", "submit_bytes") => spawn(guest, id, payload, submit_bytes),
        ("blob", "get") => spawn(guest, id, payload, blob_get),
        ("rpc", "live") => live(guest, id, payload),
        ("host", "open_link") => {
            let link = serde_json::from_slice::<serde_json::Value>(payload)
                .ok()
                .and_then(|ask| ask["link"].as_str().map(str::to_owned))
                .filter(|link| !link.is_empty());
            match link {
                Some(link) => {
                    guest.intents.push(ModuleViewEvent {
                        kind: "open_link".into(),
                        detail: serde_json::json!({ "link": link }).to_string(),
                    });
                    guest.reply(id, Ok(Vec::new()));
                }
                None => guest.refuse(id, "malformed_request", "`host.open_link` names no link"),
            }
        }
        ("host", "badge") => {
            let count = std::str::from_utf8(payload)
                .ok()
                .and_then(|text| text.trim().parse::<i64>().ok());
            match count {
                Some(count) => {
                    guest.intents.push(ModuleViewEvent {
                        kind: "badge".into(),
                        detail: format!("{{\"count\":{count}}}"),
                    });
                    guest.reply(id, Ok(Vec::new()));
                }
                None => guest.refuse(id, "malformed_request", "`host.badge` carries no count"),
            }
        }
        // A CHORD IS CLAIMED, NOT WIRED: first claim holds it.
        ("host", "chord") => {
            let chord = std::str::from_utf8(payload).unwrap_or_default().trim();
            if !is_chord(chord) {
                guest.refuse(
                    id,
                    "malformed_request",
                    format!("`{chord}` is not a claimable chord"),
                );
                return true;
            }
            if guest.chords.len() >= MAX_SUBSCRIPTIONS {
                guest.refuse(id, "subscription_limit", "too many chord claims");
                return true;
            }
            match super::claim_chord(chord, guest.module) {
                Ok(()) => guest.chords.push((id, chord.to_owned())),
                Err(holder) => guest.refuse(
                    id,
                    "chord_taken",
                    format!("`{chord}` is already {holder}'s"),
                ),
            }
        }
        ("clock", "ticks") => {
            let period = tick_period(payload);
            if guest.clocks.len() >= MAX_SUBSCRIPTIONS {
                guest.refuse(id, "subscription_limit", "too many clock subscriptions");
                return true;
            }
            match period {
                Some(period) => guest.clocks.push(Clock {
                    id,
                    period,
                    due: std::time::Instant::now() + period,
                }),
                None => guest.refuse(id, "malformed_request", "`clock.ticks` names no period"),
            }
        }
        ("host", "id") => {
            let prefix = std::str::from_utf8(payload).unwrap_or_default().trim();
            let named = !prefix.is_empty()
                && prefix.len() <= MAX_ID_PREFIX
                && prefix.bytes().all(|byte| byte.is_ascii_alphanumeric());
            match named {
                true => guest.reply(id, Ok(backend::fresh_id(prefix).into_bytes())),
                false => guest.refuse(id, "malformed_request", "`host.id` names no prefix"),
            }
        }
        _ => return false,
    }
    true
}

type Answered = std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>;
type Call = fn(Node, serde_json::Value) -> Answered;

/// The node a view's request goes to, and the network its frames name.
#[derive(Clone)]
pub(super) struct Node {
    pub(super) client: RpcClient,
    pub(super) network: String,
}

/// How long a view's node request keeps asking a node that does not answer.
const NODE_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

async fn until_answered(budget: std::time::Duration, mut call: impl FnMut() -> Answered) -> Answer {
    let deadline = tokio::time::Instant::now() + budget;
    let mut attempt = 0;
    loop {
        let detail = unanswered(match call().await {
            Ok(bytes) => return Ok(bytes),
            Err(refusal) => refusal,
        })?;
        attempt += 1;
        let delay = backend::retry_delay(attempt);
        if tokio::time::Instant::now() + delay > deadline {
            tracing::warn!(
                target: "ducktape::app",
                reason = "view_request_unanswered",
                error = %detail,
                attempts = attempt,
                "a view's node request got no answer within its retry budget"
            );
            return Err(wire::Refusal::new("rpc_client", super::NODE_UNREACHABLE));
        }
        tokio::time::sleep(delay).await;
    }
}

fn spawn(guest: &mut Guest, id: u64, payload: &[u8], call: Call) {
    let ask: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(ask) => ask,
        Err(error) => {
            guest.refuse(
                id,
                "malformed_request",
                format!("request is not JSON: {error}"),
            );
            return;
        }
    };
    let Some(node) = connected(guest, id) else {
        return;
    };
    let replies = guest.replies.clone();
    let Some(counted) = replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return;
    };
    let task = runtime().spawn(async move {
        let _counted = counted;
        let result = until_answered(NODE_RETRY_BUDGET, || call(node.clone(), ask.clone())).await;
        replies.item(id, result, true);
    });
    guest
        .tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    guest.tasks.push((id, NodeTask { task }));
}

pub(super) fn spawn_device(
    guest: &mut Guest,
    id: u64,
    future: impl std::future::Future<Output = Answer> + Send + 'static,
) {
    let replies = guest.replies.clone();
    let Some(counted) = replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return;
    };
    let task = runtime().spawn(async move {
        let _counted = counted;
        replies.item(id, future.await, true);
    });
    guest
        .tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    guest.tasks.push((id, NodeTask { task }));
}

fn connected(guest: &mut Guest, id: u64) -> Option<Node> {
    let connection = super::connection().lock().expect("views rpc");
    if connection.rev != guest.connection_rev {
        drop(connection);
        guest.refuse(
            id,
            "stale_connection",
            "view belongs to a previous network connection",
        );
        return None;
    }
    match (&connection.client, &connection.network) {
        (Some(client), network) if !network.is_empty() => Some(Node {
            client: client.clone(),
            network: network.clone(),
        }),
        _ => {
            drop(connection);
            guest.refuse(id, "not_connected", "not connected to a node");
            None
        }
    }
}

/// `rpc.live <program>`: one item per block that wrote to the program.
fn live(guest: &mut Guest, id: u64, payload: &[u8]) {
    let program = std::str::from_utf8(payload)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if program.is_empty() || guest.live_subscriptions.len() >= MAX_SUBSCRIPTIONS {
        guest.refuse(
            id,
            "malformed_request",
            "`rpc.live` names no program, or too many",
        );
        return;
    }
    let Some(node) = connected(guest, id) else {
        return;
    };
    let replies = guest.replies.clone();
    let Some(counted) = replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return;
    };
    guest.live_subscriptions.push((id, program.clone()));
    let task = runtime().spawn(async move {
        use futures::StreamExt as _;
        let _counted = counted;
        let mut drained = replies.drains();
        loop {
            let mut changes = match node.client.changes(&program).await {
                Ok(changes) => changes,
                Err(error) => {
                    tracing::debug!(target: "ducktape::app", %error, program, "changes stream not opened");
                    tokio::time::sleep(backend::retry_delay(2)).await;
                    continue;
                }
            };
            while let Some(change) = changes.next().await {
                let item = match change {
                    Ok(change) => Ok(format!("{{\"height\":{}}}", change.height).into_bytes()),
                    Err(_) => break,
                };
                if !replies.subscription_item(&mut drained, id, item).await {
                    return;
                }
            }
            // the socket closed: the node restarted or the link dropped. Say
            // so once (the view re-reads) and open it again.
            if !replies.subscription_item(&mut drained, id, Ok(b"{}".to_vec())).await {
                return;
            }
            tokio::time::sleep(backend::retry_delay(1)).await;
        }
    });
    guest.tasks.push((id, NodeTask { task }));
}

pub(super) struct NodeTask {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for NodeTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn target_of(ask: &serde_json::Value) -> Result<String, wire::Refusal> {
    let target = ask["target"].as_str().unwrap_or_default().trim();
    let named = !target.is_empty()
        && target.len() <= 64
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    match named {
        true => Ok(target.to_owned()),
        false => Err(malformed("request names no target")),
    }
}

fn query(node: Node, ask: serde_json::Value) -> Answered {
    Box::pin(async move {
        let target = target_of(&ask)?;
        let payload = serde_json::to_vec(&ask["query"]).map_err(malformed)?;
        let frame = backend::query_frame(&node.network, &target, payload).await;
        node.client
            .query(backend::Layer::Preconfirmed, frame)
            .await
            .map_err(refused)
    })
}

fn submit(node: Node, ask: serde_json::Value) -> Answered {
    Box::pin(async move {
        let target = target_of(&ask)?;
        let payload = serde_json::to_vec(&ask["payload"]).map_err(malformed)?;
        submitted(node, target, payload).await
    })
}

fn submit_bytes(node: Node, ask: serde_json::Value) -> Answered {
    Box::pin(async move {
        let target = target_of(&ask)?;
        let encoded = ask["body_b64"]
            .as_str()
            .ok_or_else(|| malformed("body_b64 must be a string"))?;
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| malformed("invalid operation base64"))?;
        submitted(node, target, payload).await
    })
}

/// The receipt's output on success, the program's refusal otherwise.
async fn submitted(node: Node, target: String, payload: Vec<u8>) -> Answer {
    let frame = backend::seated_frame(&node.client, &node.network, &target, payload).await?;
    let receipt = node.client.submit(frame).await.map_err(refused)?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => {
            Err(wire::Refusal::new(refusal.reason, refusal.sentence))
        }
    }
}

fn blob_get(node: Node, ask: serde_json::Value) -> Answered {
    Box::pin(async move {
        let id = ask["id"].as_str().unwrap_or_default();
        let (kind, hex) = id
            .split_once(':')
            .ok_or_else(|| malformed("id is `sha256:<hex>` or `sha1:<hex>`"))?;
        let digest = backend::hex_decode(hex).map_err(malformed)?;
        let id = match (kind, digest.len()) {
            ("sha256", 32) => abi::BlobId::Sha256(digest.try_into().expect("32 bytes")),
            ("sha1", 20) => abi::BlobId::Sha1(digest.try_into().expect("20 bytes")),
            _ => return Err(malformed("id is `sha256:<hex>` or `sha1:<hex>`")),
        };
        let framed = node
            .client
            .blob(id)
            .await
            .map_err(refused)?
            .ok_or_else(|| wire::Refusal::new("not_found", "the node does not hold this blob"))?;
        let body =
            backend::noded::unframe(&framed).ok_or_else(|| host_fault("blob has no header"))?;
        if body.len() > MAX_BLOB_BYTES {
            return Err(wire::Refusal::new(
                "too_large",
                "blob exceeds the view's read limit",
            ));
        }
        Ok(body.to_vec())
    })
}

pub(super) struct Clock {
    pub(super) id: u64,
    period: std::time::Duration,
    due: std::time::Instant,
}

/// WHICH CHORDS A VIEW MAY CLAIM, and why it is only these: a claim must
/// hold the platform command modifier (⌘ on a Mac, Ctrl elsewhere). A view
/// that could claim a bare letter would eat ordinary typing in every other
/// seat, and one that could claim ⇧+letter would eat capitals.
///
/// The spelling is `cmd[-shift][-alt]-<key>`, lowercase, in that order — one
/// spelling, so a claim and a press cannot disagree about how to say the
/// same chord.
pub(crate) fn chord_of(key: &str, modifiers: gpui_kit::Modifiers) -> Option<String> {
    if !backend::command_held(modifiers) {
        return None;
    }
    let key = key.trim().to_ascii_lowercase();
    let claimable = !key.is_empty()
        && key.len() <= MAX_CHORD_KEY
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if !claimable {
        return None;
    }
    let mut chord = String::from("cmd");
    if modifiers.shift {
        chord.push_str("-shift");
    }
    if modifiers.alt {
        chord.push_str("-alt");
    }
    chord.push('-');
    chord.push_str(&key);
    Some(chord)
}

/// The longest key name a chord may end in (`escape` is six).
const MAX_CHORD_KEY: usize = 12;

/// Is this the spelling [`chord_of`] would produce? A claim is refused
/// otherwise — a chord nothing can press is a view waiting forever.
fn is_chord(chord: &str) -> bool {
    let Some(rest) = chord.strip_prefix("cmd-") else {
        return false;
    };
    let rest = rest.strip_prefix("shift-").unwrap_or(rest);
    let rest = rest.strip_prefix("alt-").unwrap_or(rest);
    !rest.is_empty()
        && rest.len() <= MAX_CHORD_KEY
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The shortest and longest period a view may ask the clock for. Below the
/// floor a tick is a spin the window thread pays for every frame; above the
/// ceiling it is not a period but a date, which a view has no business
/// keeping — it reads the node for that.
const MIN_TICK_MS: i64 = 16;
const MAX_TICK_MS: i64 = 60 * 60 * 1_000;

/// A `clock.ticks` payload: the period in milliseconds, little-endian, as
/// `ui_lang_guest::every` writes it.
fn tick_period(payload: &[u8]) -> Option<std::time::Duration> {
    let millis = i64::from_le_bytes(<[u8; 8]>::try_from(payload).ok()?);
    let named = (MIN_TICK_MS..=MAX_TICK_MS).contains(&millis);
    named.then(|| std::time::Duration::from_millis(millis as u64))
}

/// Every clock item due at `now`, and the deadline re-armed for each. The
/// instant is an argument so the rule is decided, not timed: the widget
/// hands it `Instant::now()`, a test hands it the deadline it chose.
pub(super) fn ticked(clocks: &mut [Clock], now: std::time::Instant) -> Vec<wire::Event> {
    let mut items = Vec::new();
    for clock in clocks.iter_mut() {
        if clock.due > now {
            continue;
        }
        // ONE ITEM PER REDRAW, however far behind: a window that was not
        // drawn for a minute owes the view one tick, not four thousand.
        clock.due = now + clock.period;
        items.push(wire::Event::Response {
            id: clock.id,
            result: Ok(Vec::new()),
            done: false,
        });
    }
    items
}

/// When the nearest clock item comes due, for the redraw the widget asks
/// the shell to schedule.
pub(super) fn next_tick(clocks: &[Clock]) -> Option<std::time::Instant> {
    clocks.iter().map(|clock| clock.due).min()
}
