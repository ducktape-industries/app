//! The node-backed methods and how a view request becomes a task: a
//! handler is `fn(Node, Vec<u8>) -> Answered`, run on [`handle`] with an
//! in-flight slot from [`Replies::admit`], its answer put in [`Replies`];
//! the node is asked once and a transport failure is the answer. A subscription
//! (`program.changes`, `chain.heads`) is one long task writing items. Every
//! task is a [`NodeTask`] in `view.tasks`, aborted when dropped, and answers
//! through its [`Claim`] on the request, so a reconnect that takes the
//! request from it is the only one to answer it from then on.
use super::super::{consent, picker};
use super::*;
use noded_client::wire as noded;
use view_wire::methods::Method as _;

pub type Answered = std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>;
/// One node-backed method: the node to ask and the request's payload in,
/// the answer out. Not a [`methods::Call`], which is `program.query`'s and
/// `op.submit`'s envelope.
type NodeMethod = fn(Node, Vec<u8>) -> Answered;

/// The node a view's request goes to, and the network its frames name.
#[derive(Clone)]
pub(super) struct Node {
    pub(super) client: RpcClient,
    pub(super) network: String,
}

/// A refusal the transport produced, not the node's own word.
pub(super) fn transport_failed(refusal: &wire::Error) -> bool {
    matches!(
        refusal.code.as_str(),
        refusal::RPC_CLIENT | refusal::NODE_FAILED
    )
}

/// Runs `method` on the connected node as a task, asking once: a transport
/// failure is the answer, and the view decides whether to ask again.
pub(super) fn spawn_asked(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
    stage: &'static str,
) {
    spawn_method(
        view,
        id,
        payload,
        move |node, ask, _| method(node, ask),
        stage,
        None,
    );
}

/// `op.submit` with no person to ask: [`spawn_asked`] with [`submit`],
/// whose frame, once it goes out, answers in doubt if a reconnect takes
/// the request from its task ([`reconnected`]).
pub(super) fn spawn_submit(view: &mut View, id: u64, payload: &[u8]) {
    spawn_method(view, id, payload, submit, "host_call.op.submit", None);
}

/// `op.submit` of an op the person confirms first (`consent::needed`):
/// only the person's activation opens the dialog, taken here at the press
/// as `link.open` takes it (a view resubmitting after Cancel, or asking on
/// a clock, gets `needs_gesture` and no second card); refused
/// `session_locked` at once with no key to sign it (no dialog that cannot
/// lead to a signature); else its task asks the person ([`Asking`]) and
/// runs it as [`spawn_asked`] once they approve, refused
/// `consent_refused` otherwise.
pub(super) fn spawn_consented(
    view: &mut View,
    id: u64,
    payload: &[u8],
    call: methods::Call,
    words: consent::Words,
) {
    if !view.take_activation() {
        view.refuse(
            id,
            refusal::NEEDS_GESTURE,
            "this op asks the person and needs a press or key",
        );
        return;
    }
    if !backend::seated() {
        view.refuse(
            id,
            refusal::SESSION_LOCKED,
            "this device's key is locked; unlock it first",
        );
        return;
    }
    if connected(view, id).is_none() {
        return;
    }
    let asking = Asking {
        waiting: consent::waiting(),
        program: view.program,
        instance: view.instance,
        words,
        call,
    };
    spawn_method(
        view,
        id,
        payload,
        submit,
        "host_call.op.submit",
        Some(asking),
    );
}

/// A consented op's ask, made by its task: the card goes up only once its
/// words are whole, and goes with the task (aborted with its view, the ask
/// is never queued or is withdrawn).
struct Asking {
    /// the queue the request came in on (`consent::queue_in`)
    waiting: &'static std::sync::Mutex<std::collections::VecDeque<consent::Ask>>,
    program: &'static str,
    instance: u64,
    words: consent::Words,
    call: methods::Call,
}

impl Asking {
    /// The person's yes: the op described into the words where they show
    /// the target's own description (its size when the target's describe
    /// program reads nothing or fails), the ask queued and
    /// the shell woken, and its answer. `consent_refused` for anything but
    /// Approve, and while another ask of this view waits.
    async fn yes(self, node: &Node) -> Result<(), wire::Error> {
        let Asking {
            waiting,
            program,
            instance,
            mut words,
            call,
        } = self;
        if words.described {
            let len = call.body.len();
            let described = super::describe::described(node, &call.target, call.body)
                .await
                .ok()
                .flatten();
            words.describe(described.as_ref(), len);
        }
        let refused = |message: &str| wire::Error::new(refusal::CONSENT_REFUSED, message);
        let Some(mut told) = consent::queue_in(waiting, program, instance, words) else {
            return Err(refused(
                "another confirmation for this view is still waiting",
            ));
        };
        consent::moved();
        match told.answer().await {
            Some(true) => Ok(()),
            _ => Err(refused("the person did not confirm")),
        }
    }
}

