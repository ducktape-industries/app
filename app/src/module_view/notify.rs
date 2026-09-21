//! `notify.show` — one desktop notice, worded entirely by the view. The
//! host owns the platform notifier and nothing above it: it does not decide
//! what is worth a banner, does not group, does not rate-limit by content.
//! It honours one device-global preference, `desktop_notifications`, which
//! is a property of the machine the way the appearance is, and posts.
//!
//! No consent prompt: the platform raises its own where it has one (macOS),
//! and the freedesktop service needs none.
//!
//! WHAT IS NOT HERE: a click on the banner is not delivered back to the
//! view. The freedesktop `ActionInvoked` signal needs declared actions and a
//! standing bus listener, and the macOS side needs a notification-centre
//! delegate installed on the app object, which this process does not own.
//! Neither is a small addition, and a half-wired click that works on one
//! desktop would be worse than none — so a view treats a notice as fire and
//! forget.

use super::kernel::spawn_device;
use super::{Guest, wire};
use crate::backend::read_prefs;

/// The prefs key, device-global like `appearance`.
const NOTIFY_PREF: &str = "desktop_notifications";

/// The longest title and body the host will put on a screen. A notice is a
/// sentence, not a payload.
const MAX_TEXT: usize = 512;

/// Whether this machine may raise a banner at all. Default ON — only an
/// explicit `false` turns it off.
fn enabled() -> bool {
    read_prefs()[NOTIFY_PREF].as_bool().unwrap_or(true)
}

/// One notice, already worded. `tag` groups a notice with the ones before
/// it: a later notice under the same tag REPLACES the standing one rather
/// than stacking, which is the only reason the host keeps any state here.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Notice {
    title: String,
    body: String,
    tag: String,
}

impl Notice {
    fn shortened(mut self) -> Result<Self, &'static str> {
        if self.title.is_empty() && self.body.is_empty() {
            return Err("`notify.show` carries neither a title nor a body");
        }
        for text in [&mut self.title, &mut self.body, &mut self.tag] {
            if text.len() > MAX_TEXT {
                let cut = (0..=MAX_TEXT)
                    .rev()
                    .find(|end| text.is_char_boundary(*end))
                    .unwrap_or(0);
                text.truncate(cut);
            }
        }
        Ok(self)
    }
}

pub(super) fn answer(
    guest: &mut Guest,
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        ("notify", "show") => {
            let notice = serde_json::from_slice::<Notice>(payload)
                .map_err(|error| error.to_string())
                .and_then(|notice| notice.shortened().map_err(str::to_owned));
            let notice = match notice {
                Ok(notice) => notice,
                Err(error) => {
                    guest.refuse(id, "malformed_request", error);
                    return true;
                }
            };
            spawn_device(guest, id, async move {
                let shown = match enabled() {
                    false => false,
                    true => tokio::task::spawn_blocking(move || platform::post(&notice))
                        .await
                        .map_err(|error| wire::Refusal::new("host_fault", error.to_string()))?,
                };
                Ok(format!("{{\"shown\":{shown}}}").into_bytes())
            });
        }
        _ => return false,
    }
    true
}

/// macOS raises banners through the user-notification centre, which
/// TERMINATES a process that has no bundle identifier — not an error a
/// caller can catch, the process dies. `cargo test`, `cargo run` and any
/// bare binary are exactly that process, so the identifier is read first.
#[cfg(target_os = "macos")]
mod platform {
    use super::Notice;
    use objc2::rc::Retained;
    use objc2_foundation::{NSBundle, NSString};
    use objc2_user_notifications::{
        UNMutableNotificationContent, UNNotificationRequest, UNNotificationSound,
        UNUserNotificationCenter,
    };

    fn bundled() -> bool {
        // SAFETY: reading the main bundle's identifier is valid on any thread.
        unsafe { NSBundle::mainBundle().bundleIdentifier().is_some() }
    }

