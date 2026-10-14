//! The node-backed methods and how a view request becomes a task: a
//! handler is `fn(Node, Vec<u8>) -> Answered`, run on [`handle`] with an
//! in-flight slot from [`Replies::admit`], its answer put in [`Replies`];
//! transport failures are retried for [`NODE_RETRY_BUDGET`]. A subscription
//! (`program.changes`, `chain.heads`) is one long task writing items. Every
//! task is a [`NodeTask`] in `view.tasks`, aborted when dropped.
use super::super::consent;
use super::*;
use noded_client::wire as noded;

pub(super) type Answered = std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>;
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

/// How long a view's node request keeps asking a node that does not answer.
const NODE_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

/// A refusal the transport produced is retried; one that is the node's
/// own word ends the retry loop. For a READ: asking again costs nothing.
pub(super) fn transport_failed(refusal: &wire::Error) -> bool {
    matches!(
        refusal.code.as_str(),
        refusal::RPC_CLIENT | refusal::NODE_FAILED
    )
}

fn unsent(refusal: &wire::Error) -> bool {
    refusal.code == refusal::RPC_CLIENT
}

/// The answer, and how many times the node was asked for it: each ask
/// given `per_attempt` to answer ([`noded_client::ANSWER_DEADLINE`], never the
/// budget's remainder, which would cut a recovering node's held answer
/// short), asked again while `retry_on` the refusal and `budget` lasts. An
/// ask cut at its deadline may have reached the node: `node_failed`, which
/// a submit does not sign again.
async fn until_answered(
    budget: std::time::Duration,
    per_attempt: std::time::Duration,
    retry_on: fn(&wire::Error) -> bool,
    mut ask: impl FnMut() -> Answered,
) -> (Answer, u64) {
    let deadline = tokio::time::Instant::now() + budget;
    let mut attempt: u32 = 0;
    loop {
        let refusal = match tokio::time::timeout(per_attempt, ask()).await {
            Ok(Ok(bytes)) => return (Ok(bytes), u64::from(attempt) + 1),
            Ok(Err(refusal)) => refusal,
            Err(_) => wire::Error::new(
                refusal::NODE_FAILED,
                "no answer came back: the node did not answer in time",
            ),
        };
        if !retry_on(&refusal) {
            return (Err(refusal), u64::from(attempt) + 1);
        }
        let detail = refusal.message;
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
            return (
                Err(wire::Error::new(
                    refusal::RPC_CLIENT,
                    super::super::NODE_UNREACHABLE,
                )),
                u64::from(attempt),
            );
        }
        tokio::time::sleep(delay).await;
    }
}

/// Runs `method` on the connected node as a task, RETRYING transport
/// failures for [`NODE_RETRY_BUDGET`]: for reads, which ask again from
/// scratch at no cost.
pub(super) fn spawn_retrying(
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
        method,
        Some(transport_failed),
        stage,
        None,
    );
}

pub(super) fn spawn_retrying_unsent(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
    stage: &'static str,
) {
    spawn_method(view, id, payload, method, Some(unsent), stage, None);
}

/// `op.submit` of an op the person confirms first (`consent::needed`):
/// only the person's activation opens the dialog, taken here at the press
/// as `link.open` takes it (a view resubmitting after Cancel, or asking on
/// a clock, gets `needs_gesture` and no second card); refused
/// `session_locked` at once with no key to sign it (no dialog that cannot
/// lead to a signature); else its task asks the person ([`Asking`]) and
/// runs it as [`spawn_retrying_unsent`] once they approve, refused
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
    // the in-flight slot before the ask: a request refused
    // `in_flight_limit` never puts a card up that leads nowhere
    let Some(slot) = admit(view, id) else {
        return;
    };
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
        Some(unsent),
        "host_call.op.submit",
        Some((slot, asking)),
    );
}

/// How long a consented op's card waits on the target's description before
/// it goes up with the op's size instead: the press it answers is the
/// person's of a moment ago, not of minutes ago. The code blob is normally
/// on disk already (the role's own view loaded from it).
const DESCRIBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// program reads nothing or does not answer in time), the ask queued and
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
            let described = tokio::time::timeout(
                DESCRIBE_DEADLINE,
                super::describe::described(node, &call.target, call.body),
            )
            .await;
            let described = described.ok().and_then(Result::ok).flatten();
            words.shown = Some(consent::shown(described.as_ref(), len));
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