/// `stage` names the call in the perf registry (`host_call.<kind>`): its
/// latency from here to the reply. `asking`, when given, is the person's yes the task asks for before it asks the
/// node at all ([`Asking::yes`]).
#[allow(clippy::too_many_arguments, reason = "one spawn, every node method")]
fn spawn_method(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: impl FnOnce(Node, Vec<u8>, Claim) -> Answered + Send + 'static,
    stage: &'static str,
    asking: Option<Asking>,
) {
    let ask = payload.to_vec();
    let Some(node) = connected(view, id) else {
        return;
    };
    let slot = view.replies.admit();
    let replies = view.replies.clone();
    let claim = Claim::default();
    let held = claim.clone();
    let key = view.perf_key();
    // not a `perf::Timer`: its drop would record a call cancelled by the
    // view, which never got a reply
    let asked = crate::perf::on().then(std::time::Instant::now);
    run(view, id, slot, None, claim, async move {
        if let Some(asking) = asking
            && let Err(refused) = asking.yes(&node).await
        {
            replies.answer(&held, id, Err(refused), true);
            return;
        }
        let result = method(node, ask, held.clone()).await;
        if let Some(asked) = asked {
            crate::perf::record(key, stage, asked.elapsed().as_micros() as u64);
        }
        replies.answer(&held, id, result, true);
    });
}

/// Every task starts here: an in-flight slot ([`Replies::admit`]), then
/// [`run`]. The task answers request `id` through `claim`
/// ([`Replies::answer`]).
fn start(
    view: &mut View,
    id: u64,
    follows: Option<Follow>,
    claim: Claim,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let slot = view.replies.admit();
    run(view, id, slot, follows, claim, task);
}

/// `task` on [`handle`] holding its `slot`, its [`NodeTask`] kept in
/// `view.tasks` (finished ones pruned) so it is aborted with the view.
fn run(
    view: &mut View,
    id: u64,
    slot: InFlight,
    follows: Option<Follow>,
    claim: Claim,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let task = handle().spawn(async move {
        let _slot = slot;
        task.await;
    });
    view.tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    view.tasks.push((
        id,
        NodeTask {
            task,
            follows,
            claim,
        },
    ));
}

/// One SUBSCRIPTION's writing end, handed to the loop that feeds it: every
/// item goes through the same park as a node socket's, so a source that
/// outruns the redraw waits for the redraw that takes its item.
pub(in crate::runtime) struct Items {
    replies: std::sync::Arc<Replies>,
    drained: tokio::sync::watch::Receiver<()>,
    claim: Claim,
    id: u64,
}

impl Items {
    /// One more item. `false` when the view is gone or the subscription
    /// was taken from this task (a reconnect opened it again): it should
    /// end — the caller returns, and everything it holds is dropped.
    pub(in crate::runtime) async fn send(&mut self, result: Answer) -> bool {
        self.replies
            .subscription_item(&mut self.drained, &self.claim, self.id, result)
            .await
    }

    /// The refusal that ends the subscription.
    pub(super) fn last(&self, refusal: wire::Error) {
        self.replies
            .answer(&self.claim, self.id, Err(refusal), true);
    }
}

/// A subscription whose items come from a host loop (`chain.heads` polls
/// the node for them). The body owns whatever it holds open, so dropping
/// the task releases it: a cancel drops the [`NodeTask`], and so do a
/// view's teardown, swap and trap, which drop the whole `tasks` list.
pub(super) fn spawn_subscription<Body, Fut>(view: &mut View, id: u64, follows: Follow, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let replies = view.replies.clone();
    let claim = Claim::default();
    let items = Items {
        drained: replies.drains(),
        replies,
        claim: claim.clone(),
        id,
    };
    start(view, id, Some(follows), claim, body(items));
}

/// A subscription fed by the app itself, not the node (`store.changes`,
/// `host.screen`): a reconnect leaves it running ([`Follow::Host`]).
pub(in crate::runtime) fn spawn_host_subscription<Body, Fut>(view: &mut View, id: u64, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    spawn_subscription(view, id, Follow::Host, body);
}

/// Runs `future` on the kernel runtime and delivers its result as the one
/// reply to request `id`; counted in flight, aborted with the view. For a
/// host method that waits on something other than the node (`notify.post`
/// waits on its banner, `store.*` on its file).
pub(in crate::runtime) fn spawn_reply(
    view: &mut View,
    id: u64,
    future: impl std::future::Future<Output = Answer> + Send + 'static,
) {
    let replies = view.replies.clone();
    let claim = Claim::default();
    let held = claim.clone();
    start(view, id, None, claim, async move {
        replies.answer(&held, id, future.await, true);
    })
}

