//! The kernel contract: what EVERY view may ask of the app, with no
//! per-program code on this side of the wire. A view that speaks only this
//! contract is replaced by a program deployment alone — the app binary
//! never changes for it.
//!
//! Every method, its request and its reply are the types in `wire::methods`,
//! borsh on both sides; the host decodes a request by that type and nothing
//! else, so there is no per-method parsing here to drift from a view.
//! `methods::ALL` is the one list, and `tests::the_routed_kinds_are_exactly_the_methods`
//! holds the arms in this directory to it. Where each kind is answered:
//!
//! Here, in [`answer`], from app state, on the window thread:
//! - `host.visible` — a subscription to whether the view's window shows it.
//! - `host.offset` — a subscription to the reader's UTC offset in minutes,
//!   from the OS's local time, re-sent when it moves (a DST change).
//! - `host.route` — a subscription that gets the route a `duck://` link
//!   asked of this view (`explorer/tx/<hash>` → `tx/<hash>`), once,
//!   DECODED: the link's `%XX` escapes read back (`chat/forge%3Aweb%3A3`
//!   → `forge:web:3`), checked by `valid_route`.
//! - `host.badge` — the tab's unread count; `host.id` — a minted id under a
//!   short prefix; `link.open` — the one way out: a `duck://` link, or an
//!   `https://` one for the system browser; `clock.ticks` — a periodic
//!   empty item, at most one per redraw ([`ticked`]).
//!
//! On the node, as a task on [`handle`] (`node`), each answer waiting in
//! [`Replies`] for the view's next redraw; transport failures retried for
//! up to a minute unless said otherwise:
//! - `module.query` `Call{target, body}` — one query on the connected node:
//!   `body` is the payload of a signed frame to `target`, and the bytes the
//!   program `Respond`ed come back as they are.
//! - `op.submit` `Call{target, body}` — one op, signed with the key
//!   unlocked in this session at the signer's next sequence and submitted;
//!   answered with the receipt's output, or the program's refusal. Retried
//!   only while nothing reached the node: a lost answer is reported
//!   ([`refusal::NODE_FAILED`]), never followed by a second submission.
//! - `chain.status` — node status as `NodeStatus`.
//! - `chain.blocks` `BlockPage` — finalized blocks, newest first, from the
//!   node's block archive (`/v1/blocks`); `chain.block` `BlockRef` — one by
//!   height or id (`/v1/block`).
//! - `blob.get` `<id>` — a blob by `sha256:<hex>` or `sha1:<hex>` id, unframed.
//! - `module.describe` `(program, op)` — the op as the program's own
//!   describe module reads it, or `None` (`describe`).
//! - `invite.create` `CreateInvite{ttl_days}` — minted ONCE, never retried;
//!   `Invite{invite, notes}`. Node refusals retain their tokens.
//! - `module.changes` `<program>` — a subscription that gets one item per
//!   block that wrote to `program` (`/v1/changes/<program>`), so the view
//!   re-reads what moved.
//! - `chain.heads` — a subscription that gets one `Head` per finalized block,
//!   oldest first: the host reads the node's status at half its block
//!   time and fills each advance from the block archive.
//!
//! In a sibling module [`answer`] asks first:
//! - `clipboard.read`, `clipboard.write` — `clipboard`, run on the window
//!   thread after the tick.
//! - `notify.post`, `notify.seen` — `notify`, the notification centre.
//! - `store.get`, `store.set` — `store`, the view's file on this device.
//!
//! Not the kernel's — `Guest::answer` (`guest/requests.rs`) takes them once
//! [`answer`] says `false`, since they read the guest's own state:
//! - `host.widget` — a widget command; `host.session` — a subscription to
//!   the session basics (`Session`: the account, theme, chain and read-only
//!   endpoint); `host.log` — a line into app.log.
//!
//! A refusal's code is one of [`refusal`]'s, or the node's or the program's
//! own, carried through.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{Guest, Intent, wire};
use crate::backend::{self, RpcClient, refused};
use wire::methods::{self, Capability, refusal};

/// The longest `host.id` prefix: a word naming the kind of record, not a
/// payload of its own.
const MAX_ID_PREFIX: usize = 32;
/// The most a `blob.get` may pull into a view.
const MAX_BLOB_BYTES: usize = 16 << 20;
/// Node tasks and subscriptions running at once for one view
/// ([`Replies::admit`]); past it a request is refused `in_flight_limit`.
const MAX_IN_FLIGHT: usize = 256;
/// Subscriptions of one kind a view may hold, counted per kind and only
/// for `clock.ticks` and `module.changes`; the `host.*` subscriptions are
/// each one list a view has no reason to grow.
const MAX_SUBSCRIPTIONS: usize = 256;
/// Answers waiting in [`Replies`] between two redraws, by count and by
/// bytes; exceeding either stops the view for good.
const MAX_REPLY_EVENTS: usize = 1024;
const MAX_REPLY_BYTES: usize = 32 << 20;
/// The share of the reply budget SUBSCRIPTIONS may fill before they stop
/// reading their sources: a request answers once, a subscription forever,
/// against a queue only a redraw empties.
const MAX_STREAM_BACKLOG_EVENTS: usize = MAX_REPLY_EVENTS / 2;
const MAX_STREAM_BACKLOG_BYTES: usize = MAX_REPLY_BYTES / 2;
/// `link.open`s one view may have admitted in the last `LINK_WINDOW`,
/// each on its own activation; past it a request is refused `link_limit`.
const MAX_LINKS_PER_WINDOW: usize = 4;
const LINK_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

