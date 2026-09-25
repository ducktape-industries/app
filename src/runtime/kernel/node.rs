use super::*;
use crate::backend::noded;

pub(super) type Answered = std::pin::Pin<Box<dyn std::future::Future<Output = Answer> + Send>>;
type Call = fn(Node, Vec<u8>) -> Answered;

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
            return Err(wire::Refusal::new(
                "rpc_client",
                super::super::NODE_UNREACHABLE,
            ));
        }
        tokio::time::sleep(delay).await;
    }
}

pub(super) fn spawn(guest: &mut Guest, id: u64, payload: &[u8], call: Call) {
    spawn_call(guest, id, payload, call, true);
}

pub(super) fn spawn_once(guest: &mut Guest, id: u64, payload: &[u8], call: Call) {
    spawn_call(guest, id, payload, call, false);
}

fn spawn_call(guest: &mut Guest, id: u64, payload: &[u8], call: Call, retry: bool) {
    let ask = payload.to_vec();
    let Some(node) = connected(guest, id) else {
        return;
    };
    let replies = guest.replies.clone();
    let Some(counted) = replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return;
    };
    let task = handle().spawn(async move {
        let _counted = counted;
        let result = if retry {
            until_answered(NODE_RETRY_BUDGET, || call(node.clone(), ask.clone())).await
        } else {
            call(node, ask).await
        };
        replies.item(id, result, true);
    });
    guest
        .tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    guest.tasks.push((id, NodeTask { task }));
}

/// One SUBSCRIPTION's writing end, handed to a host-fed stream the way the
/// node's own socket loop writes: every item goes through the same backlog
/// park, so a device that outruns the redraw holds its own frames instead of
/// growing the reply queue into a fault.
pub(in crate::runtime) struct Items {
    replies: std::sync::Arc<Replies>,
    drained: tokio::sync::watch::Receiver<()>,
    id: u64,
}

impl Items {
    /// One more item. `false` when the view is gone and the subscription
    /// should end — the caller returns, and everything it holds is dropped.
    pub(in crate::runtime) async fn send(&mut self, result: Answer) -> bool {
        self.replies
            .subscription_item(&mut self.drained, self.id, result)
            .await
    }

    /// The LAST item, which ends the subscription for the guest: a device
    /// that will not open, or one that stopped answering. Without it a dead
    /// source would look to the view like a source that is merely quiet.
    pub(in crate::runtime) fn end(self, result: Answer) {
        self.replies.item(self.id, result, true);
    }
}

/// A subscription the HOST feeds — a device rather than a node socket. The
/// body owns whatever the stream holds open, so dropping the task releases
/// it: a cancel drops the [`NodeTask`], and so do a guest's teardown, swap
/// and trap, which drop the whole `tasks` list.
pub(in crate::runtime) fn spawn_subscription<Body, Fut>(guest: &mut Guest, id: u64, body: Body)
where
    Body: FnOnce(Items) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let replies = guest.replies.clone();
    let Some(counted) = replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return;
    };
    let items = Items {
        drained: replies.drains(),
        replies,
        id,
    };
    let task = handle().spawn(async move {
        let _counted = counted;
        body(items).await;
    });
    guest
        .tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    guest.tasks.push((id, NodeTask { task }));
}

pub(in crate::runtime) fn spawn_device(
    guest: &mut Guest,
    id: u64,
    future: impl std::future::Future<Output = Answer> + Send + 'static,
) {
    let replies = guest.replies.clone();
    let Some(counted) = replies.admit() else {
        guest.refuse(id, "in_flight_limit", "too many in-flight view requests");
        return;
    };
    let task = handle().spawn(async move {
        let _counted = counted;
        replies.item(id, future.await, true);
    });
    guest
        .tasks
        .retain(|(_, pending)| !pending.task.is_finished());
    guest.tasks.push((id, NodeTask { task }));
}

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

/// `rpc.live <program>`: one item per block that wrote to the program.
pub(super) fn live(guest: &mut Guest, id: u64, payload: &[u8]) {
    let program = doors::decode::<String>(payload)
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
    let task = handle().spawn(async move {
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
                    Ok(change) => Ok(doors::encode(&Some(change.height))),
                    Err(_) => break,
                };
                if !replies.subscription_item(&mut drained, id, item).await {
                    return;
                }
            }
            // the socket closed: the node restarted or the link dropped. Say
            // so once (the view re-reads) and open it again.
            if !replies
                .subscription_item(&mut drained, id, Ok(doors::encode(&None::<u64>)))
                .await
            {
                return;
            }
            tokio::time::sleep(backend::retry_delay(1)).await;
        }
    });
    guest.tasks.push((id, NodeTask { task }));
}