    pub(super) fn post(notice: &Notice) -> bool {
        if !bundled() {
            tracing::debug!(
                target: "ducktape::app",
                reason = "no_bundle_identifier",
                "skipped a desktop notice: this process is not an app bundle"
            );
            return false;
        }
        // SAFETY: plain framework work on objects this function owns; the
        // centre is thread-safe by contract and copies what it is handed.
        unsafe {
            let centre = UNUserNotificationCenter::currentNotificationCenter();
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(&notice.title));
            content.setBody(&NSString::from_str(&notice.body));
            if !notice.tag.is_empty() {
                content.setThreadIdentifier(&NSString::from_str(&notice.tag));
            }
            content.setSound(Some(&UNNotificationSound::defaultSound()));
            // A tag REPLACES: the platform keys a standing notice by this
            // identifier, so a fresh one per untagged notice adds instead.
            let identifier = match notice.tag.is_empty() {
                true => format!("ducktape-{}", fresh()),
                false => format!("ducktape-tag-{}", notice.tag),
            };
            let request: Retained<UNNotificationRequest> =
                UNNotificationRequest::requestWithIdentifier_content_trigger(
                    &NSString::from_str(&identifier),
                    &content,
                    None,
                );
            centre.addNotificationRequest_withCompletionHandler(&request, None);
        }
        true
    }

    fn fresh() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }
}

/// Everywhere else the notifier is the freedesktop notifications service,
/// which every desktop off macOS answers through its own daemon. Pure Rust,
/// and no new dependency — zbus is already in this binary's graph.
#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Notice;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// The desktop entry the banner is filed under (`app/packaging`), which
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

    pub(super) fn post(notice: &Notice) -> bool {
        let Some(bus) = session_bus() else {
            return false;
        };
        let replaces = match notice.tag.is_empty() {
            true => 0,
            false => standing()
                .lock()
                .expect("standing notices")
                .get(&notice.tag)
                .copied()
                .unwrap_or(0),
        };
        match notify(bus, notice, replaces) {
            Ok(raised) => {
                if !notice.tag.is_empty() {
                    standing()
                        .lock()
                        .expect("standing notices")
                        .insert(notice.tag.clone(), raised);
                }
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

    /// One `Notify` request: `(app_name, replaces_id, app_icon, summary,
    /// body, actions, hints, expire_timeout)` → the banner's id. No actions,
    /// because a click is not delivered back to the view.
    pub(super) fn notify(
        bus: &zbus::blocking::Connection,
        notice: &Notice,
        replaces: u32,
    ) -> zbus::Result<u32> {
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
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )?;
        reply.body().deserialize::<u32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The door takes a title, a body and an optional tag, and nothing else:
    /// a field this door does not have is refused rather than ignored, and a
    /// notice with no words at all is not a notice.
    #[test]
    fn a_notice_is_two_strings_and_an_optional_tag() {
        let notice: Notice = serde_json::from_str(r#"{"title":"a","body":"b","tag":"t"}"#).unwrap();
        assert_eq!((notice.title.as_str(), notice.tag.as_str()), ("a", "t"));
        let untagged: Notice = serde_json::from_str(r#"{"title":"a","body":"b"}"#).unwrap();
        assert!(untagged.tag.is_empty());
        assert!(serde_json::from_str::<Notice>(r#"{"title":"a","icon":"x"}"#).is_err());
        assert!(Notice::default().shortened().is_err());
        assert!(
            serde_json::from_str::<Notice>(r#"{"body":"b"}"#)
                .unwrap()
                .shortened()
                .is_ok()
        );
    }

    /// A view cannot put a megabyte on the person's screen, and the cut
    /// lands on a character boundary rather than splitting one.
    #[test]
    fn a_notice_is_shortened_onto_a_character_boundary() {
        let long = Notice {
            title: "가".repeat(MAX_TEXT),
            body: "b".repeat(MAX_TEXT * 2),
            tag: String::new(),
        }
        .shortened()
        .unwrap();
        assert!(long.title.len() <= MAX_TEXT);
        assert!(long.title.chars().all(|letter| letter == '가'));
        assert_eq!(long.body.len(), MAX_TEXT);
    }

    /// The machine's own preference is the only gate, and it is ON unless
    /// this device explicitly turned it off.
    #[test]
    fn the_device_preference_defaults_to_on() {
        let reading = |prefs: serde_json::Value| prefs[NOTIFY_PREF].as_bool().unwrap_or(true);
        assert!(reading(serde_json::json!({})));
        assert!(reading(serde_json::json!({ NOTIFY_PREF: true })));
        assert!(!reading(serde_json::json!({ NOTIFY_PREF: false })));
        assert!(reading(serde_json::json!({ NOTIFY_PREF: "yes" })));
    }

    /// A notice's own markup characters reach the freedesktop body escaped,
    /// so a server that renders markup shows them as the view typed them.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_freedesktop_body_escapes_the_notices_markup() {
        assert_eq!(platform::markup_escaped("a <b> & c"), "a &lt;b&gt; &amp; c");
    }
}