fn host_fault(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::HOST_FAULT, error.to_string())
}

fn malformed(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::MALFORMED_REQUEST, error.to_string())
}

/// One answer to a guest: the bytes it asked for, or the refusal that names
/// why not. Every method in this file hands back exactly this.
pub(super) type Answer = Result<Vec<u8>, wire::Error>;

mod describe;
mod node;
mod replies;

pub(super) use node::{NodeTask, spawn_reply};
use node::{
    blob_get, block, blocks, changes, heads, invite, network, query, spawn_no_retry,
    spawn_retrying, spawn_retrying_unsent, status, submit,
};
pub(super) use replies::Replies;

/// The one current-thread tokio runtime every async I/O in the app runs
/// on — node HTTP and websockets here, the shell's task stream, the loader
/// threads' fetches (`runtime::handle` hands it out) — on its own thread, so
/// the window thread never blocks on the node. CPU or OS-blocking work goes
/// to `spawn_blocking`.
pub(super) fn handle() -> tokio::runtime::Handle {
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

/// Jobs under one name run one after another, in the order they were
/// queued, each on the blocking pool (an OS call or a file): a view's
/// banners (`notify`, under its module), so a burst's "N more" never lands
/// before the banners it counts, and a view's store file (`store`, under
/// its path), so a `get` reads the `set` before it. One task per name on
/// [`handle`] runs them.
pub(super) fn in_order(name: &str, job: impl FnOnce() + Send + 'static) {
    type Job = Box<dyn FnOnce() + Send>;
    static QUEUES: OnceLock<Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<Job>>>> =
        OnceLock::new();
    let mut queues = QUEUES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let queue = queues.entry(name.to_owned()).or_insert_with(|| {
        let (queue, mut jobs) = tokio::sync::mpsc::unbounded_channel::<Job>();
        handle().spawn(async move {
            while let Some(job) = jobs.recv().await {
                // a job that panicked is lost alone; the next goes on
                let _ = tokio::task::spawn_blocking(job).await;
            }
        });
        queue
    });
    let _ = queue.send(Box::new(job));
}

/// Routes one kernel request; `false` when the kind is not the kernel's.
pub(super) fn answer(
    guest: &mut Guest,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    if super::clipboard::answer(guest, capability, operation, id, payload)
        || super::notify::answer(guest, capability, operation, id, payload)
        || super::store::answer(guest, capability, operation, id, payload)
    {
        return true;
    }
    match (capability, operation) {
        (Capability::Host, "visible") => {
            if !payload.is_empty() {
                guest.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "visibility subscription takes no payload",
                );
                return true;
            }
            guest.visibility_subscriptions.push(id);
            guest.pending.push(wire::Event::Response {
                id,
                result: Ok(methods::encode(&guest.visible)),
                done: false,
            });
        }
        (Capability::Host, "offset") => {
            if !payload.is_empty() {
                guest.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "offset subscription takes no payload",
                );
                return true;
            }
            // the subscribers already here hear a move first: they share
            // the one last-sent offset
            let minutes = offset_minutes();
            guest.sync_offset(minutes);
            guest.offset_subscriptions.push(id);
            guest.pending.push(wire::Event::Response {
                id,
                result: Ok(methods::encode(&minutes)),
                done: false,
            });
        }
        (Capability::Host, "route") => {
            if !payload.is_empty() {
                guest.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "route subscription takes no payload",
                );
                return true;
            }
            guest.route_subscriptions.push(id);
            guest.sync_route();
        }
        (Capability::Module, "query") => {
            spawn_retrying(guest, id, payload, query, "host_call.module.query")
        }
        (Capability::Chain, "status") => {
            spawn_retrying(guest, id, payload, status, "host_call.chain.status")
        }
        (Capability::Chain, "network") => {
            spawn_retrying(guest, id, payload, network, "host_call.chain.network")
        }
        (Capability::Chain, "blocks") => {
            spawn_retrying(guest, id, payload, blocks, "host_call.chain.blocks")
        }
        (Capability::Chain, "block") => {
            spawn_retrying(guest, id, payload, block, "host_call.chain.block")
        }
        (Capability::Invite, "create") => {
            spawn_no_retry(guest, id, payload, invite, "host_call.invite.create")
        }
        (Capability::Op, "submit") => {
            spawn_retrying_unsent(guest, id, payload, submit, "host_call.op.submit")
        }
        (Capability::Blob, "get") => {
            spawn_retrying(guest, id, payload, blob_get, "host_call.blob.get")
        }
        (Capability::Module, "describe") => spawn_retrying(
            guest,
            id,
            payload,
            describe::describe,
            "host_call.module.describe",
        ),
        (Capability::Module, "changes") => changes(guest, id, payload),
        (Capability::Chain, "heads") => heads(guest, id, payload),
        // the one way out: a `duck://` link, or an `https://` one for the
        // system browser; any other scheme is refused here, at the method.
        // Only the person's activation opens one, taken by the ask, inside
        // the budget: a view on a clock never opens a tab or reseats the desk
        (Capability::Link, "open") => match methods::decode::<String>(payload) {
            Ok(link) if openable(&link) => {
                if !guest.take_activation() {
                    guest.refuse(
                        id,
                        refusal::NEEDS_GESTURE,
                        "`link.open` needs a press or key",
                    );
                    return true;
                }
                let now = std::time::Instant::now();
                guest.links.retain(|opened| now - *opened < LINK_WINDOW);
                if guest.links.len() >= MAX_LINKS_PER_WINDOW {
                    guest.refuse(id, refusal::LINK_LIMIT, "too many links opened this minute");
                    return true;
                }
                guest.links.push(now);
                guest.intents.push(Intent::OpenLink(link));
                guest.reply(id, Ok(Vec::new()));
            }
            Ok(_) => guest.refuse(
                id,
                refusal::MALFORMED_REQUEST,
                "`link.open` opens a duck:// or https:// link",
            ),
            Err(error) => guest.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        (Capability::Host, "badge") => match methods::decode::<i64>(payload).ok() {
            Some(count) => {
                guest.intents.push(Intent::Badge(count));
                guest.reply(id, Ok(Vec::new()));
            }
            None => guest.refuse(
                id,
                refusal::MALFORMED_REQUEST,
                "`host.badge` carries no count",
            ),
        },
        (Capability::Clock, "ticks") => {
            let period = tick_period(payload);
            if guest.clocks.len() >= MAX_SUBSCRIPTIONS {
                guest.refuse(
                    id,
                    refusal::SUBSCRIPTION_LIMIT,
                    "too many clock subscriptions",
                );
                return true;
            }
            match period {
                Some(period) => guest.clocks.push(Clock {
                    id,
                    period,
                    due: std::time::Instant::now() + period,
                }),
                None => guest.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "`clock.ticks` names no period",
                ),
            }
        }
        (Capability::Host, "id") => {
            let prefix = methods::decode::<String>(payload).unwrap_or_default();
            let prefix = prefix.trim();
            let named = !prefix.is_empty()
                && prefix.len() <= MAX_ID_PREFIX
                && prefix.bytes().all(|byte| byte.is_ascii_alphanumeric());
            match named {
                true => guest.reply(id, Ok(methods::encode(&backend::fresh_id(prefix)))),
                false => guest.refuse(id, refusal::MALFORMED_REQUEST, "`host.id` names no prefix"),
            }
        }
        _ => return false,
    }
    true
}

