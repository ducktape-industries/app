//! Desktop notices. MODULES DECIDE, THE INBOX CARRIES, THE HOST SHOWS: a
//! program writes finished notices on chain; the inbox at the RPC address
//! keeps them per account; this layer is the app's view of that inbox
//! (`center`), filled by `follow` and drawn by the shell, and decides on
//! this device whether an arriving entry becomes a banner:
//!
//! - the person's word on the program: none yet → the centre asks at its
//!   top ("Show banners from X? Allow · Silent"); Silent → counted only;
//!   Allow → on to the rest. Keeping a program out altogether is the
//!   inbox's mute, synced across devices (`Center::set_muted`);
//! - banners off for the device (`desktop_notifications`) → counted only;
//! - the program is the view the person is looking at → counted only,
//!   unless they asked to see banners in front too;
//! - a token bucket per program (the burst limit a minute): past it, one
//!   standing banner per program says how many more are waiting.
//!
//! An entry under a non-empty `tag` replaces the program's standing banner
//! under it, and folds the centre's rows under it into one with a count.
//! An entry read, retracted or hidden anywhere takes the banner down.
//!
//! A view that knows the reader has seen a tag's rows says so
//! (`notify.seen`, a height watermark): the rows at or below it read, and
//! the read sent upstream once. Another program's rows are never touched.
//!
//! A click on a banner opens its row, the way the centre's row does:
//! freedesktop's `ActionInvoked` on the banner's default action, and on
//! macOS the notification centre's delegate hearing the default action on a
//! banner that names its row (in its `userInfo`).

use std::sync::OnceLock;

use super::kernel::in_order;
use super::wire::methods::{self, Capability, Seen, refusal};
use super::{Guest, Intent};

mod center;
mod follow;
mod settings;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use center::{CenterHandle, InboxState, Notice, Opened, Row, center, wall};
#[cfg(test)]
pub(crate) use follow::start_with;
pub(crate) use follow::{Follower, start};
pub(crate) use settings::{
    BURSTS, Permission, Settings, save_banners, save_burst, save_in_front, set_permission,
};

pub(super) fn answer(
    guest: &mut Guest,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        (Capability::Notify, "seen") => seen(guest, id, payload),
        _ => return false,
    }
    true
}

/// `notify.seen`: a watermark. The program's rows under the tag at or
/// below the height are read, the read goes upstream when the max moved,
/// and the banner standing for an entry among them goes down.
fn seen(guest: &mut Guest, id: u64, payload: &[u8]) {
    let seen = match methods::decode::<Seen>(payload) {
        Ok(seen) => seen,
        Err(error) => {
            guest.refuse(id, refusal::MALFORMED_REQUEST, error);
            return;
        }
    };
    if center()
        .lock()
        .watermark(guest.module, &seen.tag, seen.through_height)
    {
        // the bell and the centre redraw
        guest.intents.push(Intent::Notified);
    }
    guest.reply(id, Ok(methods::encode(&())));
}

/// The banner under `tag` comes down, queued behind the banners already
/// raised (the centre, as the entry it stands for is read or gone).
pub(super) fn withdraw(tag: String) {
    in_order("inbox", move || platform::withdraw(&tag));
}

/// What opens a clicked banner's link: the shell's, handed over at startup
/// ([`on_open_link`]) so this layer never calls up into it.
static OPEN_LINK: OnceLock<fn(String)> = OnceLock::new();
/// What wakes the shell when the inbox moved the centre off the window
/// thread ([`on_wake`]): the bell reads the centre again.
static WAKE: OnceLock<fn()> = OnceLock::new();

/// The shell says how a link a banner click opens reaches `Windows`.
pub(crate) fn on_open_link(open: fn(String)) {
    let _ = OPEN_LINK.set(open);
}

/// The shell says how the centre's changes off its thread reach it.
pub(crate) fn on_wake(wake: fn()) {
    let _ = WAKE.set(wake);
}

pub(crate) fn wake() {
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// A banner clicked: its row opens as if picked in the centre — read now,
/// and its link opened. A link that does not mint opens nothing.
fn clicked(entry: u64) {
    let opened = {
        let mut center = center().lock();
        center.row_of(entry as i64).and_then(|row| center.open(row))
    };
    wake();
    let Some(Opened { link: Some(link) }) = opened else {
        tracing::info!(target: "ducktape::app", reason = "notice_link_unminted", "a banner's notice has no link this app opens");
        return;
    };
    match OPEN_LINK.get() {
        Some(open) => open(link),
        None => {
            tracing::error!(target: "ducktape::app", reason = "no_link_opener", "a banner's link could not be opened")
        }
    }
}

/// Once, at start: the log the centre kept on this device before it
/// became a view of the inbox. No backcompat: the inbox has the rows.
pub(crate) fn remove_old_log(config: &std::path::Path) {
    let old = config.join("notifications");
    if old.is_dir()
        && let Err(error) = std::fs::remove_dir_all(&old)
    {
        tracing::warn!(target: "ducktape::app", path = %old.display(), %error, "the old notification log was not removed");
    }
}

// Each platform module exposes `post(&Notice, row: Option<u64>) -> raised`
// and `withdraw(tag)`. Both block on OS calls and run only through
// `in_order` under the program's name, on the blocking pool; a click on a
// banner calls back `clicked(row)`.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(target_os = "macos"))]
mod freedesktop;
#[cfg(not(target_os = "macos"))]
use freedesktop as platform;
