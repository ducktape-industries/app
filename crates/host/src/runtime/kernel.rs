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
//! - `host.visible` — a subscription to whether the person is looking at
//!   the view: its pane is on a desk of the window in front.
//! - `host.float` — the view's pane in a window of its own, kept above
//!   other windows where the platform can (the shell does it), at a press
//!   or key ([`View::take_activation`]); `host.unfloat` — back on the desk;
//!   `host.placement` — a subscription to where the pane is (floating, kept
//!   on top), as the seat places it.
//! - `host.route` — a subscription that gets the route a `duck://` link
//!   asked of this view (`explorer/tx/<hash>` → `tx/<hash>`), once,
//!   DECODED: the link's `%XX` escapes read back (`chat/forge%3Aweb%3A3`
//!   → `forge:web:3`), checked by `valid_route`. The view whose manifest
//!   declares `web` gets an http(s) link the app opens as its whole
//!   address instead, checked by the web piece's own rule
//!   (`render/web.rs` `view_for`: https, or http on this machine, no user
//!   name or password).
//! - `host.badge` — the tab's unread count; `host.title` — the name of the
//!   view's window, cleaned ([`shown_title`]); `host.id` — a minted id under a
//!   prefix word; `link.open` — the one way out: a `duck://` link, or an
//!   `https://` one for the system browser; `clock.ticks` — a periodic
//!   empty item, at most one per redraw ([`ticked`]).
//! - `camera.devices`, `microphone.devices` — the devices the system lists
//!   now (`render/av.rs`), listed on a blocking thread; `microphone.chime`
//!   — the test chime on a speaker, at a press or key, answered once it
//!   has sounded (on a blocking thread). A build without feature `av`
//!   knows none of the three (`unknown_request`).
//!
//! On the node, as a task on [`handle`] (`node`), each answer waiting in
//! [`Replies`] for the view's next redraw; each asked once, a transport
//! failure being the answer:
//! - `program.query` `Call{target, body}` — one query on the connected node:
//!   `body` is the payload of a signed frame to `target`, and the bytes the
//!   program `Respond`ed come back as they are. It reads the node's
//!   preconfirmed layer: an op this view was just answered for is already
//!   in what it reads.
//! - `op.submit` `Call{target, body}` — one op, signed with the key
//!   unlocked in this session and submitted;
//!   answered with the receipt's output, or the program's refusal. The
//!   receipt is the node's preconfirmation, not finality: the op ran over
//!   the preconfirmed layer and waits for a block, and until one carries it
//!   the node can still drop it (a block that lands first and makes it
//!   fail, a node restart) with no word to anyone; a view shows what
//!   `program.query` reads. A lost answer is reported
//!   ([`refusal::NODE_FAILED`]), never followed by a second submission.
//! - `chain.status` — node status as `NodeStatus`.
//! - `chain.blocks` `BlockPage` — finalized blocks, newest first, as the
//!   node lists them (`/v1/blocks`): each transaction's size, its payload
//!   when at most `most` bytes, and the lowest height the node lists;
//!   `chain.block` `BlockAsk` — one by height or id (`/v1/block`), each
//!   payload when at most `most` bytes; `chain.tx` `TxAsk` — one
//!   transaction by its place or hash, its payload when at most `most`
//!   bytes (`/v1/tx`).
//! - `blob.get` `<id>` — a blob by `sha256:<hex>` or `sha1:<hex>` id, unframed.
//! - `program.describe` `(program, op)` — the op as the program's own
//!   describe module reads it, or `None` (`describe`).
//! - `invite.create` `CreateInvite{ttl_days}` — `Invite{invite, notes}`. Node refusals retain their tokens.
//! - `program.changes` `<program>` — a subscription that gets one item per
//!   block that wrote to `program` (`/v1/changes/<program>`): the block's
//!   height and the keys it wrote, as the node publishes them, so the view
//!   re-reads what moved; `None` when the link was reopened and the view
//!   should re-read everything.
//! - `chain.heads` — a subscription that gets one `Head` per finalized block,
//!   oldest first: the host reads the node's status at half its block
//!   time and fills each advance from the block archive.
//!
//! A connect inside the network opens both again on the new node
//! (`reconnected`): neither ends on it.
//!
//! In `live`, through the connection's one live session with the node's
//! relay (`backend::live`), off consensus: `live.publish`, `live.send`,
//! `live.subscribe`, `live.tracks`. Each item wakes its view at once,
//! through [`Replies`] like any answer. A connect inside the network
//! opens them again too.
//!
//! In a sibling module [`answer`] asks first:
//! - `clipboard.read`, `clipboard.write` — `clipboard`, run on the window
//!   thread after the tick.
//! - `notify.post`, `notify.seen` — `notify`, the notification centre.
//! - `store.get`, `store.set` — `store`, the view's file on this device.
//!
//! Not the kernel's — `View::answer` (`view/requests.rs`) takes them once
//! [`answer`] says `false`, since they read the view's own state:
//! - `host.widget` — a widget command; `host.session` — a subscription to
//!   the session basics (`Session`: the account, theme, chain and read-only
//!   endpoint); `host.log` — a line into app.log.
//!
//! A refusal's code is one of [`refusal`]'s, or the node's or the program's
//! own, carried through.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{Intent, View, wire};
use crate::backend::{self, RpcClient, refused};
use wire::methods::{self, Capability, refusal};

