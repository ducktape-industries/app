//! The node-backed methods and how a view request becomes a task: a
//! handler is `fn(Node, Vec<u8>) -> Answered`, run on [`handle`] with an
//! in-flight slot from [`Replies::admit`], its answer put in [`Replies`];
//! transport failures are retried for [`NODE_RETRY_BUDGET`]. A subscription
//! (`module.changes`, `chain.heads`) is one long task writing items. Every
//! task is a [`NodeTask`] in `guest.tasks`, aborted when dropped.
use super::*;
use crate::backend::noded;

pub(super) type Answered = std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>;
/// One node-backed method: the node to ask and the request's payload in,
/// the answer out. Not a [`methods::Call`], which is `module.query`'s and
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
fn transport_failed(refusal: &wire::Error) -> bool {
    matches!(refusal.code.as_str(), "rpc_client" | "node_failed")
}

/// Only a refusal that proves no frame went out is retried (a connect
/// failure, `backend::refused`; or any failure of the sequence read that
/// comes before the frame, `submitted`). For a WRITE: a request the node
/// may have applied is not signed and sent again — the sequence moved, so
/// the node would apply it twice.
fn unsent(refusal: &wire::Error) -> bool {
    refusal.code == "rpc_client"
}

async fn until_answered(
    budget: std::time::Duration,
    retry_on: fn(&wire::Error) -> bool,
    mut ask: impl FnMut() -> Answered,
) -> Answer {
    let deadline = tokio::time::Instant::now() + budget;
    let mut attempt = 0;
    loop {
        let refusal = match ask().await {
            Ok(bytes) => return Ok(bytes),
            Err(refusal) => refusal,
        };
        if !retry_on(&refusal) {
            return Err(refusal);
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
            return Err(wire::Error::new(
                "rpc_client",
                super::super::NODE_UNREACHABLE,
            ));
        }
        tokio::time::sleep(delay).await;
    }
}

/// Runs `method` on the connected node as a task, RETRYING transport
/// failures for [`NODE_RETRY_BUDGET`]: for reads, which ask again from
/// scratch at no cost.
pub(super) fn spawn_retrying(guest: &mut Guest, id: u64, payload: &[u8], method: NodeMethod) {
    spawn_method(guest, id, payload, method, Some(transport_failed));
}

/// Runs `method` on the connected node as a task, retrying for
/// [`NODE_RETRY_BUDGET`] only while nothing reached the node: for
/// `op.submit`, which re-signs at a fresh sequence and so must not follow
/// an answer that went missing with a second submission.
pub(super) fn spawn_retrying_unsent(
    guest: &mut Guest,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
) {
    spawn_method(guest, id, payload, method, Some(unsent));
}

/// Runs `method` on the connected node as a task, asking exactly once: a
/// transport failure is the answer (`invite.create` must not mint twice).
pub(super) fn spawn_no_retry(guest: &mut Guest, id: u64, payload: &[u8], method: NodeMethod) {
    spawn_method(guest, id, payload, method, None);
}

fn spawn_method(
    guest: &mut Guest,
    id: u64,
    payload: &[u8],
    method: NodeMethod,
    retry_on: Option<fn(&wire::Error) -> bool>,
) {
    let ask = payload.to_vec();
    let Some(node) = connected(guest, id) else {
        return;
    };
    let replies = guest.replies.clone();
    start(guest, id, async move {
        let result = match retry_on {
            Some(retry_on) => {
                until_answered(NODE_RETRY_BUDGET, retry_on, || {
                    method(node.clone(), ask.clone())
                })
                .await
            }
            None => method(node, ask).await,
        };
        replies.item(id, result, true);
    });
}

/// Every task starts here: an in-flight slot from [`Replies::admit`] — or
/// the request refused `in_flight_limit`, and `false` — then `task` on
/// [`handle`] holding the slot, its [`NodeTask`] kept in `guest.tasks`
/// (finished ones pruned) so it is aborted with the guest.
fn start(
    guest: &mut Guest,
    id: u64,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) -> bool {
    let Some(counted) = guest.replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return false;
    };
    let task = handle().spawn(async move {
        let _counted = counted;
        task.await;
    });
    guest
        .tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    guest.tasks.push((id, NodeTask { task }));
    true
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
/// guest's teardown, swap and trap, which drop the whole `tasks` list.
fn spawn_subscription<Body, Fut>(guest: &mut Guest, id: u64, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let replies = guest.replies.clone();
    let items = Items {
        drained: replies.drains(),
        replies,
        id,
    };
    start(guest, id, body(items));
}

/// Runs `future` on the kernel runtime and delivers its result as the one
/// reply to request `id`; counted in flight, aborted with the guest. For a
/// host method that waits on something other than the node (`notify.post`
/// waits on its banner).
pub(in crate::runtime) fn spawn_reply(
    guest: &mut Guest,
    id: u64,
    future: impl std::future::Future<Output = Answer> + Send + 'static,
) {
    let replies = guest.replies.clone();
    start(guest, id, async move {
        replies.item(id, future.await, true);
    });
}

