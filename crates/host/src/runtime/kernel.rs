//! The kernel contract: what every view may ask of the app, with no per-program code on this
//! side of the wire. Each kind of `methods::ALL` is answered here or in this directory, and
//! `tests::the_routed_kinds_are_exactly_the_methods` holds the arms to that list.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{Intent, View, wire};
use crate::backend::{self, RpcClient, refused};
use wire::methods::{self, Capability, Method as _, refusal};

fn host_fault(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::HOST_FAULT, error.to_string())
}

fn malformed(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::MALFORMED_REQUEST, error.to_string())
}

/// One answer to a view: the bytes it asked for, or the refusal that names
/// why not. Every method in this file hands back exactly this.
pub(super) type Answer = Result<Vec<u8>, wire::Error>;

// The kinds this host answers (`host.methods`): every method but those its
// build or its platform has none of, the ones `answer` hands back as
// unknown (`return false`) where they are.
fn hosted(floats: bool) -> Vec<String> {
    let floating = ["host.float", "host.unfloat", "host.placement"];
    let captures = ["camera.devices", "microphone.devices", "microphone.chime"];
    methods::ALL
        .iter()
        .filter(|kind| floats || !floating.contains(kind))
        .filter(|kind| cfg!(feature = "av") || !captures.contains(kind))
        .filter(|kind| **kind != methods::NotifyNeeds::KIND)
        .map(|kind| (*kind).to_owned())
        .collect()
}

mod describe;
pub(super) mod live;
mod node;
pub(super) mod socket;
pub use node::{Answered, artifact};
#[cfg(feature = "ui")]
pub(crate) use node::{Writes, writes};
mod replies;
pub(super) mod screen;

#[cfg(test)]
pub(super) use node::hold_for_test;
pub(super) use node::{Joints, NodeTask, reconnected, spawn_host_subscription, spawn_reply};
use node::{
    blob_get, blob_manifest, block, blocks, changes, heads, invite, network, query, spawn_asked,
    spawn_put, spawn_submit, status, tx, watch,
};
#[cfg(feature = "ui")]
pub(super) use node::{reaches_node, spawn_event, spawn_keep, spawn_save};
pub(super) use replies::Replies;
use replies::{Claim, InFlight};

/// Whether the app is in front of the person: a phone's lifecycle moves it
/// (`set_in_front`), a desktop's app always is. Behind another app no view
/// holds a socket or a poll of the node.
fn front() -> &'static tokio::sync::watch::Sender<bool> {
    static FRONT: OnceLock<tokio::sync::watch::Sender<bool>> = OnceLock::new();
    FRONT.get_or_init(|| tokio::sync::watch::channel(true).0)
}

/// The app went behind another (`false`) or came in front again.
pub fn set_in_front(in_front: bool) {
    front().send_if_modified(|front| std::mem::replace(front, in_front) != in_front);
}

/// Whether the app is in front, as a view's tasks read it.
pub(super) fn in_front() -> tokio::sync::watch::Receiver<bool> {
    front().subscribe()
}

fn debug_mode() -> &'static tokio::sync::watch::Sender<bool> {
    static DEBUG: OnceLock<tokio::sync::watch::Sender<bool>> = OnceLock::new();
    DEBUG.get_or_init(|| tokio::sync::watch::channel(crate::backend::load_debug()).0)
}

pub fn set_debug(on: bool) {
    debug_mode().send_if_modified(|was| std::mem::replace(was, on) != on);
}

pub(super) fn debug() -> tokio::sync::watch::Receiver<bool> {
    debug_mode().subscribe()
}