/// The node to ask, the one the view's own connection names, or `None`
/// with the request REFUSED: `stale_connection` for a view from before the
/// app's last (re)connect, `not_connected` without a node.
pub(super) fn connected(view: &mut View, id: u64) -> Option<Node> {
    let rev = super::super::connection().lock().expect("views rpc").rev;
    if rev != view.connection.rev {
        view.refuse(
            id,
            refusal::STALE_CONNECTION,
            "view belongs to a previous network connection",
        );
        return None;
    }
    let node = node_on(&view.connection);
    if node.is_none() {
        view.refuse(id, refusal::NOT_CONNECTED, "not connected to a node");
    }
    node
}

/// The node `connection` names, if it names one.
fn node_on(connection: &super::super::Connection) -> Option<Node> {
    match (&connection.client, &connection.network) {
        (Some(client), network) if !network.is_empty() => Some(Node {
            client: client.clone(),
            network: network.clone(),
        }),
        _ => None,
    }
}

/// A connect inside the network, under a view that kept its code
/// (`View::reconnect`): each node subscription goes on, opened again on
/// `connection`, and its follower with it — a `program.changes` one is told
/// first to re-read (`None`, as when the node closes the socket), a
/// `chain.heads` one starts over at the new node's tip. A submit whose frame
/// went out answers in doubt (`Submitted::InDoubt`: the old node may hold
/// it); every other request still running on the old connection is refused
/// `stale_connection`, for the view to ask again. A request whose task had
/// already ended it is left ended: each id ends once.
pub(in crate::runtime) fn reconnected(view: &mut View, connection: &super::super::Connection) {
    // counted again as each opens
    view.live_subscriptions.clear();
    for (id, mut task) in std::mem::take(&mut view.tasks) {
        // the app's own feed has no socket on the old connection
        if matches!(task.follows, Some(Follow::Host)) {
            view.tasks.push((id, task));
            continue;
        }
        // taken from the task before anything is answered for it: a
        // request its task already ended (a subscription the old node
        // ended, a reply not yet pruned) stays ended, and whatever the
        // aborting task still sends is dropped
        let Some(standing) = view.replies.take(&task.claim) else {
            continue;
        };
        let follows = task.follows.take();
        // dropped, it aborts: the old connection's socket goes with it
        drop(task);
        match (follows, node_on(connection), standing) {
            (Some(Follow::Changes(program)), Some(node), _) => {
                view.replies
                    .item(id, Ok(methods::encode(&None::<methods::Change>)), false);
                follow_changes(view, id, node, program);
            }
            (Some(Follow::Heads), Some(node), _) => follow_heads(view, id, node),
            (Some(Follow::Live(follows)), Some(_), _) => super::live::reopen(view, id, follows),
            // a submit whose frame went out: the old node may hold it
            (None, _, Some(standing)) => view.reply(id, standing),
            _ => view.refuse(id, refusal::STALE_CONNECTION, "network connection changed"),
        }
    }
}

/// `program.changes <program>`: one item per block that wrote to the program,
/// carrying the keys it wrote.
pub(super) fn changes(view: &mut View, id: u64, payload: &[u8]) {
    let program = methods::decode::<String>(payload)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if program.is_empty() {
        view.refuse(
            id,
            refusal::MALFORMED_REQUEST,
            "`program.changes` names no program",
        );
        return;
    }
    if !view.targets.contains(&program) {
        view.refuse(
            id,
            refusal::UNDECLARED_TARGET,
            undeclared("program.changes", &program),
        );
        return;
    }
    let Some(node) = connected(view, id) else {
        return;
    };
    follow_changes(view, id, node, program);
}

/// Subscription `id`: `program.changes` of `program` on `node`.
fn follow_changes(view: &mut View, id: u64, node: Node, program: String) {
    let replies = view.replies.clone();
    let subscribed = (id, program.clone());
    let follows = Follow::Changes(program.clone());
    let claim = Claim::default();
    let held = claim.clone();
    start(view, id, Some(follows), claim, async move {
        use futures::StreamExt as _;
        let mut drained = replies.drains();
        loop {
            let mut changes = match node.client.changes(&program).await {
                Ok(changes) => changes,
                Err(error) => {
                    tracing::debug!(target: "ducktape::app", %error, program, "changes stream not opened");
                    replies.answer(&held, id, Err(refused(error)), true);
                    return;
                }
            };
            while let Some(change) = changes.next().await {
                // what the node publishes, as it publishes it: the height and
                // the keys the block wrote (the values stay on the node)
                let item = match change {
                    Ok(change) => Ok(methods::encode(&Some(methods::Change {
                        height: change.height,
                        keys: change.writes.into_iter().map(|(key, _)| key).collect(),
                    }))),
                    Err(_) => break,
                };
                if !replies
                    .subscription_item(&mut drained, &held, id, item)
                    .await
                {
                    return;
                }
            }
            // the socket closed: the node restarted or the link dropped. Say
            // so once (the view re-reads) and open it again; a node that
            // does not take it again ends the subscription with why.
            if !replies
                .subscription_item(
                    &mut drained,
                    &held,
                    id,
                    Ok(methods::encode(&None::<methods::Change>)),
                )
                .await
            {
                return;
            }
        }
    });
    view.live_subscriptions.push(subscribed);
}

