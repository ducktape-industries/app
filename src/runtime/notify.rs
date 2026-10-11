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
            let _ = tell.send(platform::post(&notice, entry));
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

/// What opens a clicked banner's link: the shell's, handed over at startup
/// ([`on_open_link`]) so this layer never calls up into it.
static OPEN_LINK: OnceLock<fn(String)> = OnceLock::new();

/// The shell says how a link a banner click opens reaches `Windows`.
pub(crate) fn on_open_link(open: fn(String)) {
    let _ = OPEN_LINK.set(open);
}

/// A banner clicked: its row opens as if picked in the centre — read now,
/// and its `duck://` link opened (the view's seat, when it carried none).
fn clicked(entry: u64) {
    let Some(entry) = center().lock().open(entry) else {
        return;
    };
    let link = match entry.link.is_empty() {
        true => format!("duck://{}", entry.module),
        false => entry.link,
    };
    match OPEN_LINK.get() {
        Some(open) => open(link),
        None => {
            tracing::error!(target: "ducktape::app", reason = "no_link_opener", "a banner's link could not be opened")
        }
    }
}

// Each platform module exposes `post(&Notice, row: Option<u64>) -> raised`
// and `withdraw(tag)`. Both block on OS calls and run only through
// `in_order` under the view's module, on the blocking pool; a click on a banner calls back
// `clicked(row)`.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
mod freedesktop;
#[cfg(not(target_os = "macos"))]
use freedesktop as platform;
