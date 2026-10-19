//! Banners on every desktop off macOS: the freedesktop notifications
//! service (`org.freedesktop.Notifications` on the session D-Bus), which
//! each desktop answers through its own daemon. Pure Rust, and no new
//! dependency — zbus is already in this binary's graph.

use ducktape_host::runtime::notify::{Click, Notice, NoticeKind, SOUND};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The desktop entry the banner is filed under (`packaging`), which
/// is what gives it this app's icon and lets the desktop group it.
const DESKTOP_ENTRY: &str = "dev.ducktape.app";

/// The sound an ordinary notice plays ([`SOUND`]), carried in the binary:
/// the Linux app is one file, with nothing installed beside it.
const QUACK: &[u8] = include_bytes!("../../assets/sounds/quack.wav");

/// The quack where the daemon can play it (the `sound-file` hint), kept
/// in this user's cache; `None` when it cannot be kept there, and the
/// notice goes up all the same with the daemon's own sound.
fn sound_file() -> Option<String> {
    let kept = crate::backend::cache_dir()
        .map(|cache| cache.join("sounds"))
        .and_then(|dir| {
            kept_sound(&dir, QUACK).map_err(|error| format!("{}: {error}", dir.display()))
        });
    match kept {
        Ok(file) => file.to_str().map(str::to_owned),
        Err(error) => {
            tracing::debug!(
                target: "ducktape::app",
                reason = "no_sound_file",
                %error,
                "a desktop notice goes without its quack"
            );
            None
        }
    }
}

/// `bytes` as [`SOUND`] in `dir`: left as it is when those bytes are there
/// already, else written beside it and renamed into place, so none of this
/// user's other processes plays it half written. Read at each notice, so
/// a cache cleared under a running app is mended at its next one.
fn kept_sound(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let file = dir.join(SOUND);
    if std::fs::read(&file).is_ok_and(|kept| kept == bytes) {
        return Ok(file);
    }
    std::fs::create_dir_all(dir)?;
    let part = dir.join(format!("{SOUND}.{}", std::process::id()));
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, &file)?;
    Ok(file)
}

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

/// The banner a chain's tag is standing under, so the next notice under
/// that tag replaces it instead of stacking a second one: two networks'
/// programs may share a tag, and each stands.
fn standing() -> &'static Mutex<HashMap<(String, String), u32>> {
    static STANDING: OnceLock<Mutex<HashMap<(String, String), u32>>> = OnceLock::new();
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

/// What a click on each clickable banner opens, by banner id.
fn clicks() -> &'static Mutex<HashMap<u32, Click>> {
    static CLICKS: OnceLock<Mutex<HashMap<u32, Click>>> = OnceLock::new();
    CLICKS.get_or_init(Mutex::default)
}

/// One thread for the life of the process, hearing the daemon say a
/// banner was clicked: what it names is opened.
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
                    let click = clicks()
                        .lock()
                        .expect("banner clicks")
                        .get(&banner)
                        .cloned();
                    match (click, action.as_str()) {
                        // a call's Answer opens it as a click does
                        (Some(click), "default" | "answer") => super::clicked(click),
                        // a call's Decline is this device's alone (SPEC D11)
                        (Some(_), "decline") => declined(banner),
                        _ => {}
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::info!(target: "ducktape::app", %error, "banner clicks are not heard");
        }
    });
}