/// `chain.heads`: one [`methods::Head`] per finalized block, oldest first. The
/// node pushes no block stream, so this reads its status at half the block
/// time and fills each advance from the archive, page after page as the
/// node answers them; where the archive holds none, the tip alone. A read
/// the node does not answer is read again at the same pace; a first read
/// it does not answer ends the subscription with why.
pub(super) fn heads(view: &mut View, id: u64, payload: &[u8]) {
    if !payload.is_empty() {
        view.refuse(
            id,
            refusal::MALFORMED_REQUEST,
            "`chain.heads` takes no payload",
        );
        return;
    }
    let Some(node) = connected(view, id) else {
        return;
    };
    follow_heads(view, id, node);
}

/// Subscription `id`: `chain.heads` on `node`.
fn follow_heads(view: &mut View, id: u64, node: Node) {
    spawn_subscription(view, id, Follow::Heads, move |mut items| async move {
        let mut last: Option<u64> = None;
        let mut block_time_ms = None;
        loop {
            let read = next_heads(&node, last).await;
            let pace = match (read, block_time_ms) {
                (Ok((heads, read_block_time_ms)), _) => {
                    block_time_ms = Some(read_block_time_ms);
                    for head in heads {
                        last = Some(head.height);
                        if !items.send(Ok(methods::encode(&head))).await {
                            return;
                        }
                    }
                    read_block_time_ms
                }
                (Err(error), Some(known)) => {
                    tracing::debug!(target: "ducktape::app", %error, "heads not read");
                    known
                }
                (Err(error), None) => {
                    items.last(refused(error));
                    return;
                }
            };
            tokio::time::sleep(std::time::Duration::from_millis(pace / 2)).await;
        }
    });
}

/// The heads past `last` (the tip alone at the first read), oldest first,
/// and the node's block time.
async fn next_heads(
    node: &Node,
    last: Option<u64>,
) -> noded_client::Result<(Vec<methods::Head>, u64)> {
    let status = node.client.status().await?;
    let tip = status.height;
    if last.is_some_and(|last| last >= tip) {
        return Ok((Vec::new(), status.block_time_ms));
    }
    let floor = last.map_or(tip, |last| last + 1);
    let before = tip.saturating_add(1);
    let page = noded::Blocks {
        before: Some(before),
        limit: u32::try_from(before - floor).unwrap_or(u32::MAX),
        most: 0,
    };
    let mut found = node.client.blocks(&page).await?.blocks;
    found.sort_by_key(|block| block.height);
    let mut heads: Vec<methods::Head> = found
        .into_iter()
        .map(|block| methods::Head {
            height: block.height,
            time: block.time,
            id: block.id,
        })
        .collect();
    if heads.last().is_none_or(|head| head.height < tip) {
        heads.push(methods::Head {
            height: tip,
            time: 0,
            id: status.tip,
        });
    }
    Ok((heads, status.block_time_ms))
}

/// A running request or subscription; dropping it aborts the task, which is
/// how a cancel, a swap or a teardown stops a socket waiting on the node.
pub(in crate::runtime) struct NodeTask {
    task: tokio::task::JoinHandle<()>,
    /// What a node subscription follows, which [`reconnected`] opens again
    /// on a new connection; `None` for a request answered once.
    follows: Option<Follow>,
    /// The task's hold on its request, which a reconnect takes from it.
    claim: Claim,
}

/// A subscription, as a reconnect opens it again.
pub(super) enum Follow {
    /// `program.changes` of this program.
    Changes(String),
    /// `chain.heads`.
    Heads,
    /// A `live.*` request (`live.rs`).
    Live(super::live::LiveFollow),
    /// The app's own feed: it goes on as it is.
    Host,
}

impl Drop for NodeTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The envelope of a node method, its target checked: a program id
/// (`program::is_name`, the rule a manifest's targets follow), not a
/// path.
fn call_of(ask: &[u8]) -> Result<methods::Call, wire::Error> {
    let call: methods::Call = methods::decode(ask).map_err(malformed)?;
    let target = call.target.trim();
    match program::is_name(target) {
        true => Ok(methods::Call {
            target: target.to_owned(),
            body: call.body,
        }),
        false => Err(malformed("request names no target")),
    }
}

fn undeclared(kind: &str, target: &str) -> String {
    format!("`{kind}` names `{target}`, which this view's manifest does not list among its targets")
}

