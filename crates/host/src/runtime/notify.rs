//! Desktop notices. VIEWS ASK, THE HOST DECIDES: a view hands the host a
//! notice (`notify.post`) and the host records
//! it in its notification centre (per network, on this device) and decides
//! whether a banner reaches the screen:
//!
//! - the person's word on the view: none yet → logged, its newest few
//!   rows only, and the view's window asks them (the permission bar); a
//!   call still rings;
//!   Block → dropped, not even logged; Silent → logged only; Allow → on
//!   to the rest;
//! - banners off for the device (`desktop_notifications`) → logged only;
//! - the view is the window the person is looking at → logged only, unless
//!   they asked to see banners in front too;
//! - a token bucket per view (the burst limit a minute): past it, one
//!   standing banner per view says how many more are waiting.
//!
//! A non-empty `tag` replaces the view's standing banner under it, and
//! folds the centre's rows under it into one with a count.
//!
//! A view that knows the reader has seen what it posted under a tag says so
//! (`notify.seen`): its own rows under the tag read, its standing banner
//! under it taken down. Another view's rows are never touched.
//!
//! A notice with an `until` is never raised past it, and its banner is
//! taken down at it by a timer of the host's ([`withdraw_at`]). A call
//! ([`NoticeKind::Call`]) is the OS's call banner (Answer opens its row as
//! a click does; Decline takes it down), never held back by the burst.
//!
//! A click on a banner opens its row, the way the centre's row does:
//! freedesktop's `ActionInvoked` on the banner's default action, and on
//! macOS the notification centre's delegate hearing the default action on a
//! banner that names its row (in its `userInfo`).
//!
//! The host posts notices of its own: the self-update's "ready" and
//! "installed". What is a notice is decided by whoever owns it: the host
//! owns its update notices, as each program owns its own. They are no
//! view's, so no view's word and no burst limit; only the device's banners
//! switch decides the banner (the shell's `post_host`), and a click on it
//! opens Settings › Updates. Each gets a row in the centre
//! ([`center::Center::post_host`]), the program [`HOST`]'s: a row of the
//! device's own, not the chain's, so it shows whichever network is open
//! and before any is. "ready" and "installed" stand under tags of their
//! own: under one, a newer notice takes the row's place (a newer release's
//! "ready"); "installed", and a "ready" withdrawn (the release dropped or
//! rolled back, or none staged as the app starts), take "ready"'s row out
//! ([`center::Center::withdraw_host`]) and leave "installed"'s. A click on
//! the row opens Settings › Updates, as the banner's does.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use super::kernel::{in_order, spawn_reply};
use super::notifier::Recipient;
use super::wire::methods::{self, Capability, Delivery, Notification, refusal};
use super::{Intent, View};

mod center;
mod settings;
#[cfg(test)]
mod tests;

pub use center::{CenterHandle, Entry, HOST, Notice, NoticeKind, center, wall};
pub(crate) use center::{Log, log_path as log_file};
pub use settings::{
    BURSTS, Permission, Settings, not_now, save_banners, save_burst, save_in_front, set_permission,
};

/// A notice with words, and links that are `duck://` links or nothing.
fn checked(post: Notification) -> Result<Notification, &'static str> {
    if post.title.is_empty() && post.body.is_empty() {
        return Err("a notice carries neither a title nor a body");
    }
    if !post.link.is_empty() && !post.link.starts_with("duck://") {
        return Err("a notice's link is a duck:// link");
    }
    if !post.end_link.is_empty() && !post.end_link.starts_with("duck://") {
        return Err("a call's end link is a duck:// link");
    }
    Ok(post)
}

pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        (Capability::Notify, "post") => {
            let center = view.center().clone();
            post(view, &center, id, payload)
        }
        (Capability::Notify, "seen") => seen(view, id, payload),
        _ => return false,
    }
    true
}

/// Whether `view`'s notices are too late for `center` (held locked, so a
/// chain taken up in hand meanwhile waits for the answer): a view from
/// before the app's last (re)connect or of a followed set that left, or,
/// the person's, one of another chain than the centre holds (the chain
/// just left, between the centre taking the next one up and the connect
/// that stops its notifiers).
fn stale(view: &View, center: &center::Center) -> bool {
    view.connection.stale()
        || (view.recipient.is_none() && center.network() != view.connection.chain)
}

const STALE: &str = "view belongs to another network connection than its centre's";