pub(super) use rt::task::handle;

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
        drop(rt::task::spawn(async move {
            while let Some(job) = jobs.recv().await {
                let _ = rt::task::blocking(job).await;
            }
        }));
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
    let answered_by_its_module = super::clipboard::answer(view, capability, operation, id, payload)
        || super::picker::answer(view, capability, operation, id, payload)
        || super::notify::answer(view, capability, operation, id, payload)
        || super::store::answer(view, capability, operation, id, payload, props)
        || super::secret::answer(view, capability, operation, id, payload, props);
    if answered_by_its_module {
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
        (Capability::Host, "debug") => {
            if !payload.is_empty() {
                view.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "debug subscription takes no payload",
                );
                return true;
            }
            view.debug_subscriptions.push(id);
            let on = *view.debug_mode.borrow_and_update();
            view.stream_item(id, methods::encode(&on));
        }
        (Capability::Notify, "needs") => return false,
        (Capability::Host, "methods") => {
            let hosted = methods::Hosted {
                level: wire::WIRE_LEVEL,
                kinds: hosted(view.floats),
            };
            view.reply(id, Ok(methods::HostMethods::encode_reply(&hosted)));
        }
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
                "`host.float` names a size of at least its minimum, at least 1",
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
        // the route the view shows, and the routes its program's views
        // show on screen (`screen`)
        (Capability::Host, "showing") => match methods::decode::<String>(payload) {
            Ok(route) => {
                view.intents.push(Intent::Showing(route.clone()));
                view.showing = Some(route);
                view.screen_moved();
                view.reply(id, Ok(methods::encode(&())));
            }
            Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
        },
        (Capability::Host, "screen") => {
            if !payload.is_empty() {
                view.refuse(
                    id,
                    refusal::MALFORMED_REQUEST,
                    "screen subscription takes no payload",
                );
                return true;
            }
            screen::follow(view, id);
        }
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
            if node::enveloped(view, id, payload).is_some() {
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
            if !super::consent::bound(view.program, view.connection.on.roster().roles().as_ref()) {
                view.refuse(
                    id,
                    refusal::UNDECLARED_CAPABILITY,
                    "`invite.create` is a role's own view's: this view's program is bound to no role",
                );
                return true;
            }
            spawn_asked(view, id, payload, invite, "host_call.invite.create")
        }
        // a write: to any program, after the person's native yes where
        // the op removes or suspends a key or an agent, or is a view no
        // vote reviewed writing to a role (`consent::needed`)
        (Capability::Op, "submit") => {
            let Some(call) = node::enveloped(view, id, payload) else {
                return true;
            };
            // the account the seated key holds, as the app resolved it
            // (`roster::props`): the card tells the person's own account
            // from an agent's by it
            let own = props
                .as_deref()
                .and_then(|props| wire::methods::decode::<wire::methods::Session>(props).ok())
                .and_then(|session| session.account);
            let roles = view.connection.on.roster().roles();
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
        // blobs put in the person's name, with no card: the host builds
        // the blobs-role op itself from the bytes the view hands it
        // (`node::put`), so a view can only ask it to keep bytes, never
        // craft a blobs op (a `Receipt`). The card's reason, a view
        // crafting an op to a role's program (`consent::needed`), does not
        // arise.
        (Capability::Op, "put") => spawn_put(view, id, payload),
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
        (Capability::Program, "watch") => watch(view, id, payload),
        (Capability::Chain, "heads") => heads(view, id, payload),
        (Capability::Link, "open") => {
            open_link(view, id, payload, methods::LinkOpen::KIND, Intent::OpenLink)
        }
        (Capability::Link, "window") => open_link(
            view,
            id,
            payload,
            methods::LinkWindow::KIND,
            Intent::OpenWindow,
        ),
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
                ) if !settings_page(place) => view.refuse(
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
        (Capability::Socket, "open") => socket::open(view, id, payload),
        (Capability::Socket, "send") => socket::send(view, id, payload),
        (Capability::Host, "random") => socket::random(view, id, payload),
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

/// Whether a `host.float` names a size a window can have: each side at
/// least its minimum, the minimum at least 1. The shell fits it to the
/// display it opens on.
fn floatable(float: &methods::Float) -> bool {
    (1..=float.width).contains(&float.min_width) && (1..=float.height).contains(&float.min_height)
}

fn open_link(
    view: &mut View,
    id: u64,
    payload: &[u8],
    kind: &'static str,
    opened: fn(String) -> Intent,
) {
    let link = match methods::decode::<String>(payload) {
        Ok(link) => link,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    if !openable(&link) {
        let refused = format!("`{kind}` opens a duck:// or https:// link");
        return view.refuse(id, refusal::MALFORMED_REQUEST, refused);
    }
    if !view.take_activation() {
        let refused = format!("`{kind}` needs a press or key");
        return view.refuse(id, refusal::NEEDS_GESTURE, refused);
    }
    view.intents.push(opened(link));
    view.reply(id, Ok(Vec::new()));
}

/// Whether `link.open` may open `link`: `duck://` or `https://` with
/// something after it.
fn openable(link: &str) -> bool {
    ["duck://", "https://"].iter().any(|scheme| {
        link.strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    })
}

pub use rt::process::local_offset;

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

/// Whether this host has a settings page for `place`'s device
/// (`render::av`): a host with no screen has none.
fn settings_page(place: methods::Place) -> bool {
    #[cfg(feature = "ui")]
    return crate::render::av::settings_page(place).is_some();
    #[cfg(not(feature = "ui"))]
    {
        let _ = place;
        false
    }
}