/// Whether `link.open` may open `link`: `duck://` or `https://` with
/// something after it.
fn openable(link: &str) -> bool {
    ["duck://", "https://"].iter().any(|scheme| {
        link.strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    })
}

/// The UTC offset in seconds the OS keeps for the local time at unix
/// second `wall`.
pub(crate) fn local_offset(wall: i64) -> i64 {
    let time = wall as libc::time_t;
    // SAFETY: `localtime_r` writes only the `tm` it is handed.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        match libc::localtime_r(&time, &mut tm).is_null() {
            true => 0,
            false => tm.tm_gmtoff as i64,
        }
    }
}

/// The reader's UTC offset in minutes now, as `host.offset` hands it.
pub(super) fn offset_minutes() -> i32 {
    (local_offset(super::notify::wall()) / 60) as i32
}

pub(super) struct Clock {
    pub(super) id: u64,
    period: std::time::Duration,
    due: std::time::Instant,
}

/// The shortest and longest period a view may ask the clock for. Below the
/// floor a tick is a spin the window thread pays for every frame; above the
/// ceiling it is not a period but a date, which a view has no business
/// keeping — it reads the node for that.
const MIN_TICK_MS: i64 = 16;
const MAX_TICK_MS: i64 = 60 * 60 * 1_000;

/// A `clock.ticks` payload: the period in milliseconds.
fn tick_period(payload: &[u8]) -> Option<std::time::Duration> {
    let millis = methods::decode::<i64>(payload).ok()?;
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

#[cfg(test)]
pub(super) mod tests;