/// The envelope of `program.query` or `op.submit` (`kind`) from `view`, its
/// target among the view's manifest targets; `None` with the request
/// refused otherwise (`malformed_request`, `undeclared_target`). The task
/// decodes the envelope again: a string and the bytes, cheaper than
/// carrying it.
pub(super) fn targeted(
    view: &mut View,
    id: u64,
    payload: &[u8],
    kind: &str,
) -> Option<methods::Call> {
    let call = match call_of(payload) {
        Ok(call) => call,
        Err(refusal) => {
            view.reply(id, Err(refusal));
            return None;
        }
    };
    if !view.targets.contains(&call.target) {
        view.refuse(
            id,
            refusal::UNDECLARED_TARGET,
            undeclared(kind, &call.target),
        );
        return None;
    }
    Some(call)
}

pub(super) fn query(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let call = call_of(&ask)?;
        let frame = backend::query_frame(&node.network, &call.target, call.body).await?;
        node.client
            .query(backend::Layer::Preconfirmed, frame)
            .await
            .map_err(refused)
    })
}

/// `op.submit`; `claim` is its request's, which it leaves in doubt once
/// the frame goes out ([`submitted`]).
pub(super) fn submit(node: Node, ask: Vec<u8>, claim: Claim) -> Answered {
    Box::pin(async move {
        let call = call_of(&ask)?;
        submitted(node, call.target, call.body, claim).await
    })
}

/// What the node made of the frame, as `methods::Submitted`: every arm
/// carries the frame's id (`Body::id`) and the last height that may carry
/// it (`Body::until`). The node's preconfirm receipt is `Applied` or
/// `Refused` (pooled, the refusal a prediction). An answer that went
/// missing after the bytes went out (the transport, a status this client
/// does not read, a reply it cannot decode) is `InDoubt`: the node may hold
/// the frame. `Err` is only for a frame nothing pooled: unsigned, unsent
/// (`rpc_client`), or refused outright by the node.
async fn submitted(node: Node, target: String, payload: Vec<u8>, claim: Claim) -> Answer {
    let frame = backend::seated_frame(&node.client, &node.network, &target, payload)
        .await
        .map_err(|refusal| match transport_failed(&refusal) {
            true => wire::Error::new(refusal::RPC_CLIENT, refusal.message),
            false => refusal,
        })?;
    let (id, until) = (frame.body.id(), frame.body.until());
    // from here the node may hold the frame: a request taken from this
    // task now (a reconnect) answers in doubt, as a lost answer does
    claim.stand(Ok(methods::encode(&methods::Submitted::InDoubt {
        frame: id,
        until,
    })));
    let submitted = match node.client.preconfirm(frame.encode()).await {
        Ok(receipt) => match receipt.outcome {
            abi::Outcome::Applied { output } => methods::Submitted::Applied {
                frame: id,
                until,
                output,
            },
            abi::Outcome::Rejected(refusal) => methods::Submitted::Refused {
                frame: id,
                until,
                refusal: wire::Error::new(refusal.reason, refusal.sentence),
            },
        },
        Err(error @ (noded_client::Error::Refused(_) | noded_client::Error::Unreachable(_))) => {
            return Err(refused(error));
        }
        Err(
            noded_client::Error::Transport(_)
            | noded_client::Error::Failed { .. }
            | noded_client::Error::Decode(_),
        ) => methods::Submitted::InDoubt { frame: id, until },
    };
    Ok(methods::encode(&submitted))
}

/// One blob of an `op.put`, checked at the ask: its kind, and its bytes,
/// read where they are each time they are needed.
pub(super) struct Blob {
    kind: String,
    body: Body,
}

enum Body {
    Bytes(Vec<u8>),
    /// a slice of a file of the latest pick, within its size as picked
    Picked {
        file: std::sync::Arc<picker::Picked>,
        offset: u64,
        len: u64,
    },
}

impl Blob {
    /// The blob framed (`kind len\0body`), a picked slice read on a
    /// blocking thread: a file whose size changed since the pick is
    /// refused `host_fault` (`picker::slice`).
    async fn framed(&self) -> Result<Vec<u8>, wire::Error> {
        let read;
        let body = match &self.body {
            Body::Bytes(bytes) => bytes,
            Body::Picked { file, offset, len } => {
                let (file, offset, len) = (file.clone(), *offset, *len);
                read = tokio::task::spawn_blocking(move || picker::slice(&file, offset, len))
                    .await
                    .map_err(host_fault)??;
                &read
            }
        };
        blob::frame(&self.kind, body).map_err(|refusal| malformed(refusal.sentence))
    }
}

