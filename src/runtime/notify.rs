//! Desktop notices. VIEWS ASK, THE HOST DECIDES: a view hands the host a
//! notice (`notify.post`) and the host records
//! it in its notification centre (per network, on this device) and decides
//! whether a banner reaches the screen:
//!
//! - the person's word on the view: none yet → logged, its newest few
//!   rows only, and the view's window asks them (the permission bar);
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
//! A click on a banner opens its row, the way the centre's row does:
//! freedesktop's `ActionInvoked` on the banner's default action, and on
//! macOS the notification centre's delegate hearing the default action on a
//! banner that names its row (in its `userInfo`).
//!
//! The host posts one notice of its own ([`post_host`]): the self-update's
//! "ready". It is no view's, so no row, no view's word and no burst limit;
//! only the device's banners switch decides it, and a click on it opens
//! Settings › Updates.

use std::sync::OnceLock;
use std::time::Instant;

use super::kernel::{in_order, spawn_reply};
use super::wire::methods::{self, Capability, Delivery, Notification, refusal};
use super::{Guest, Intent};

mod center;
mod settings;
#[cfg(test)]
mod tests;

pub(crate) use center::{CenterHandle, Entry, Notice, center, wall};
pub(crate) use settings::{
    BURSTS, Permission, Settings, not_now, save_banners, save_burst, save_in_front, set_permission,
};

/// The longest title, body, tag and link the host keeps. A notice is a
/// sentence, not a payload.
const MAX_TEXT: usize = 512;

/// Shortened to what a screen shows, on a character boundary; a notice
/// with no words is not one, and a link is a `duck://` link or nothing.
fn shortened(mut post: Notification) -> Result<Notification, &'static str> {
    if post.title.is_empty() && post.body.is_empty() {
        return Err("a notice carries neither a title nor a body");
    }
    if !post.link.is_empty() && !post.link.starts_with("duck://") {
        return Err("a notice's link is a duck:// link");
    }
    // a cut link goes nowhere: the cut says so, not the length after it
    let link_cut = post.link.len() > MAX_TEXT;
    for text in [
        &mut post.title,
        &mut post.body,
        &mut post.tag,
        &mut post.link,
    ] {
        if text.len() > MAX_TEXT {
            let cut = (0..=MAX_TEXT)
                .rev()
                .find(|end| text.is_char_boundary(*end))
                .unwrap_or(0);
            text.truncate(cut);
        }
    }
    if link_cut {
        post.link.clear();
    }
    Ok(post)
}

pub(super) fn answer(
    guest: &mut Guest,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        (Capability::Notify, "post") => post(guest, center(), id, payload),
        (Capability::Notify, "seen") => seen(guest, id, payload),
        _ => return false,
    }
    true
}

/// `notify.post` into `center`: judged by the settings as last read
/// ([`Settings::kept`]), not a prefs read per post; the log is written at
/// the end of the redraw ([`center::Center::flush`]).
fn post(guest: &mut Guest, center: &CenterHandle, id: u64, payload: &[u8]) {
    let post = methods::decode::<Notification>(payload)
        .and_then(|post| shortened(post).map_err(str::to_owned));
    let post = match post {
        Ok(post) => post,
        Err(error) => {
            guest.refuse(id, refusal::MALFORMED_REQUEST, error);
            return;
        }
    };
    let (posted, banner, entry) = {
        let mut center = center.lock();
        let (posted, banner) = center.post(
            &Settings::kept(),
            guest.module,
            &guest.name,
            post,
            Instant::now(),
            wall(),
        );
        // the row just logged is the newest: a click on its banner opens it
        let entry = (posted == Delivery::Banner)
            .then(|| center.entries().next().map(|entry| entry.id))
            .flatten();
        (posted, banner, entry)
    };
    // the bell and the bar redraw
    guest.intents.push(Intent::Notified);
    // queued now, in the order the view posted, not when the task runs
    let raised = banner.map(|notice| {
        let (tell, told) = tokio::sync::oneshot::channel();
        in_order(guest.module, move || {
            let _ = tell.send(platform::post(&notice, entry.map(Click::Row)));
        });
        told
    });
    spawn_reply(guest, id, async move {
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
fn seen(guest: &mut Guest, id: u64, payload: &[u8]) {
    let tag = match methods::decode::<String>(payload) {
        Ok(tag) => tag,
        Err(error) => {
            guest.refuse(id, refusal::MALFORMED_REQUEST, error);
            return;
        }
    };
    if center().lock().read_tag(guest.module, &tag) {
        // the bell and the centre redraw
        guest.intents.push(Intent::Notified);
    }
    if !tag.is_empty() {
        let tag = format!("{}/{tag}", guest.module);
        in_order(guest.module, move || platform::withdraw(&tag));
    }
    guest.reply(id, Ok(methods::encode(&())));
}

/// What a click on a banner opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Click {
    /// A centre row, as if picked in the centre.
    Row(u64),
    /// Settings › Updates: the host's own update notice.
    Updates,
}

/// What a click asks the shell to open.
pub(crate) enum Opened {
    /// A `duck://` link, for `Windows::open_link`.
    Link(String),
    /// Settings › Updates.
    Updates,
}

/// What opens what a banner click names: the shell's, handed over at
/// startup ([`on_open`]) so this layer never calls up into it.
static OPEN: OnceLock<fn(Opened)> = OnceLock::new();

/// The shell says how what a banner click opens reaches `Windows`.
pub(crate) fn on_open(open: fn(Opened)) {
    let _ = OPEN.set(open);
}

/// A banner clicked. A row opens as if picked in the centre — read now, and
/// its `duck://` link opened (the view's seat, when it carried none).
fn clicked(click: Click) {
    let opened = match click {
        Click::Row(entry) => {
            let Some(entry) = center().lock().open(entry) else {
                return;
            };
            Opened::Link(match entry.link.is_empty() {
                true => format!("duck://{}", entry.module),
                false => entry.link,
            })
        }
        Click::Updates => Opened::Updates,
    };
    match OPEN.get() {
        Some(open) => open(opened),
        None => {
            tracing::error!(target: "ducktape::app", reason = "no_link_opener", "a banner's link could not be opened")
        }
    }
}

/// The host's own notice: up if the device's banners are on, a click
/// opening Settings › Updates. One stands for the install, whichever of its
/// processes raised it: a newer one takes its place, and [`withdraw_host`]
/// takes it down. Blocks on the OS call, so the caller is a thread of its
/// own. Whether it went up.
pub(crate) fn post_host(notice: &Notice) -> bool {
    Settings::load().banners && platform::post(notice, Some(Click::Updates))
}

/// The host's notice down, whichever process raised it. Blocks on the OS
/// call, as [`post_host`] does.
pub(crate) fn withdraw_host() {
    platform::withdraw_host();
}

/// A banner request's identifier, new across processes and not only within
/// one: macOS replaces a delivered banner when a new request carries its
/// identifier, so one that restarts with every process takes down, unseen,
/// a banner an earlier process left standing. This process's token, then a
/// count within it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn request_id() -> String {
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

// Each platform module exposes `post(&Notice, click: Option<Click>) -> raised`
// and `withdraw(tag)`. Both block on OS calls; a view's run only through
// `in_order` under the view's module, on the blocking pool, and the host's
// on the thread that posts it. A click on a banner calls back
// `clicked(click)`.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
mod freedesktop;
#[cfg(not(target_os = "macos"))]
use freedesktop as platform;
