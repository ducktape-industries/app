//! The kernel contract: what EVERY view may ask of the app, with no
//! per-program code on this side of the wire. A view that speaks only this
//! contract is replaced by a program deployment alone — the app binary
//! never changes for it.
//!
//! - `rpc.query` `{target, query}` — one query on the connected node: the
//!   JSON `query` is the payload of a signed frame to `target`, and the
//!   bytes the program `Respond`ed come back as they are. `rpc.view` is the
//!   same door (a program answers its own views). `rpc.query_bytes`
//!   `{target, body_b64}` sends exact binary query bytes and returns raw bytes.
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
//! - `fs.*`, `clipboard.*` — device file grants and the clipboard, in
//!   `filesystem`.
//! - `media.*`, `audio.*`, `video.*` — the raw capture and playout devices,
//!   in `media`; `notify.show` — one desktop notice, in `notify`.
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

mod node;
mod replies;

pub(super) use node::{Items, NodeTask, spawn_device, spawn_subscription};
use node::{blob_get, live, query, query_bytes, spawn, submit, submit_bytes};
pub(super) use replies::Replies;

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
    if super::filesystem::answer(guest, capability, operation, id, payload)
        || super::media::answer(guest, capability, operation, id, payload)
        || super::notify::answer(guest, capability, operation, id, payload)
    {
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
        ("rpc", "query_bytes") => spawn(guest, id, payload, query_bytes),
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

#[cfg(test)]
mod tests;