/// The node to ask, or `None` with the request REFUSED: `stale_connection`
/// for a view from before the last (re)connect, `not_connected` without a
/// node.
fn connected(guest: &mut Guest, id: u64) -> Option<Node> {
    let connection = super::super::connection().lock().expect("views rpc");
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

/// `module.changes <program>`: one item per block that wrote to the program.
pub(super) fn changes(guest: &mut Guest, id: u64, payload: &[u8]) {
    let program = methods::decode::<String>(payload)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if program.is_empty() {
        guest.refuse(id, "malformed_request", "`module.changes` names no program");
        return;
    }
    if guest.live_subscriptions.len() >= MAX_SUBSCRIPTIONS {
        guest.refuse(
            id,
            "subscription_limit",
            "too many `module.changes` subscriptions",
        );
        return;
    }
    let Some(node) = connected(guest, id) else {
        return;
    };
    let replies = guest.replies.clone();
    let subscribed = (id, program.clone());
    let started = start(guest, id, async move {
        use futures::StreamExt as _;
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
                    Ok(change) => Ok(methods::encode(&Some(change.height))),
                    Err(_) => break,
                };
                if !replies.subscription_item(&mut drained, id, item).await {
                    return;
                }
            }
            // the socket closed: the node restarted or the link dropped. Say
            // so once (the view re-reads) and open it again.
            if !replies
                .subscription_item(&mut drained, id, Ok(methods::encode(&None::<u64>)))
                .await
            {
                return;
            }
            tokio::time::sleep(backend::retry_delay(1)).await;
        }
    });
    if started {
        guest.live_subscriptions.push(subscribed);
    }
}

/// The most blocks one `/v1/blocks` page answers (the node's cap).
const MAX_BLOCK_PAGE: u32 = 100;

/// `chain.heads`: one [`methods::Head`] per finalized block, oldest first. The
/// node pushes no block stream, so this reads its status at half the block
/// time and fills each advance from the archive (a page, at most the node's
/// cap); where the archive holds none, the tip alone.
pub(super) fn heads(guest: &mut Guest, id: u64, payload: &[u8]) {
    if !payload.is_empty() {
        guest.refuse(id, "malformed_request", "`chain.heads` takes no payload");
        return;
    }
    let Some(node) = connected(guest, id) else {
        return;
    };
    spawn_subscription(guest, id, move |mut items| async move {
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
async fn next_heads(node: &Node, last: Option<u64>) -> noded::Result<(Vec<methods::Head>, u64)> {
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
}

impl Drop for NodeTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The envelope of a node method, its target checked: a program name, not a
/// path.
fn call_of(ask: &[u8]) -> Result<methods::Call, wire::Error> {
    let call: methods::Call = methods::decode(ask).map_err(malformed)?;
    let target = call.target.trim();
    let named = !target.is_empty()
        && target.len() <= 64
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    match named {
        true => Ok(methods::Call {
            target: target.to_owned(),
            body: call.body,
        }),
        false => Err(malformed("request names no target")),
    }
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
    // the sequence read comes before any frame: nothing signed, nothing
    // sent, so its lost answer is asked again as a read's is
    let frame = backend::seated_frame(&node.client, &node.network, &target, payload)
        .await
        .map_err(|refusal| match transport_failed(&refusal) {
            true => wire::Error::new("rpc_client", refusal.message),
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
        let digest = backend::hex_decode(hex).map_err(malformed)?;
        let id = match (kind, digest.len()) {
            ("sha256", 32) => abi::BlobId::Sha256(digest.try_into().expect("32 bytes")),
            ("sha1", 20) => abi::BlobId::Sha1(digest.try_into().expect("20 bytes")),
            _ => return Err(malformed("id is `sha256:<hex>` or `sha1:<hex>`")),
        };
        // absent is `None`, never a refusal: the ask itself did not fail
        let Some(framed) = node.client.blob(id).await.map_err(refused)? else {
            return Ok(methods::encode(&None::<Vec<u8>>));
        };
        let body =
            backend::noded::unframe(&framed).ok_or_else(|| host_fault("blob has no header"))?;
        if body.len() > MAX_BLOB_BYTES {
            return Err(wire::Error::new(
                "too_large",
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

pub(super) fn invite(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let ttl = methods::decode::<methods::CreateInvite>(&ask)
            .map_err(malformed)?
            .ttl_days;
        if ttl == 0 {
            return Err(malformed("ttl_days must be a positive integer"));
        }
        // a node without /v1/invite answers an unenveloped 404, which the
        // client classifies `http_error` — its "404 Not Found: no detail." is
        // not something to show a person as-is.
        let refusal = |error: ducktape_rpc::Error| {
            if error.reason() == "http_error" && error.status() == Some(404) {
                return wire::Error::new("invite_unsupported", "This node doesn't mint invites.");
            }
            wire::Error::new(error.reason(), error.message())
        };
        // `/v1/invite` is JSON and not in `backend::noded`; ducktape-rpc is
        // used for it alone
        let client = ducktape_rpc::Client::new(node.client.endpoint()).map_err(refusal)?;
        let minted = client.mint_invite(ttl).await.map_err(refusal)?;
        let notes = minted
            .notes
            .into_iter()
            .map(|note| wire::Error {
                code: note.reason,
                message: note.sentence,
            })
            .collect();
        Ok(methods::encode(&methods::Invite {
            invite: minted.invite,
            notes,
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
        txs: block
            .txs
            .into_iter()
            .map(|tx| methods::Tx {
                hash: tx.hash,
                signer: tx.signer,
                seq: tx.seq,
                target: tx.target,
                payload: tx.payload,
                receipt: tx.receipt.map(receipt_of),
            })
            .collect(),
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
        let blocks = node.client.blocks(&page).await.map_err(refused)?;
        let blocks: Vec<methods::Block> = blocks.into_iter().map(block_of).collect();
        Ok(methods::encode(&blocks))
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

#[cfg(test)]
mod tests;