/// `op.put`'s blobs, checked at the ask: no blob, one over
/// [`methods::BLOB_MOST`], a kind the node does not take, or a slice of no
/// file of the latest pick (or past its end) is `malformed_request`.
fn blobs_of(view: &View, payload: &[u8]) -> Result<Vec<Blob>, wire::Error> {
    let put: methods::PutBlobs = methods::decode(payload).map_err(malformed)?;
    if put.blobs.is_empty() {
        return Err(malformed("`op.put` names no blob"));
    }
    let blob = |methods::NewBlob { kind, body }| {
        if !blob::is_kind(&kind) {
            return Err(malformed(
                "a blob's kind is one word, without spaces or NUL",
            ));
        }
        let (body, len) = match body {
            methods::BlobBody::Bytes(bytes) => {
                let len = bytes.len() as u64;
                (Body::Bytes(bytes), len)
            }
            methods::BlobBody::Picked { file, offset, len } => {
                let picked = view
                    .picker
                    .picked(file)
                    .ok_or_else(|| malformed(format!("file {file} is not of the latest pick")))?;
                if offset.checked_add(len).is_none_or(|end| end > picked.size) {
                    return Err(malformed(format!(
                        "{len} bytes from {offset} run past file {file}'s {} bytes",
                        picked.size
                    )));
                }
                (
                    Body::Picked {
                        file: picked,
                        offset,
                        len,
                    },
                    len,
                )
            }
        };
        if len > methods::BLOB_MOST {
            return Err(malformed(format!(
                "a blob of {len} bytes is over the {} one holds",
                methods::BLOB_MOST
            )));
        }
        Ok(Blob { kind, body })
    };
    put.blobs.into_iter().map(blob).collect()
}

/// `op.put`: its blobs checked here ([`blobs_of`]), then put by its task
/// ([`put`]), which goes with the view as every node task does.
pub(super) fn spawn_put(view: &mut View, id: u64, payload: &[u8]) {
    let blobs = match blobs_of(view, payload) {
        Ok(blobs) => blobs,
        Err(refusal) => return view.reply(id, Err(refusal)),
    };
    // the blobs are taken: the payload is not carried again
    let put = move |node, _, _| -> Answered { Box::pin(put(node, blobs)) };
    spawn_method(view, id, &[], put, "host_call.op.put", None);
}

/// `op.put`'s task. A hash pass (each blob read, framed, hashed, dropped)
/// makes the bundle's `Put`, which the seated key signs to the program
/// the node binds to the blobs role; a stream pass reads each blob again
/// into the node's blob upload after it, so no pass holds the bundle
/// whole. A file that changed between the passes no longer hashes to its
/// id, and the node's refusal of it is the answer. The answer, each blob's
/// sha256 digest in order, waits for the network to hold the bundle.
pub(super) async fn put(node: Node, blobs: Vec<Blob>) -> Answer {
    use futures::StreamExt as _;
    use sha2::Digest as _;
    let (mut ids, mut entries) = (Vec::with_capacity(blobs.len()), Vec::new());
    for blob in &blobs {
        let framed = blob.framed().await?;
        let digest: [u8; 32] = sha2::Sha256::digest(&framed).into();
        ids.push(digest);
        entries.push(blob::contract::Entry {
            id: abi::BlobId::Sha256(digest),
            len: framed.len() as u64,
            kind: blob.kind.clone(),
        });
    }
    let roles = node.client.status().await.map_err(refused)?.roles;
    let target = roles
        .bound(abi::role::BLOBS)
        .map_err(|refusal| wire::Error::new(refusal.reason, refusal.sentence))?
        .clone();
    let op = blob::contract::Op::Put { blobs: entries };
    let frame =
        backend::seated_frame(&node.client, &node.network, &target, abi::encode(&op)).await?;
    // both followed from before the upload, so nothing between is missed
    let mut blocks = node.client.changes(noded::RECENT).await.map_err(refused)?;
    let mut bundles = node.client.changes(&target).await.map_err(refused)?;
    let unread = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sent = futures::stream::iter(blobs).then({
        let unread = unread.clone();
        move |blob| {
            let unread = unread.clone();
            async move {
                blob.framed().await.map_err(|refusal| {
                    let error = std::io::Error::other(refusal.message.clone());
                    *unread.lock().expect("an unread blob") = Some(refusal);
                    error
                })
            }
        }
    });
    if let Err(error) = node.client.put_blobs(frame.encode(), sent).await {
        // a blob that could not be read again ended the upload: its refusal
        let unread = unread.lock().expect("an unread blob").take();
        return Err(unread.unwrap_or_else(|| refused(error)));
    }
    let bundle = landed(&node, &mut blocks, frame.body.id(), frame.body.until()).await?;
    drop(blocks);
    let key = blob::contract::bundle_key(bundle);
    while let Some(change) = bundles.next().await {
        let held = change
            .map_err(refused)?
            .writes
            .into_iter()
            .any(|(written, value)| {
                written == key && blob::contract::held_since(value.as_deref()).is_some()
            });
        if held {
            return Ok(methods::encode(&ids));
        }
    }
    Err(wire::Error::new(
        refusal::NODE_FAILED,
        format!(
            "the node stopped telling the blobs program's changes before bundle {bundle} was held"
        ),
    ))
}

