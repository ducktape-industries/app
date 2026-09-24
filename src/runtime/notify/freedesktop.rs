//! Everywhere else the notifier is the freedesktop notifications service,
//! which every desktop off macOS answers through its own daemon. Pure Rust,
//! and no new dependency — zbus is already in this binary's graph.

use super::Notice;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The desktop entry the banner is filed under (`packaging`), which
/// is what gives it this app's icon and lets the desktop group it.
const DESKTOP_ENTRY: &str = "dev.ducktape.app";

/// One session-bus connection per process. A host with no session bus
/// answers every banner the same way, and says so once.
fn session_bus() -> Option<&'static zbus::blocking::Connection> {
    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
    BUS.get_or_init(|| match zbus::blocking::Connection::session() {
        Ok(connection) => Some(connection),
        Err(error) => {
            tracing::info!(
                target: "ducktape::app",
                reason = "no_session_bus",
                %error,
                "desktop notices are off on this host"
            );
            None
        }
    })
    .as_ref()
}

/// The banner a tag is standing under, so the next notice under that tag
/// replaces it instead of stacking a second one.
fn standing() -> &'static Mutex<HashMap<String, u32>> {
    static STANDING: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    STANDING.get_or_init(Mutex::default)
}

/// The freedesktop body is markup for the servers that render it, so the
/// notice's own angle brackets and ampersands are escaped rather than
/// swallowed as tags.
pub(super) fn markup_escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The centre row each clickable banner stands for, by banner id.
fn clicks() -> &'static Mutex<HashMap<u32, u64>> {
    static CLICKS: OnceLock<Mutex<HashMap<u32, u64>>> = OnceLock::new();
    CLICKS.get_or_init(Mutex::default)
}

/// One thread for the life of the process, hearing the daemon say a
/// banner was clicked: its row is opened as if picked in the centre.
fn listen(bus: &'static zbus::blocking::Connection) {
    static LISTENING: OnceLock<()> = OnceLock::new();
    LISTENING.get_or_init(|| {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.Notifications")
            .and_then(|rule| rule.member("ActionInvoked"))
            .map(|rule| rule.build());
        let messages = rule
            .and_then(|rule| zbus::blocking::MessageIterator::for_match_rule(rule, bus, Some(64)));
        let messages = match messages {
            Ok(messages) => messages,
            Err(error) => {
                tracing::info!(target: "ducktape::app", %error, "banner clicks are not heard");
                return;
            }
        };
        let spawned = std::thread::Builder::new()
            .name("notice-clicks".into())
            .spawn(move || {
                for message in messages.flatten() {
                    let Ok((banner, action)) = message.body().deserialize::<(u32, String)>() else {
                        continue;
                    };
                    let entry = clicks()
                        .lock()
                        .expect("banner clicks")
                        .get(&banner)
                        .copied();
                    if let Some(entry) = entry.filter(|_| action == "default") {
                        super::clicked(entry);
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::info!(target: "ducktape::app", %error, "banner clicks are not heard");
        }
    });
}

pub(super) fn post(notice: &Notice, entry: Option<u64>) -> bool {
    let Some(bus) = session_bus() else {
        return false;
    };
    if entry.is_some() {
        listen(bus);
    }
    // held across the call: two notices under one tag posted at once
    // would otherwise both read "none standing" and stack
    let mut standing = standing().lock().expect("standing notices");
    let replaces = match notice.tag.is_empty() {
        true => 0,
        false => standing.get(&notice.tag).copied().unwrap_or(0),
    };
    match notify(bus, notice, replaces, entry.is_some()) {
        Ok(raised) => {
            if !notice.tag.is_empty() {
                standing.insert(notice.tag.clone(), raised);
            }
            let mut clicks = clicks().lock().expect("banner clicks");
            match entry {
                Some(entry) => clicks.insert(raised, entry),
                None => clicks.remove(&raised),
            };
            true
        }
        Err(error) => {
            tracing::debug!(
                target: "ducktape::app",
                reason = "notice_refused",
                %error,
                "the notification daemon refused a desktop notice"
            );
            false
        }
    }
}

/// The banner standing under `tag`, closed (`CloseNotification`).
pub(super) fn withdraw(tag: &str) {
    let Some(banner) = standing().lock().expect("standing notices").remove(tag) else {
        return;
    };
    clicks().lock().expect("banner clicks").remove(&banner);
    let Some(bus) = session_bus() else {
        return;
    };
    let closed = bus.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "CloseNotification",
        &(banner,),
    );
    if let Err(error) = closed {
        tracing::debug!(target: "ducktape::app", %error, "a standing banner was not closed");
    }
}

/// One `Notify` request: `(app_name, replaces_id, app_icon, summary,
/// body, actions, hints, expire_timeout)` → the banner's id. A banner
/// that opens something declares the default action, a click on it.
pub(super) fn notify(
    bus: &zbus::blocking::Connection,
    notice: &Notice,
    replaces: u32,
    clickable: bool,
) -> zbus::Result<u32> {
    let actions = match clickable {
        true => vec!["default", "Open"],
        false => Vec::new(),
    };
    let body = markup_escaped(&notice.body);
    let hints: HashMap<&str, zbus::zvariant::Value<'_>> =
        HashMap::from([("desktop-entry", zbus::zvariant::Value::from(DESKTOP_ENTRY))]);
    let reply = bus.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Ducktape",
            replaces,
            "",
            notice.title.as_str(),
            body.as_str(),
            actions,
            hints,
            -1i32,
        ),
    )?;
    reply.body().deserialize::<u32>()
}