/// `notify.post` into `center` (the view's, [`View::center`]): judged by
/// the settings as last read ([`Settings::kept`]), not a prefs read per
/// post, or by its recipient's; the log is written at the end of the
/// redraw ([`center::Center::flush`]). Refused for a view too late for it
/// ([`stale`]).
fn post(view: &mut View, center: &CenterHandle, id: u64, payload: &[u8]) {
    let post = methods::decode::<Notification>(payload)
        .and_then(|post| checked(post).map_err(str::to_owned));
    let post = match post {
        Ok(post) => post,
        Err(error) => {
            view.refuse(id, refusal::MALFORMED_REQUEST, error);
            return;
        }
    };
    let settings = view
        .recipient
        .as_ref()
        .and_then(|recipient| recipient.settings.clone())
        .unwrap_or_else(Settings::kept);
    let (posted, banner, entry) = {
        let mut center = center.lock();
        if stale(view, &center) {
            drop(center);
            return view.refuse(id, refusal::STALE_CONNECTION, STALE);
        }
        let (posted, banner) = center.post(
            &settings,
            view.program,
            &view.name,
            post,
            Instant::now(),
            wall(),
        );
        // a click on its banner opens the row just logged
        let entry = (posted == Delivery::Banner)
            .then(|| center.logged())
            .flatten();
        (posted, banner, entry)
    };
    // the bell and the bar redraw
    view.intents.push(Intent::Notified);
    // queued now, in the order the view posted, not when the task runs
    let raised = banner.map(|notice| {
        if notice.until != 0 {
            withdraw_at(
                center.clone(),
                view.recipient.clone(),
                view.program,
                (notice.chain.clone(), notice.tag.clone()),
                notice.until,
            );
        }
        let (tell, told) = tokio::sync::oneshot::channel();
        let recipient = view.recipient.clone();
        in_order(&queue_of(view.program, recipient.as_deref()), move || {
            let banners = banners_of(recipient.as_deref());
            let click = entry.map(|row| Click::Row {
                chain: notice.chain.clone(),
                row,
            });
            let _ = tell.send(banners.post(&notice, click));
        });
        told
    });
    spawn_reply(view, id, async move {
        let raised = match raised {
            None => false,
            Some(told) => told.await.unwrap_or(false),
        };
        let posted = match posted {
            Delivery::Banner if !raised => Delivery::Logged,
            posted => posted,
        };
        Ok(methods::encode(&posted))
    });
}

/// `notify.seen`: the view's rows under the tag read, and its standing
/// banner under it down, queued behind the banners it already raised.
/// Refused for a view too late for its centre ([`stale`]).
fn seen(view: &mut View, id: u64, payload: &[u8]) {
    let tag = match methods::decode::<String>(payload) {
        Ok(tag) => tag,
        Err(error) => {
            view.refuse(id, refusal::MALFORMED_REQUEST, error);
            return;
        }
    };
    let read = {
        let mut center = view.center().lock();
        (!stale(view, &center)).then(|| center.read_tag(view.program, &tag))
    };
    let Some(read) = read else {
        return view.refuse(id, refusal::STALE_CONNECTION, STALE);
    };
    if read {
        // the bell and the centre redraw
        view.intents.push(Intent::Notified);
    }
    if !tag.is_empty() {
        let tag = format!("{}/{tag}", view.program);
        let chain = view.connection.chain.clone();
        let recipient = view.recipient.clone();
        in_order(&queue_of(view.program, recipient.as_deref()), move || {
            banners_of(recipient.as_deref()).withdraw(&chain, &tag)
        });
    }
    view.reply(id, Ok(methods::encode(&())));
}

/// The person declined the call raised under `tag` on `chain`'s banner:
/// the centre holding that chain's log lets its program ring again
/// ([`center::Center::declined`]). From the OS's own thread.
pub fn declined(chain: &str, tag: &str) {
    let held = match chain.is_empty() {
        true => Some(center().clone()),
        false => super::notifier::holding(chain),
    };
    if let Some(held) = held {
        held.lock().declined(tag);
    }
}

/// The banner raised under `tag` on `chain` taken down at `until` (Unix
/// seconds), from where it went (`recipient`'s channel, or the OS's),
/// queued behind `program`'s other banners, unless a later one stands there
/// by then or it was taken down already ([`Center::ring_over`]), asked of
/// the centre that holds `chain`'s log THEN (a switch moves it; `center`,
/// the one posted into, holds it on a host with no screen). A banner with
/// no tag has nothing to take it down by: it stays.
pub(super) fn withdraw_at(
    center: CenterHandle,
    recipient: Option<Arc<Recipient>>,
    program: &'static str,
    (chain, tag): (String, String),
    until: u64,
) -> tokio::task::JoinHandle<()> {
    super::kernel::handle().spawn(async move {
        // an `until` past any time the clock can name: no timer (adding it
        // would panic, and a phone's panic aborts the app)
        let Some(at) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(until))
        else {
            return;
        };
        if let Ok(left) = at.duration_since(std::time::SystemTime::now()) {
            tokio::time::sleep(left).await;
        }
        // none holds the chain's log (parked as it left hand): no notifier
        // of it runs, so nothing newer stands under the tag
        let over = || match chain.is_empty() {
            true => center.lock().ring_over(&tag, until),
            false => super::notifier::holding(&chain)
                .is_none_or(|held| held.lock().ring_over(&tag, until)),
        };
        if !tag.is_empty() && over() {
            in_order(&queue_of(program, recipient.as_deref()), move || {
                banners_of(recipient.as_deref()).withdraw(&chain, &tag)
            });
        }
    })
}