pub(in crate::runtime) struct NodeTask {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for NodeTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The envelope of a node door, its target checked: a program name, not a
/// path.
fn call_of(ask: &[u8]) -> Result<doors::Call, wire::Refusal> {
    let call: doors::Call = doors::decode(ask).map_err(malformed)?;
    let target = call.target.trim();
    let named = !target.is_empty()
        && target.len() <= 64
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    match named {
        true => Ok(doors::Call {
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
    let frame = backend::seated_frame(&node.client, &node.network, &target, payload).await?;
    let receipt = node.client.submit(frame).await.map_err(refused)?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => {
            Err(wire::Refusal::new(refusal.reason, refusal.sentence))
        }
    }
}

pub(super) fn blob_get(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let id: String = doors::decode(&ask).map_err(malformed)?;
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
        // `BlobGet`'s door promises `Vec<u8>` through the standard `door!`
        // macro, which borsh-wraps (length-prefixes) it on both ends — the
        // guest's generic `Door::decode_reply` expects that prefix on every
        // reply, this door included. Raw bytes here would desync it from
        // the very first blob any view fetched.
        // `BlobGet`'s door promises `Vec<u8>` through the standard `door!`
        // macro, which borsh-wraps (length-prefixes) it on both ends — the
        // guest's generic `Door::decode_reply` expects that prefix on every
        // reply, this door included. Raw bytes here would desync it from
        // the very first blob any view fetched.
        Ok(doors::encode(&body.to_vec()))
    })
}

pub(super) fn status(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        if !ask.is_empty() {
            return Err(malformed("rpc.status takes no payload"));
        }
        let status = node.client.status().await.map_err(refused)?;
        Ok(doors::encode(&doors::NodeStatus {
            network: status.network,
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
        let ttl = doors::decode::<doors::Mint>(&ask)
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
                return wire::Refusal::new("invite_unsupported", "This node doesn't mint invites.");
            }
            wire::Refusal::new(error.reason(), error.message())
        };
        let client = ducktape_rpc::Client::new(node.client.endpoint()).map_err(refusal)?;
        let minted = client.mint_invite(ttl).await.map_err(refusal)?;
        let notes = minted
            .notes
            .into_iter()
            .map(|note| doors::Note {
                reason: note.reason,
                sentence: note.sentence,
            })
            .collect();
        Ok(doors::encode(&doors::Minted {
            invite: minted.invite,
            notes,
        }))
    })
}

fn block_of(block: noded::Finalized) -> doors::Block {
    doors::Block {
        height: block.height,
        id: block.id,
        parent: block.parent,
        time: block.time,
        epoch: block.epoch,
        proposer: block.proposer,
        txs: block
            .txs
            .into_iter()
            .map(|tx| doors::Tx {
                hash: tx.hash,
                signer: tx.signer,
                seq: tx.seq,
                target: tx.target,
                payload: tx.payload,
            })
            .collect(),
    }
}

/// `rpc.blocks`: a page of finalized blocks from the node's archive.
pub(super) fn blocks(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let page: doors::BlockPage = doors::decode(&ask).map_err(malformed)?;
        let page = noded::Blocks {
            before: page.before,
            limit: page.limit,
        };
        let blocks = node.client.blocks(&page).await.map_err(refused)?;
        let blocks: Vec<doors::Block> = blocks.into_iter().map(block_of).collect();
        Ok(doors::encode(&blocks))
    })
}

/// `rpc.block`: one finalized block by height or id, if the node has it.
pub(super) fn block(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let by = match doors::decode::<doors::BlockRef>(&ask).map_err(malformed)? {
            doors::BlockRef::Height(height) => noded::BlockRef::Height(height),
            doors::BlockRef::Id(id) => noded::BlockRef::Id(id),
        };
        let block = node.client.block(&by).await.map_err(refused)?;
        Ok(doors::encode(&block.map(block_of)))
    })
}
