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
use super::notifier::Recipient;
use super::wire::methods::{self, Capability, Delivery, Notification, refusal};
use super::{Intent, View};

mod center;
mod settings;
#[cfg(test)]
mod tests;

pub use center::{CenterHandle, Entry, Notice, center, wall};
pub use settings::{
    BURSTS, Permission, Settings, not_now, save_banners, save_burst, save_in_front, set_permission,
};

/// A notice with words, and a link that is a `duck://` link or nothing.
fn checked(post: Notification) -> Result<Notification, &'static str> {
    if post.title.is_empty() && post.body.is_empty() {
        return Err("a notice carries neither a title nor a body");
    }
    if !post.link.is_empty() && !post.link.starts_with("duck://") {
        return Err("a notice's link is a duck:// link");
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

/// `notify.post` into `center` (the view's, [`View::center`]): judged by
/// the settings as last read ([`Settings::kept`]), not a prefs read per
/// post, or by its recipient's; the log is written at the end of the
/// redraw ([`center::Center::flush`]).
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
        .map_or_else(Settings::kept, |recipient| recipient.settings.clone());
    let (posted, banner, entry) = {
        let mut center = center.lock();
        let (posted, banner) = center.post(
            &settings,
            view.program,
            &view.name,
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
    view.intents.push(Intent::Notified);
    // queued now, in the order the view posted, not when the task runs
    let raised = banner.map(|notice| {
        let (tell, told) = tokio::sync::oneshot::channel();
        let recipient = view.recipient.clone();
        // ponytail: every recipient's banners queue under the program, one
        // after another; a queue per recipient once a channel's post is slow
        // (a call to APNs or FCM)
        in_order(view.program, move || {
            let banners = banners_of(recipient.as_deref());
            let _ = tell.send(banners.post(&notice, entry.map(Click::Row)));
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
fn seen(view: &mut View, id: u64, payload: &[u8]) {
    let tag = match methods::decode::<String>(payload) {
        Ok(tag) => tag,
        Err(error) => {
            view.refuse(id, refusal::MALFORMED_REQUEST, error);
            return;
        }
    };
    if view.center().lock().read_tag(view.program, &tag) {
        // the bell and the centre redraw
        view.intents.push(Intent::Notified);
    }
    if !tag.is_empty() {
        let tag = format!("{}/{tag}", view.program);
        let recipient = view.recipient.clone();
        in_order(view.program, move || {
            banners_of(recipient.as_deref()).withdraw(&tag)
        });
    }
    view.reply(id, Ok(methods::encode(&())));
}

/// What a click on a banner opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// A centre row, as if picked in the centre.
    Row(u64),
    /// Settings › Updates: the host's own update notice.
    Updates,
}

/// What a click asks the shell to open.
pub enum Opened {
    /// A `duck://` link, for `Windows::open_link`.
    Link(String),
    /// Settings › Updates.
    Updates,
}

/// What opens what a banner click names: the shell's, handed over at
/// startup ([`on_open`]) so this layer never calls up into it.
static OPEN: OnceLock<fn(Opened)> = OnceLock::new();

/// The shell says how what a banner click opens reaches `Windows`.
pub fn on_open(open: fn(Opened)) {
    let _ = OPEN.set(open);
}

/// A banner clicked (the shell's `Banners` impl calls this). A row opens as
/// if picked in the centre — read now, and its `duck://` link opened (the
/// view's seat, when it carried none).
pub fn clicked(click: Click) {
    let opened = match click {
        Click::Row(entry) => {
            let Some(entry) = center().lock().open(entry) else {
                return;
            };
            Opened::Link(match entry.link.is_empty() {
                true => format!("duck://{}", entry.program),
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

/// The shell's banners (`OsServices::banners`).
fn banners() -> &'static dyn crate::contract::Banners {
    crate::contract::os().banners()
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