/// The sound an ordinary notice plays, by its file's name (a call keeps
/// the ring its OS gives it): `assets/sounds/quack.wav`, which `quack.py`
/// beside it makes. Each app carries it where its OS finds a notice's
/// sound (the Mac and iPhone apps' resources, Android's `res/raw`, a file
/// the Linux app keeps for its notification daemon), and pushd's alerts to
/// an iPhone name it.
pub const SOUND: &str = "quack.wav";

/// What a click on a banner opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    /// A centre row, as if picked in the centre: `row` of `chain`'s log.
    Row { chain: String, row: u64 },
    /// Settings › Updates: the host's own update notice.
    Updates,
}

/// What a click asks the shell to open.
pub enum Opened {
    /// A `duck://` link, for `Windows::open_link`.
    Link(String),
    /// `row` of `chain`'s log, a network not in hand: the shell switches
    /// to it first and opens the row there.
    Row { chain: String, row: u64 },
    /// Settings › Updates.
    Updates,
}

/// What opens what a banner click names: the shell's `Boot::open`, which
/// `start` installs ([`on_open`]) so this layer never calls up into it.
static OPEN: OnceLock<fn(Opened, Option<String>)> = OnceLock::new();

/// `Boot::open` installed: how what a banner click opens reaches the
/// shell's windows. The first one stays.
pub(crate) fn on_open(open: fn(Opened, Option<String>)) {
    let _ = OPEN.set(open);
}

/// A banner clicked: `start`'s drain calls this for each
/// `Signal::BannerClick` the shell's `Banners` impl sends through
/// `Inbound`. A row opens as if picked in the centre — read now, and its
/// `duck://` link opened (the view's seat, when it carried none). A row of
/// another chain than the one in hand goes to the shell as it is, to
/// switch first.
pub(crate) fn clicked(click: Click, activation: Option<String>) {
    tracing::info!(target: "ducktape::app", ?click, "a banner was clicked");
    let Some(opened) = opened(click) else {
        tracing::info!(target: "ducktape::app", reason = "row_gone", "a clicked banner's row is gone");
        return;
    };
    if let Opened::Link(link) = &opened {
        tracing::info!(target: "ducktape::app", %link, "a clicked banner opens its link");
    }
    match OPEN.get() {
        Some(open) => open(opened, activation),
        None => {
            tracing::error!(target: "ducktape::app", reason = "no_link_opener", "a banner's link could not be opened")
        }
    }
}

/// What `click` opens: a row of the chain in hand read now, its link (or
/// its view's seat); a row of another chain as it is; none for a row gone.
fn opened(click: Click) -> Option<Opened> {
    Some(match click {
        Click::Row { chain, row } => {
            let mut center = center().lock();
            if !chain.is_empty() && center.network() != chain {
                drop(center);
                Opened::Row { chain, row }
            } else {
                let entry = center.open(row)?;
                Opened::Link(match entry.link.is_empty() {
                    true => format!("duck://{}", entry.program),
                    false => entry.link,
                })
            }
        }
        Click::Updates => Opened::Updates,
    })
}

/// The shell's banners (`OsServices::banners`).
fn banners() -> &'static dyn crate::contract::Banners {
    crate::contract::os().banners()
}

/// The queue a program's banners run through in order, so a burst's "N
/// more" never lands before the banners it counts: named by the program
/// and the store its notifier keeps, so one per account on a host with no
/// screen, where a post is a call to a push service and one account's slow
/// call must hold up no other's, and one for the person at this device,
/// whichever network runs the program (in hand, or followed).
pub(super) fn queue_of(program: &str, recipient: Option<&Recipient>) -> String {
    format!("{program}@{}", super::store::store_of(recipient).display())
}

/// Where a notice of `recipient`'s goes: its banners, or the shell's for
/// the person at this device.
fn banners_of(recipient: Option<&Recipient>) -> &dyn crate::contract::Banners {
    match recipient {
        Some(recipient) => &*recipient.banners,
        None => banners(),
    }
}

impl View {
    /// The centre this view's notices go into: its recipient's, or the
    /// app's ([`center`]).
    pub(super) fn center(&self) -> &CenterHandle {
        self.recipient
            .as_ref()
            .map_or(center(), |recipient| &recipient.center)
    }
}

/// A banner request's identifier, new across processes and not only within
/// one: macOS replaces a delivered banner when a new request carries its
/// identifier, so one that restarts with every process takes down, unseen,
/// a banner an earlier process left standing. This process's token, then a
/// count within it. For a shell's `Banners` impl whose OS keys banners by
/// such an identifier.
pub fn request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static PROCESS: OnceLock<String> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let process =
        PROCESS.get_or_init(|| process_token(std::process::id(), std::time::SystemTime::now()));
    format!(
        "ducktape-{process}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A process told apart from every other: its pid and when it started (a
/// pid alone is reused).
fn process_token(pid: u32, started: std::time::SystemTime) -> String {
    let nanos = started
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{pid:x}-{nanos:x}")
}