/// Runs `method` on the connected node as a task, asking exactly once: a
/// transport failure is the answer (`invite.create` must not mint twice).
pub(super) fn spawn_no_retry(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
    stage: &'static str,
) {
    spawn_method(view, id, payload, method, None, stage, None);
}

/// `stage` names the call in the perf registry (`host_call.<kind>`): its
/// latency from here to the reply, and the attempts it took. `consent`,
/// when given, is the person's yes the task asks for before it asks the
/// node at all ([`Asking::yes`]).
#[allow(clippy::too_many_arguments, reason = "one spawn, every node method")]
fn spawn_method(
    view: &mut View,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
    retry_on: Option<fn(&wire::Error) -> bool>,
    stage: &'static str,
    consent: Option<(InFlight, Asking)>,
) {
    let ask = payload.to_vec();
    let Some(node) = connected(view, id) else {
        return;
    };
    let (slot, asking) = match consent {
        Some((slot, asking)) => (slot, Some(asking)),
        None => match admit(view, id) {
            Some(slot) => (slot, None),
            None => return,
        },
    };
    let replies = view.replies.clone();
    let key = view.perf_key();
    // not a `perf::Timer`: its drop would record a call refused
    // `in_flight_limit` or cancelled by the view, which never got a reply
    let asked = crate::perf::on().then(std::time::Instant::now);
    let attempts_stage = crate::perf::suffixed(stage, ".attempts");
    run(view, id, slot, None, async move {
        if let Some(asking) = asking
            && let Err(refused) = asking.yes(&node).await
        {
            replies.item(id, Err(refused), true);
            return;
        }
        let (result, attempts) = match retry_on {
            Some(retry_on) => {
                until_answered(
                    NODE_RETRY_BUDGET,
                    noded_client::ANSWER_DEADLINE,
                    retry_on,
                    || method(node.clone(), ask.clone()),
                )
                .await
            }
            None => (method(node, ask).await, 1),
        };
        if let Some(asked) = asked {
            crate::perf::record(key, stage, asked.elapsed().as_micros() as u64);
        }
        if let Some(attempts_stage) = attempts_stage {
            crate::perf::count(key, attempts_stage, attempts);
        }
        replies.item(id, result, true);
    });
}

/// Every task starts here: an in-flight slot ([`admit`]) — or the request
/// refused `in_flight_limit`, and `false` — then [`run`].
fn start(
    view: &mut View,
    id: u64,
    follows: Option<Follow>,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) -> bool {
    match admit(view, id) {
        Some(slot) => {
            run(view, id, slot, follows, task);
            true
        }
        None => false,
    }
}

/// An in-flight slot from [`Replies::admit`] for request `id`, or the
/// request refused `in_flight_limit` and `None`.
fn admit(view: &mut View, id: u64) -> Option<InFlight> {
    let slot = view.replies.admit();
    if slot.is_none() {
        view.refuse(
            id,
            refusal::IN_FLIGHT_LIMIT,
            "too many in-flight view requests",
        );
    }
    slot
}

/// `task` on [`handle`] holding its `slot`, its [`NodeTask`] kept in
/// `view.tasks` (finished ones pruned) so it is aborted with the view.
fn run(
    view: &mut View,
    id: u64,
    slot: InFlight,
    follows: Option<Follow>,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let task = handle().spawn(async move {
        let _slot = slot;
        task.await;
    });
    view.tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    view.tasks.push((id, NodeTask { task, follows }));
}

/// One SUBSCRIPTION's writing end, handed to the loop that feeds it: every
/// item goes through the same backlog park as a node socket's, so a source
/// that outruns the redraw waits instead of growing the reply queue into a
/// fault.
struct Items {
    replies: std::sync::Arc<Replies>,
    drained: tokio::sync::watch::Receiver<()>,
    id: u64,
}

impl Items {
    /// One more item. `false` when the view is gone or its reply queue
    /// faulted, and the subscription should end — the caller returns, and
    /// everything it holds is dropped.
    async fn send(&mut self, result: Answer) -> bool {
        self.replies
            .subscription_item(&mut self.drained, self.id, result)
            .await
    }
}