/// The bundle the block that carries frame `id` put, read off `blocks` (a
/// change a block): the program's refusal where it rejected the put, and
/// `anchor` once a block at `until` (the last that may carry the frame)
/// came without it.
async fn landed(
    node: &Node,
    blocks: &mut (impl futures::Stream<Item = noded_client::Result<noded::Change>> + Unpin),
    id: [u8; 32],
    until: u64,
) -> Result<u64, wire::Error> {
    use futures::StreamExt as _;
    let ask = noded::TxAsk {
        tx: noded::TxRef::Hash(id),
        most: 0,
    };
    while let Some(change) = blocks.next().await {
        let height = change.map_err(refused)?.height;
        let located = node.client.tx(&ask).await.map_err(refused)?;
        if let Some(receipt) = located.and_then(|located| located.tx.receipt) {
            return match receipt.outcome {
                abi::Outcome::Applied { output } => abi::decode(&output).map_err(|refusal| {
                    wire::Error::new(view_wire::code::UNEXPECTED_REPLY, refusal.sentence)
                }),
                abi::Outcome::Rejected(refusal) => {
                    Err(wire::Error::new(refusal.reason, refusal.sentence))
                }
            };
        }
        if height >= until {
            return Err(wire::Error::new(
                abi::reason::ANCHOR,
                format!(
                    "no block carried the put by block {until}, the last its frame may land at"
                ),
            ));
        }
    }
    Err(wire::Error::new(
        refusal::NODE_FAILED,
        "the node stopped telling its blocks before the put landed",
    ))
}

/// The blob id a `blob.*` ask spells, `sha256:<hex>` or `sha1:<hex>`.
fn blob_id(ask: &[u8]) -> Result<abi::BlobId, wire::Error> {
    let id: String = methods::decode(ask).map_err(malformed)?;
    let (kind, hex) = id
        .split_once(':')
        .ok_or_else(|| malformed("id is `sha256:<hex>` or `sha1:<hex>`"))?;
    let digest = noded_client::hex::hex_decode(hex).map_err(malformed)?;
    Ok(match (kind, digest.len()) {
        ("sha256", 32) => abi::BlobId::Sha256(digest.try_into().expect("32 bytes")),
        ("sha1", 20) => abi::BlobId::Sha1(digest.try_into().expect("20 bytes")),
        _ => return Err(malformed("id is `sha256:<hex>` or `sha1:<hex>`")),
    })
}

/// `blob.manifest`: a blob the programs role names (a listed program's
/// code, a view-only entry's, or one a scheduled change brings: the
/// scheduled list is read for the ask, never per block), read as a seat
/// reads it (`views::program_bytes`, cached under its id); the view in its
/// `ducktape.view` section or the blob itself for a view-only entry, and
/// that view's manifest as `manifest_of` would read it. A blob is a view by
/// its manifest section, read or not: one this host cannot read is a view
/// with no manifest, never a program that draws nothing. The bytes stay on
/// the host. An id the roster does not name is refused as
/// `undeclared_target`, never fetched: `None` is the node's word that it
/// holds no such blob, and a view asks the host only about code the chain
/// names, as a node method names only a program its manifest declares.
pub(super) fn blob_manifest(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        use backend::views::{Fetch, VIEW_SECTION, program_bytes, section};
        use view_wire::manifest::{MANIFEST_SECTION, read_manifest};
        let id = blob_id(&ask)?;
        let named = super::super::roster::roster().names_code(&id)
            || match backend::views::coming(&node.client, &node.network).await {
                Ok(coming) => coming.contains(&id),
                Err(Fetch::Unreachable(reason)) => {
                    return Err(wire::Error::new(refusal::RPC_CLIENT, reason));
                }
                Err(error) => return Err(host_fault(error)),
            };
        if !named {
            return Err(wire::Error::new(
                refusal::UNDECLARED_TARGET,
                "the programs role names no such code",
            ));
        }
        let bytes = match program_bytes(&node.client, &id).await {
            Ok((bytes, _)) => bytes,
            Err(Fetch::NotHeld) => return Ok(methods::encode(&None::<methods::Manifested>)),
            Err(Fetch::Unreachable(reason)) => {
                return Err(wire::Error::new(refusal::RPC_CLIENT, reason));
            }
            Err(error) => return Err(host_fault(error)),
        };
        let view = section(&bytes, VIEW_SECTION)
            // no section: the blob is a view itself, or carries none
            .or_else(|| section(&bytes, MANIFEST_SECTION).map(|_| &bytes[..]))
            .map(|view| methods::CarriedView {
                size: view.len() as u64,
                manifest: read_manifest(view),
            });
        Ok(methods::encode(&Some(methods::Manifested {
            size: bytes.len() as u64,
            view,
        })))
    })
}

pub(super) fn blob_get(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let id = blob_id(&ask)?;
        // absent is `None`, never a refusal: the ask itself did not fail
        let Some(framed) = node.client.blob(id).await.map_err(refused)? else {
            return Ok(methods::encode(&None::<Vec<u8>>));
        };
        let body =
            noded_client::wire::unframe(&framed).ok_or_else(|| host_fault("blob has no header"))?;
        Ok(methods::encode(&Some(body.to_vec())))
    })
}