fn host_fault(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::HOST_FAULT, error.to_string())
}

fn malformed(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::MALFORMED_REQUEST, error.to_string())
}

/// One answer to a view: the bytes it asked for, or the refusal that names
/// why not. Every method in this file hands back exactly this.
pub(super) type Answer = Result<Vec<u8>, wire::Error>;

mod describe;
pub(super) mod live;
mod node;
pub use node::Answered;
mod replies;

pub(super) use node::{NodeTask, reconnected, spawn_reply};
use node::{
    blob_get, blob_manifest, block, blocks, changes, heads, invite, network, query, spawn_asked,
    spawn_submit, status, tx,
};
pub(super) use replies::Replies;
use replies::{Claim, InFlight};

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
/// banners (`notify`, under its program), so a burst's "N more" never lands
/// before the banners it counts, a view's store file (`store`, under its
/// path), so a `get` reads the `set` before it, and the notification log
/// (under its path). One task per name on [`handle`] runs them; [`drain`]
/// waits on every one of them as the app quits.
pub(super) fn in_order(name: &str, job: impl FnOnce() + Send + 'static) {
    let mut queues = queues();
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

/// Done once every job queued so far under every [`in_order`] name has
/// run: one more job behind each queue's last, and they run in order. The
/// app's quit waits on it (`runtime::quitting`), so a `set` a view was
/// answered, or a log the centre handed over, is on disk before the
/// process ends.
pub(super) fn drain() -> impl Future<Output = ()> {
    let ran: Vec<_> = queues()
        .values()
        .map(|queue| {
            let (tell, told) = tokio::sync::oneshot::channel::<()>();
            let _ = queue.send(Box::new(move || {
                let _ = tell.send(());
            }));
            told
        })
        .collect();
    async move {
        for told in ran {
            let _ = told.await;
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;
type Queues = HashMap<String, tokio::sync::mpsc::UnboundedSender<Job>>;

/// [`in_order`]'s queues by name.
fn queues() -> std::sync::MutexGuard<'static, Queues> {
    static QUEUES: OnceLock<Mutex<Queues>> = OnceLock::new();
    QUEUES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Routes one kernel request; `false` when the kind is not the kernel's.
pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
    props: &Option<Vec<u8>>,
) -> bool {
    if super::clipboard::answer(view, capability, operation, id, payload)
        || super::notify::answer(view, capability, operation, id, payload)
        || super::store::answer(view, capability, operation, id, payload)
    {
        return true;
    }
    match (capability, operation) {
        (Capability::Host, "visible") => {
            if !payload.is_empty() {
                view.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "visibility subscription takes no payload",
                );
                return true;
            }
            view.visibility_subscriptions.push(id);
            view.pending.push(wire::Event::Response {
                id,
                result: Ok(methods::encode(&view.visible)),
                done: false,
            });
        }
        // a host with no window to float a pane into (a phone's) knows
        // none of the three
        (Capability::Host, "float" | "unfloat" | "placement") if !view.floats => return false,
        (Capability::Host, "placement") => {
            if !payload.is_empty() {
                view.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "placement subscription takes no payload",
                );
                return true;
            }
            view.placement_subscriptions.push(id);
            view.stream_item(id, methods::encode(&view.placement));
        }
        // a window above every other app's: only at the person's press or
        // key (a view on a clock never raises one), taken by the ask. The
        // reply is what the platform allows; `host.placement` follows with
        // what it did
        (Capability::Host, "float") => match methods::decode::<methods::Float>(payload) {
            Ok(float) if floatable(&float) => {
                if !view.take_activation() {
                    view.refuse(
                        id,
                        refusal::NEEDS_GESTURE,
                        "`host.float` needs a press or key",
                    );
                    return true;
                }
                view.intents.push(Intent::Float(float));
                let placement = methods::Placement {
                    floating: true,
                    on_top: float.on_top && view.keeps_above,
                };
                view.reply(id, Ok(methods::encode(&placement)));
            }
            Ok(_) => view.refuse(
                id,
                refusal::MALFORMED_REQUEST,
                format!(
                    "`host.float` names a size from its minimum (at least 1) up to {MAX_FLOAT}"
                ),
            ),
            Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        // it only takes a window away: no press needed
        (Capability::Host, "unfloat") => match methods::decode::<()>(payload) {
            Ok(()) => {
                view.intents.push(Intent::Unfloat);
                view.reply(id, Ok(Vec::new()));
            }
            Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        (Capability::Host, "route") => {
            if !payload.is_empty() {
                view.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "route subscription takes no payload",
                );
                return true;
            }
            view.route_subscriptions.push(id);
            view.sync_route();
        }
        (Capability::Program, "query") => {
            if node::targeted(view, id, payload, "program.query").is_some() {
                spawn_asked(view, id, payload, query, "host_call.program.query")
            }
        }
        (Capability::Chain, "status") => {
            spawn_asked(view, id, payload, status, "host_call.chain.status")
        }
        (Capability::Chain, "network") => {
            spawn_asked(view, id, payload, network, "host_call.chain.network")
        }
        (Capability::Chain, "blocks") => {
            spawn_asked(view, id, payload, blocks, "host_call.chain.blocks")
        }
        (Capability::Chain, "block") => {
            spawn_asked(view, id, payload, block, "host_call.chain.block")
        }
        (Capability::Chain, "tx") => spawn_asked(view, id, payload, tx, "host_call.chain.tx"),
        // the node's key signs an invite: only a role's own view asks
        // for one (a view no vote reviewed could hand it to its program)
        (Capability::Invite, "create") => {
            if !super::consent::bound(view.program, super::roster().roles().as_ref()) {
                view.refuse(
                    id,
                    refusal::UNDECLARED_CAPABILITY,
                    "`invite.create` is a role's own view's: this view's program is bound to no role",
                );
                return true;
            }
            spawn_asked(view, id, payload, invite, "host_call.invite.create")
        }
        // a write: to a declared target, and after the person's native
        // yes where the op removes or suspends a key or an agent, or is a
        // view no vote reviewed writing to a role (`consent::needed`)
        (Capability::Op, "submit") => {
            let Some(call) = node::targeted(view, id, payload, "op.submit") else {
                return true;
            };
            // the account the seated key holds, as the app resolved it
            // (`roster::props`): the card tells the person's own account
            // from an agent's by it
            let own = props
                .as_deref()
                .and_then(|props| wire::methods::decode::<wire::methods::Session>(props).ok())
                .and_then(|session| session.account);
            let roles = super::roster().roles();
            match super::consent::needed(
                view.program,
                roles.as_ref(),
                &call.target,
                &call.body,
                own,
            ) {
                None => spawn_submit(view, id, payload),
                Some(words) => node::spawn_consented(view, id, payload, call, words),
            }
        }
        (Capability::Blob, "get") => spawn_asked(view, id, payload, blob_get, "host_call.blob.get"),
        (Capability::Blob, "manifest") => {
            spawn_asked(view, id, payload, blob_manifest, "host_call.blob.manifest")
        }
        (Capability::Program, "describe") => spawn_asked(
            view,
            id,
            payload,
            describe::describe,
            "host_call.program.describe",
        ),
        (Capability::Program, "changes") => changes(view, id, payload),
        (Capability::Chain, "heads") => heads(view, id, payload),
        // the one way out: a `duck://` link, or an `https://` one for the
        // system browser (and where they open, the two privacy settings
        // pages a refused camera or microphone points at, `render/av.rs`);
        // any other scheme is refused here, at the method.
        // Only the person's activation opens one, taken by the ask: a view
        // on a clock never opens a tab or reseats the desk
        (Capability::Link, "open") => match methods::decode::<String>(payload) {
            Ok(link) if openable(&link) => {
                if !view.take_activation() {
                    view.refuse(
                        id,
                        refusal::NEEDS_GESTURE,
                        "`link.open` needs a press or key",
                    );
                    return true;
                }
                view.intents.push(Intent::OpenLink(link));
                view.reply(id, Ok(Vec::new()));
            }
            Ok(_) => view.refuse(
                id,
                refusal::MALFORMED_REQUEST,
                "`link.open` opens a duck:// or https:// link",
            ),
            Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        // one of the host's own places, as a link: only the person's press
        // or key brings one up. The reader's account is shown only where
        // there is one (`roster::props`), a refused camera's or
        // microphone's system settings only where this host has that page
        // (`render/av.rs`); a place this host does not know does not decode
        (Capability::Host, "show") => {
            match methods::decode::<methods::Place>(payload) {
                Ok(methods::Place::Account)
                    if props
                        .as_deref()
                        .and_then(|props| methods::decode::<methods::Session>(props).ok())
                        .and_then(|session| session.account)
                        .is_none() =>
                {
                    view.refuse(
                        id,
                        refusal::UNKNOWN_REQUEST,
                        "there is no reader's account to show",
                    )
                }
                Ok(
                    place @ (methods::Place::CameraSettings | methods::Place::MicrophoneSettings),
                ) if crate::render::av::settings_page(place).is_none() => view.refuse(
                    id,
                    refusal::UNKNOWN_REQUEST,
                    "this host has no settings page for the device",
                ),
                Ok(place) => {
                    if !view.take_activation() {
                        view.refuse(
                            id,
                            refusal::NEEDS_GESTURE,
                            "`host.show` needs a press or key",
                        );
                        return true;
                    }
                    view.intents.push(Intent::Show(place));
                    view.reply(id, Ok(Vec::new()));
                }
                Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
            }
        }
        // the cameras, microphones and speakers (`render/av.rs`), listed
        // on a blocking thread and answered when they are. A build without
        // av captures nothing: it has none of the three (`_ => return
        // false`, an unknown request)
        #[cfg(feature = "av")]
        (Capability::Camera, "devices") => {
            node::spawn_reply(view, id, crate::render::av::cameras())
        }
        #[cfg(feature = "av")]
        (Capability::Microphone, "devices") => {
            node::spawn_reply(view, id, crate::render::av::microphones())
        }
        // the speaker test: a sound, so only at the person's press or key
        // (a view on a clock never chimes), answered once it has sounded
        #[cfg(feature = "av")]
        (Capability::Microphone, "chime") => match methods::decode::<Option<String>>(payload) {
            Ok(output) => {
                if !view.take_activation() {
                    view.refuse(
                        id,
                        refusal::NEEDS_GESTURE,
                        "`microphone.chime` needs a press or key",
                    );
                    return true;
                }
                node::spawn_reply(view, id, crate::render::av::chime(output));
            }
            Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        (Capability::Host, "badge") => match methods::decode::<i64>(payload).ok() {
            Some(count) => {
                view.intents.push(Intent::Badge(count));
                view.reply(id, Ok(Vec::new()));
            }
            None => view.refuse(
                id,
                refusal::MALFORMED_REQUEST,
                "`host.badge` carries no count",
            ),
        },
        (Capability::Host, "title") => match methods::decode::<String>(payload) {
            Ok(title) => {
                view.intents.push(Intent::Title(shown_title(&title)));
                view.reply(id, Ok(Vec::new()));
            }
            Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        (Capability::Clock, "ticks") => match tick_period(payload) {
            Some(period) => view.clocks.push(Clock {
                id,
                period,
                due: std::time::Instant::now() + period,
            }),
            None => view.refuse(
                id,
                refusal::MALFORMED_REQUEST,
                "`clock.ticks` names no period",
            ),
        },
        // tracks through the connected node, off consensus (`live.rs`)
        (Capability::Live, "publish") => live::publish(view, id, payload),
        (Capability::Live, "send") => live::send(view, id, payload),
        (Capability::Live, "subscribe") => live::subscribe(view, id, payload),
        (Capability::Live, "tracks") => live::tracks(view, id, payload),
        (Capability::Host, "id") => {
            let prefix = methods::decode::<String>(payload).unwrap_or_default();
            let prefix = prefix.trim();
            let named =
                !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_alphanumeric());
            match named {
                true => view.reply(
                    id,
                    Ok(methods::encode(&noded_client::ids::fresh_id(prefix))),
                ),
                false => view.refuse(id, refusal::MALFORMED_REQUEST, "`host.id` names no prefix"),
            }
        }
        _ => return false,
    }
    true
}

/// A `host.title` as the window shows it, one line of the view's own text
/// ([`plain`] chars), no space at either end.
pub fn shown_title(title: &str) -> String {
    let title: String = title.chars().filter(plain).collect();
    title.trim().to_owned()
}

/// Whether the host shows a char of a view's text as it is, on one line: no
/// control character, no bidi embedding, override or isolate (each
/// reorders what follows it), no line or paragraph separator. Direction
/// marks and joiners stay: an RTL name or an emoji needs them.
pub fn plain(c: &char) -> bool {
    !(c.is_control()
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// The widest and tallest a `host.float` may ask its view laid out: a
/// window past any display is no float.
const MAX_FLOAT: u32 = 8192;

/// Whether a `host.float` names a size a window can have: each side at
/// least its minimum, the minimum at least 1, nothing past [`MAX_FLOAT`].
fn floatable(float: &methods::Float) -> bool {
    (1..=float.width).contains(&float.min_width)
        && (1..=float.height).contains(&float.min_height)
        && float.width <= MAX_FLOAT
        && float.height <= MAX_FLOAT
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
pub fn local_offset(wall: i64) -> i64 {
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

/// Unix second `seconds` as its UTC day: `27 Sep 2026`.
pub fn day(seconds: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = (seconds / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// The reader's UTC offset in minutes now, as `Event::Offset` carries it
/// (`View::sync_offset`), from the OS's local time.
pub(super) fn offset_minutes() -> i32 {
    (local_offset(super::notify::wall()) / 60) as i32
}

pub(super) struct Clock {
    pub(super) id: u64,
    period: std::time::Duration,
    due: std::time::Instant,
}

/// A `clock.ticks` payload: the period in milliseconds, more than none. A
/// period shorter than a frame ticks once a redraw ([`ticked`]).
fn tick_period(payload: &[u8]) -> Option<std::time::Duration> {
    let millis = methods::decode::<i64>(payload).ok()?;
    let named = millis > 0;
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