/// A subscription whose items come from a host loop (`chain.heads` polls
/// the node for them). The body owns whatever it holds open, so dropping
/// the task releases it: a cancel drops the [`NodeTask`], and so do a
/// view's teardown, swap and trap, which drop the whole `tasks` list.
fn spawn_subscription<Body, Fut>(view: &mut View, id: u64, follows: Follow, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let replies = view.replies.clone();
    let items = Items {
        drained: replies.drains(),
        replies,
        id,
    };
    start(view, id, Some(follows), body(items));
}

/// Runs `future` on the kernel runtime and delivers its result as the one
/// reply to request `id`; counted in flight, aborted with the view. For a
/// host method that waits on something other than the node (`notify.post`
/// waits on its banner, `store.*` on its file). `false` when the request
/// was refused `in_flight_limit` instead.
pub(in crate::runtime) fn spawn_reply(
    view: &mut View,
    id: u64,
    future: impl std::future::Future<Output = Answer> + Send + 'static,
) -> bool {
    let replies = view.replies.clone();
    start(view, id, None, async move {
        replies.item(id, future.await, true);
    })
}

/// The node to ask, the one the view's own connection names, or `None`
/// with the request REFUSED: `stale_connection` for a view from before the
/// app's last (re)connect, `not_connected` without a node.
fn connected(view: &mut View, id: u64) -> Option<Node> {
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
/// `chain.heads` one starts over at the new node's tip. Every other request
/// still running on the old connection is refused `stale_connection`, for
/// the view to ask again.
pub(in crate::runtime) fn reconnected(view: &mut View, connection: &super::super::Connection) {
    // counted again as each opens
    view.live_subscriptions.clear();
    for (id, mut task) in std::mem::take(&mut view.tasks) {
        let follows = task.follows.take();
        // dropped, it aborts: the old connection's socket goes with it
        drop(task);
        match (follows, node_on(connection)) {
            (Some(Follow::Changes(program)), Some(node)) => {
                view.replies
                    .item(id, Ok(methods::encode(&None::<methods::Change>)), false);
                follow_changes(view, id, node, program);
            }
            (Some(Follow::Heads), Some(node)) => follow_heads(view, id, node),
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
    if view.live_subscriptions.len() >= MAX_SUBSCRIPTIONS {
        view.refuse(
            id,
            refusal::SUBSCRIPTION_LIMIT,
            "too many `program.changes` subscriptions",
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
    let started = start(view, id, Some(follows), async move {
        use futures::StreamExt as _;
        let mut drained = replies.drains();
        let mut warned = false;
        loop {
            let mut changes = match node.client.changes(&program).await {
                Ok(changes) => changes,
                Err(error) => {
                    if warned {
                        tracing::debug!(target: "ducktape::app", %error, program, "changes stream not opened");
                    } else {
                        warned = true;
                        tracing::warn!(target: "ducktape::app", %error, program, "changes stream not opened");
                    }
                    tokio::time::sleep(backend::retry_delay(2)).await;
                    continue;
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
                if !replies.subscription_item(&mut drained, id, item).await {
                    return;
                }
            }
            // the socket closed: the node restarted or the link dropped. Say
            // so once (the view re-reads) and open it again.
            if !replies
                .subscription_item(
                    &mut drained,
                    id,
                    Ok(methods::encode(&None::<methods::Change>)),
                )
                .await
            {
                return;
            }
            tokio::time::sleep(backend::retry_delay(1)).await;
        }
    });
    if started {
        view.live_subscriptions.push(subscribed);
    }
}

/// The most blocks one `/v1/blocks` page answers (the node's cap).
const MAX_BLOCK_PAGE: u32 = 100;

/// `chain.heads`: one [`methods::Head`] per finalized block, oldest first. The
/// node pushes no block stream, so this reads its status at half the block
/// time and fills each advance from the archive (a page, at most the node's
/// cap); where the archive holds none, the tip alone.
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
        loop {
            let pace = match next_heads(&node, last).await {
                Ok((heads, block_time_ms)) => {
                    for head in heads {
                        last = Some(head.height);
                        if !items.send(Ok(methods::encode(&head))).await {
                            return;
                        }
                    }
                    std::time::Duration::from_millis(block_time_ms / 2).clamp(
                        std::time::Duration::from_millis(100),
                        std::time::Duration::from_secs(2),
                    )
                }
                Err(error) => {
                    tracing::debug!(target: "ducktape::app", %error, "heads not read");
                    backend::retry_delay(2)
                }
            };
            tokio::time::sleep(pace).await;
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
    let wanted = last
        .map_or(1, |last| tip - last)
        .min(u64::from(MAX_BLOCK_PAGE));
    let page = noded::Blocks {
        before: tip.checked_add(1),
        limit: wanted as u32,
    };
    let mut heads: Vec<methods::Head> = node
        .client
        .blocks(&page)
        .await?
        .blocks
        .into_iter()
        .rev()
        .filter(|block| last.is_none_or(|last| block.height > last))
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
}

/// A node subscription, as a reconnect opens it again.
enum Follow {
    /// `program.changes` of this program.
    Changes(String),
    /// `chain.heads`.
    Heads,
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
        let frame = backend::query_frame(&node.network, &call.target, call.body).await;
        node.client
            .query(backend::Layer::Preconfirmed, frame)
            .await
            .map_err(refused)
    })
}

pub(super) fn submit(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let call = call_of(&ask)?;
        submitted(node, call.target, call.body).await
    })
}

/// The receipt's output on success, the program's refusal otherwise.
async fn submitted(node: Node, target: String, payload: Vec<u8>) -> Answer {
    let frame = backend::seated_frame(&node.client, &node.network, &target, payload)
        .await
        .map_err(|refusal| match transport_failed(&refusal) {
            true => wire::Error::new(refusal::RPC_CLIENT, refusal.message),
            false => refusal,
        })?;
    let receipt = node.client.submit(frame).await.map_err(refused)?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => Err(wire::Error::new(refusal.reason, refusal.sentence)),
    }
}

pub(super) fn blob_get(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let id: String = methods::decode(&ask).map_err(malformed)?;
        let (kind, hex) = id
            .split_once(':')
            .ok_or_else(|| malformed("id is `sha256:<hex>` or `sha1:<hex>`"))?;
        let digest = noded_client::hex::hex_decode(hex).map_err(malformed)?;
        let id = match (kind, digest.len()) {
            ("sha256", 32) => abi::BlobId::Sha256(digest.try_into().expect("32 bytes")),
            ("sha1", 20) => abi::BlobId::Sha1(digest.try_into().expect("20 bytes")),
            _ => return Err(malformed("id is `sha256:<hex>` or `sha1:<hex>`")),
        };
        // absent is `None`, never a refusal: the ask itself did not fail.
        // Read no further than the cap and the blob's header (`kind len\0`,
        // under 32 bytes) in its borsh `Option<Vec<u8>>` (5)
        let cap = MAX_BLOB_BYTES + 64;
        let Some(framed) = node.client.blob(id, cap).await.map_err(refused)? else {
            return Ok(methods::encode(&None::<Vec<u8>>));
        };
        let body =
            noded_client::wire::unframe(&framed).ok_or_else(|| host_fault("blob has no header"))?;
        if body.len() > MAX_BLOB_BYTES {
            return Err(wire::Error::new(
                refusal::TOO_LARGE,
                "blob exceeds the view's read limit",
            ));
        }
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

/// `chain.network`: each member's newest finalize vote. A node from before
/// `/v1/network` answers a bare 404: refused at once as a request it does
/// not know, not retried as a node that is down would be, so the view says
/// to update it.
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
        let workspace = ducktape_home::root().map_err(host_fault)?;
        let key = backend::invite::node_key(&workspace).map_err(host_fault)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(host_fault)?
            .as_millis() as u64;
        let invite = backend::invite::mint(&node.client, key, ttl, now).await?;
        Ok(methods::encode(&methods::Invite {
            invite,
            notes: Vec::new(),
        }))
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
        let page: methods::BlockPage = methods::decode(&ask).map_err(malformed)?;
        let page = noded::Blocks {
            before: page.before,
            limit: page.limit,
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
        let by = match methods::decode::<methods::BlockRef>(&ask).map_err(malformed)? {
            methods::BlockRef::Height(height) => noded::BlockRef::Height(height),
            methods::BlockRef::Id(id) => noded::BlockRef::Id(id),
        };
        let block = node.client.block(&by).await.map_err(refused)?;
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