pub(super) fn status(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        if !ask.is_empty() {
            return Err(malformed("chain.status takes no payload"));
        }
        let status = node.client.status().await.map_err(refused)?;
        Ok(methods::encode(&methods::NodeStatus {
            chain_id: status.network,
            time: status.time,
            block_time_ms: status.block_time_ms,
            epoch_length: status.epoch_length,
            height: status.height,
            tip: status.tip,
            root: status.root.0,
            epoch: status.epoch,
            identity: status.identity,
            contract: status.contract,
        }))
    })
}

pub(super) fn network(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        if !ask.is_empty() {
            return Err(malformed("chain.network takes no payload"));
        }
        let network = node.client.network().await.map_err(|error| match error {
            noded_client::Error::Failed { status: 404, .. } => wire::Error::new(
                refusal::UNKNOWN_REQUEST,
                "This node doesn't report its validators' votes. Update the node.",
            ),
            error => refused(error),
        })?;
        let members = network
            .members
            .into_iter()
            .map(|peer| methods::Peer {
                key: peer.key,
                signed: peer.signed,
            })
            .collect();
        Ok(methods::encode(&methods::NetworkStatus {
            height: network.height,
            members,
        }))
    })
}

pub(super) fn invite(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let ttl = methods::decode::<methods::CreateInvite>(&ask)
            .map_err(malformed)?
            .ttl_days;
        if ttl == 0 {
            return Err(malformed("ttl_days must be a positive integer"));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(host_fault)?
            .as_millis() as u64;
        let ask = crate::contract::MintInvite {
            client: node.client.clone(),
            ttl_days: ttl,
            now_ms,
        };
        let Some(minted) = crate::contract::os().mint_invite(ask) else {
            return Err(wire::Error::new(
                refusal::INVITE_UNSUPPORTED,
                "Only the machine running this node can mint its invites.",
            ));
        };
        minted.await
    })
}

fn block_of(block: noded::Finalized) -> methods::Block {
    methods::Block {
        height: block.height,
        id: block.id,
        parent: block.parent,
        time: block.time,
        epoch: block.epoch,
        proposer: block.proposer,
        txs: block.txs.into_iter().map(tx_of).collect(),
    }
}

fn tx_of(tx: noded::Tx) -> methods::Tx {
    methods::Tx {
        hash: tx.hash,
        signer: tx.signer,
        anchor: methods::Tip {
            height: tx.anchor.height,
            id: tx.anchor.id,
        },
        target: tx.target,
        size: tx.size,
        payload: tx.payload,
        receipt: tx.receipt.map(receipt_of),
    }
}

fn receipt_of(receipt: noded::Receipt) -> methods::Receipt {
    methods::Receipt {
        program: receipt.program,
        outcome: match receipt.outcome {
            abi::Outcome::Applied { output } => methods::Outcome::Applied { output },
            abi::Outcome::Rejected(refusal) => {
                methods::Outcome::Rejected(wire::Error::new(refusal.reason, refusal.sentence))
            }
        },
        events: receipt.events,
        nested: receipt.nested.into_iter().map(receipt_of).collect(),
    }
}

/// `chain.blocks`: a page of finalized blocks from the node's archive.
pub(super) fn blocks(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let methods::BlockPage {
            before,
            limit,
            most,
        } = methods::ChainBlocks::decode_request(&ask).map_err(malformed)?;
        let page = noded::Blocks {
            before,
            limit,
            most,
        };
        let page = node.client.blocks(&page).await.map_err(refused)?;
        Ok(methods::encode(&methods::BlockList {
            blocks: page.blocks.into_iter().map(block_of).collect(),
            listed_from: page.listed_from,
        }))
    })
}

/// `chain.block`: one finalized block by height or id, if the node has it.
pub(super) fn block(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let methods::BlockAsk { by, most } =
            methods::ChainBlock::decode_request(&ask).map_err(malformed)?;
        let by = match by {
            methods::BlockRef::Height(height) => noded::BlockRef::Height(height),
            methods::BlockRef::Id(id) => noded::BlockRef::Id(id),
        };
        let ask = noded::BlockAsk { by, most };
        let block = node.client.block(&ask).await.map_err(refused)?;
        Ok(methods::encode(&block.map(block_of)))
    })
}

pub(super) fn tx(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let ask: methods::TxAsk = methods::decode(&ask).map_err(malformed)?;
        let tx = match ask.tx {
            methods::TxRef::At { height, position } => noded::TxRef::At { height, position },
            methods::TxRef::Hash(hash) => noded::TxRef::Hash(hash),
        };
        let ask = noded::TxAsk { tx, most: ask.most };
        let located = node.client.tx(&ask).await.map_err(refused)?;
        Ok(methods::encode(&located.map(|located| methods::Located {
            height: located.height,
            position: located.position,
            time: located.time,
            tx: tx_of(located.tx),
        })))
    })
}

#[cfg(test)]
mod tests;