pub(super) fn post(notice: &Notice, click: Option<Click>) -> bool {
    let Some(bus) = session_bus() else {
        return false;
    };
    if click.is_some() {
        listen(bus);
    }
    // held across the call: two notices under one tag posted at once
    // would otherwise both read "none standing" and stack
    let mut standing = standing().lock().expect("standing notices");
    // the host's notice (a click opens Updates) replaces the one any of
    // this session's processes left standing
    let host = click == Some(Click::Updates);
    let replaces = match host {
        true => host_banner().unwrap_or(0),
        false => replaced(&standing, notice),
    };
    match notify(bus, notice, replaces, click.is_some()) {
        Ok(raised) => {
            if host {
                keep_host_banner(Some(raised));
            } else if !notice.tag.is_empty() {
                standing.insert((notice.chain.clone(), notice.tag.clone()), raised);
            }
            let mut clicks = clicks().lock().expect("banner clicks");
            match click {
                Some(click) => clicks.insert(raised, click),
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

/// The banner `notice` replaces: the one standing under its chain's tag;
/// none (0) with no tag.
fn replaced(standing: &HashMap<(String, String), u32>, notice: &Notice) -> u32 {
    let under = (notice.chain.clone(), notice.tag.clone());
    match notice.tag.is_empty() {
        true => 0,
        false => standing.get(&under).copied().unwrap_or(0),
    }
}

/// A call's banner declined: no longer standing, and closed (a daemon
/// that keeps a banner after its action would leave it up); the centre
/// holding its chain's log hears it (the one in hand, or that network's
/// followed set).
fn declined(banner: u32) {
    let under = standing()
        .lock()
        .expect("standing notices")
        .iter()
        .find(|(_, raised)| **raised == banner)
        .map(|(under, _)| under.clone());
    if let Some((chain, tag)) = under {
        withdraw(&chain, &tag);
        ducktape_host::runtime::notify::declined(&chain, &tag);
    }
}

/// The banner standing under `tag` on `chain`, closed (`CloseNotification`).
pub(super) fn withdraw(chain: &str, tag: &str) {
    let standing = standing()
        .lock()
        .expect("standing notices")
        .remove(&(chain.to_owned(), tag.to_owned()));
    let Some(banner) = standing else {
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

/// The host's notice down, whichever of this session's processes raised it.
pub(super) fn withdraw_host() {
    let Some(banner) = host_banner() else {
        return;
    };
    keep_host_banner(None);
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
        tracing::debug!(target: "ducktape::app", %error, "the host's banner was not closed");
    }
}

/// Where the host's banner id is kept for the session's other processes:
/// `<boot id> <banner id>` beside the app's log (whose dir `main` makes at
/// its start). The daemon numbers banners per boot, so an id from another
/// boot is none of ours.
// ponytail: a daemon restarted within one boot numbers afresh, and a kept
// id may then name another app's banner; the boot id guards a reboot only.
fn host_banner_path() -> Option<std::path::PathBuf> {
    let log = crate::backend::app_log_path().ok()?;
    Some(log.with_file_name("update-notice"))
}

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The host's banner standing in this boot, if one is.
fn host_banner() -> Option<u32> {
    let text = std::fs::read_to_string(host_banner_path()?).ok()?;
    let (boot, banner) = text.trim().split_once(' ')?;
    (boot == boot_id()).then(|| banner.parse().ok()).flatten()
}

fn keep_host_banner(banner: Option<u32>) {
    let Some(path) = host_banner_path() else {
        return;
    };
    let _ = match banner {
        Some(banner) => std::fs::write(&path, format!("{} {banner}\n", boot_id())),
        None => std::fs::remove_file(&path),
    };
}

/// What a `Notify` request carries besides its words: the actions, the
/// hints, and how long it stays. A banner that opens something declares
/// the default action, a click on it. A call's has Answer and Decline,
/// rings with the theme's incoming-call sound and stays until the host
/// takes it down (its `until`, or the view's `notify.seen`). Any other
/// plays `sound`, the quack's file ([`sound_file`]), when there is one: the
/// daemon decides whether it sounds (its do-not-disturb, its settings).
fn request(
    notice: &Notice,
    clickable: bool,
    sound: Option<String>,
) -> (
    Vec<&'static str>,
    HashMap<&'static str, zbus::zvariant::Value<'static>>,
    i32,
) {
    let mut hints = HashMap::from([("desktop-entry", zbus::zvariant::Value::from(DESKTOP_ENTRY))]);
    let call = notice.kind == NoticeKind::Call;
    if call {
        // critical: the daemon keeps it up, over a do-not-disturb
        hints.insert("urgency", zbus::zvariant::Value::U8(2));
        hints.insert("category", zbus::zvariant::Value::from("call.incoming"));
        hints.insert(
            "sound-name",
            zbus::zvariant::Value::from("phone-incoming-call"),
        );
    } else if let Some(sound) = sound {
        hints.insert("sound-file", zbus::zvariant::Value::from(sound));
    }
    let actions = match (clickable, call) {
        (true, true) => vec![
            "default", "Answer", "answer", "Answer", "decline", "Decline",
        ],
        (true, false) => vec!["default", "Open"],
        (false, _) => Vec::new(),
    };
    (actions, hints, if call { 0 } else { -1 })
}

/// One `Notify` request: `(app_name, replaces_id, app_icon, summary,
/// body, actions, hints, expire_timeout)` → the banner's id.
fn notify(
    bus: &zbus::blocking::Connection,
    notice: &Notice,
    replaces: u32,
    clickable: bool,
) -> zbus::Result<u32> {
    let sound = (notice.kind == NoticeKind::Notice)
        .then(sound_file)
        .flatten();
    let (actions, hints, timeout) = request(notice, clickable, sound);
    let body = markup_escaped(&notice.body);
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
            timeout,
        ),
    )?;
    reply.body().deserialize::<u32>()
}

#[cfg(test)]
/// A notice's own markup characters reach the freedesktop body escaped,
/// so a server that renders markup shows them as the view typed them.
#[test]
fn the_freedesktop_body_escapes_the_notices_markup() {
    assert_eq!(markup_escaped("a <b> & c"), "a &lt;b&gt; &amp; c");
}

#[cfg(test)]
/// A call's banner offers Answer and Decline, rings as an incoming call and
/// stays; a plain notice's opens on a click and expires as the daemon says.
#[test]
fn a_calls_banner_answers_declines_and_stays() {
    let call = Notice {
        title: "Ada is calling".into(),
        kind: NoticeKind::Call,
        until: 1,
        ..Notice::default()
    };
    let quack = Some("/run/quack.wav".to_owned());
    let (actions, hints, timeout) = request(&call, true, quack);
    assert_eq!(
        actions,
        [
            "default", "Answer", "answer", "Answer", "decline", "Decline"
        ]
    );
    assert_eq!(
        hints["category"],
        zbus::zvariant::Value::from("call.incoming")
    );
    assert_eq!(hints["urgency"], zbus::zvariant::Value::U8(2));
    assert_eq!(timeout, 0);
    assert!(!hints.contains_key("sound-file"), "a call quacked");
    let plain = Notice::default();
    let (actions, hints, timeout) = request(&plain, true, None);
    assert_eq!(
        (actions, hints.len(), timeout),
        (vec!["default", "Open"], 1, -1)
    );
    assert!(request(&plain, false, None).0.is_empty());
}

#[cfg(test)]
/// A notice's banner plays the quack's file; a call's rings as it did and
/// does not quack.
#[test]
fn a_notice_quacks_and_a_call_keeps_its_ring() {
    let quack = "/home/ada/.cache/ducktape/sounds/quack.wav".to_owned();
    let (_, hints, _) = request(&Notice::default(), false, Some(quack.clone()));
    assert_eq!(
        hints["sound-file"],
        zbus::zvariant::Value::from(quack.clone())
    );
    assert!(!hints.contains_key("sound-name"));
    let call = Notice {
        kind: NoticeKind::Call,
        ..Notice::default()
    };
    let (_, hints, _) = request(&call, false, Some(quack));
    let ring = zbus::zvariant::Value::from("phone-incoming-call");
    assert_eq!(hints["sound-name"], ring);
    assert!(!hints.contains_key("sound-file"));
}

#[cfg(test)]
/// The quack is kept once in its directory, written again over other
/// bytes, and leaves no part file behind.
#[test]
fn the_quack_is_kept_once_and_mended() {
    let dir = tempfile::tempdir().unwrap();
    let sounds = dir.path().join("sounds");
    let file = kept_sound(&sounds, QUACK).unwrap();
    assert_eq!(file, sounds.join(SOUND));
    assert_eq!(std::fs::read(&file).unwrap(), QUACK);
    std::fs::write(&file, b"stale").unwrap();
    assert_eq!(kept_sound(&sounds, QUACK).unwrap(), file);
    assert_eq!(std::fs::read(&file).unwrap(), QUACK);
    let names: Vec<String> = std::fs::read_dir(&sounds)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [SOUND]);
}

#[cfg(test)]
/// The quack is a sound every notifier plays: RIFF PCM, 16-bit, mono, one
/// short quack of a second at most.
#[test]
fn the_quack_is_a_short_pcm_wave() {
    let word = |at: usize| u16::from_le_bytes([QUACK[at], QUACK[at + 1]]);
    let long =
        |at: usize| u32::from_le_bytes([QUACK[at], QUACK[at + 1], QUACK[at + 2], QUACK[at + 3]]);
    assert_eq!(&QUACK[..4], b"RIFF");
    assert_eq!(&QUACK[8..16], b"WAVEfmt ");
    assert_eq!(&QUACK[36..40], b"data");
    // PCM, one channel, 16 bits
    assert_eq!((word(20), word(22), word(34)), (1, 1, 16));
    let (rate, bytes) = (long(24), long(40));
    assert_eq!(QUACK.len(), 44 + bytes as usize);
    assert!(
        bytes <= rate * 2,
        "{bytes} bytes at {rate} Hz: over a second"
    );
}

#[cfg(test)]
/// Two networks' programs raise one tag (garage's and duckpond's
/// `chat/#general`): each banner stands, and a later one replaces only its
/// own chain's.
#[test]
fn one_tag_on_two_chains_stands_twice() {
    let on = |chain: &str| Notice {
        title: "ada mentioned you".into(),
        tag: "chat/#general".into(),
        chain: chain.into(),
        ..Notice::default()
    };
    let standing = HashMap::from([(("garage#01".into(), "chat/#general".into()), 5)]);
    assert_eq!(
        replaced(&standing, &on("duckpond#02")),
        0,
        "garage's was replaced"
    );
    assert_eq!(replaced(&standing, &on("garage#01")), 5);
    let untagged = Notice {
        tag: String::new(),
        ..on("garage#01")
    };
    assert_eq!(replaced(&standing, &untagged), 0);
}
