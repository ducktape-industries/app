//! Desktop notices. VIEWS ASK, THE HOST DECIDES: a view hands the host a
//! notice (`notify.post`, or the older `notify.show`) and the host records
//! it in its notification centre (per network, on this device) and decides
//! whether a banner reaches the screen:
//!
//! - the person's word on the view: none yet → logged, and the view's
//!   window asks them (the permission bar); Block → dropped, not even
//!   logged; Silent → logged only; Allow → on to the rest;
//! - banners off for the device (`desktop_notifications`) → logged only;
//! - the view is the window the person is looking at → logged only, unless
//!   they asked to see banners in front too;
//! - a token bucket per view (the burst limit a minute): past it, one
//!   standing banner per view says how many more are waiting.
//!
//! A non-empty `tag` replaces the view's standing banner under it, and
//! folds the centre's rows under it into one with a count.
//!
//! A click on a banner opens its row, the way the centre's row does, where
//! the desktop says so: freedesktop's `ActionInvoked` on the banner's
//! default action. macOS would need a notification-centre delegate on an
//! app object this process does not own, so there a banner is fire and
//! forget and the centre's row is the way to a notice's link.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use super::kernel::spawn_device;
use super::wire::doors::{self, Notice, Post, Posted};
use super::{Guest, Intent, wire};
use crate::backend::{read_prefs, write_prefs};
use crate::shell::WindowKey;

/// The prefs keys, device-global like `appearance`.
const NOTIFY_PREF: &str = "desktop_notifications";
const FRONT_PREF: &str = "notify_in_front";
const BURST_PREF: &str = "notify_burst";
const VIEWS_PREF: &str = "notify_views";

/// The burst limits a person picks from, banners a minute per view.
pub(crate) const BURSTS: [u32; 3] = [3, 6, 12];
const DEFAULT_BURST: u32 = 6;

/// The longest title, body, tag and link the host keeps. A notice is a
/// sentence, not a payload.
const MAX_TEXT: usize = 512;
/// The centre keeps this many rows, and none older than this.
const MAX_ENTRIES: usize = 500;
const MAX_AGE: i64 = 30 * 86_400;

/// The person's word on one view's notices; no word yet is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Permission {
    Allow,
    Silent,
    Block,
}

impl Permission {
    pub(crate) const ALL: [Self; 3] = [Self::Allow, Self::Silent, Self::Block];

    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::Silent => "Silent",
            Self::Block => "Block",
        }
    }

    fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.word() == word)
    }
}

/// This device's notification settings, as the prefs file holds them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Banners at all. Default on: only an explicit `false` turns it off.
    pub(crate) banners: bool,
    /// Banners for the window in front, too. Default off.
    pub(crate) in_front: bool,
    /// Banners a minute, per view.
    pub(crate) burst: u32,
    pub(crate) views: BTreeMap<String, Permission>,
}

impl Settings {
    pub(crate) fn load() -> Self {
        Self::of(&read_prefs())
    }

    fn of(prefs: &serde_json::Value) -> Self {
        let burst = prefs[BURST_PREF]
            .as_u64()
            .and_then(|burst| BURSTS.into_iter().find(|it| u64::from(*it) == burst))
            .unwrap_or(DEFAULT_BURST);
        let views = prefs[VIEWS_PREF]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(module, word)| Some((module.clone(), Permission::of(word.as_str()?)?)))
            .collect();
        Self {
            banners: prefs[NOTIFY_PREF].as_bool().unwrap_or(true),
            in_front: prefs[FRONT_PREF].as_str() == Some("show"),
            burst,
            views,
        }
    }
}

fn edit_prefs(change: impl FnOnce(&mut serde_json::Value)) {
    let mut prefs = read_prefs();
    change(&mut prefs);
    write_prefs(&prefs);
}

pub(crate) fn save_banners(on: bool) {
    edit_prefs(|prefs| prefs[NOTIFY_PREF] = serde_json::json!(on));
}

pub(crate) fn save_in_front(show: bool) {
    edit_prefs(|prefs| prefs[FRONT_PREF] = serde_json::json!(if show { "show" } else { "hide" }));
}

pub(crate) fn save_burst(burst: u32) {
    edit_prefs(|prefs| prefs[BURST_PREF] = serde_json::json!(burst));
}

/// The person answered a view: its bar goes, and the word is kept.
pub(crate) fn set_permission(module: &str, permission: Permission) {
    edit_prefs(|prefs| {
        if !prefs[VIEWS_PREF].is_object() {
            prefs[VIEWS_PREF] = serde_json::json!({});
        }
        prefs[VIEWS_PREF][module] = serde_json::json!(permission.word());
    });
    center().asking.remove(module);
}

/// "Not now": the bar goes for this run; the view's notices stay logged
/// silently, and the bar asks again next launch.
pub(crate) fn not_now(module: &str) {
    let mut center = center();
    center.asking.remove(module);
    center.not_now.insert(module.to_owned());
}

/// One row of the centre.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: u64,
    /// The view's program, and its name when it posted.
    pub(crate) module: String,
    pub(crate) view: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) tag: String,
    pub(crate) link: String,
    /// Unix seconds of the latest notice folded into the row.
    pub(crate) at: i64,
    /// Notices folded into the row under its tag.
    pub(crate) count: u32,
    pub(crate) read: bool,
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

/// The notification centre: the log of the network in hand, and what the
/// policy remembers between notices.
#[derive(Default)]
pub(crate) struct Center {
    /// The chain the log is for; none, and nothing is written to disk.
    network: String,
    entries: Vec<Entry>,
    next: u64,
    buckets: HashMap<String, Bucket>,
    /// Notices past the burst limit since the view's last banner.
    more: HashMap<String, u32>,
    /// The window in front and the view focused in it.
    front: Option<(WindowKey, &'static str)>,
    /// Views that posted before the person said anything about them.
    asking: BTreeSet<String>,
    not_now: BTreeSet<String>,
}

pub(crate) fn center() -> MutexGuard<'static, Center> {
    static CENTER: OnceLock<Mutex<Center>> = OnceLock::new();
    CENTER
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Unix seconds now.
pub(crate) fn wall() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

impl Center {
    /// What to do with one notice from `module` (named `view`): the answer
    /// for the view, and the banner to raise, if any — the notice's own, or
    /// the view's "N more" once it is past its burst.
    pub(crate) fn post(
        &mut self,
        settings: &Settings,
        module: &str,
        view: &str,
        post: Post,
        now: Instant,
        wall: i64,
    ) -> (Posted, Option<Notice>) {
        let permission = settings.views.get(module).copied();
        if permission == Some(Permission::Block) {
            return (Posted::Blocked, None);
        }
        // a view's tags are its own: two views never replace each other's
        let notice = Notice {
            title: post.title.clone(),
            body: post.body.clone(),
            tag: match post.tag.is_empty() {
                true => String::new(),
                false => format!("{module}/{}", post.tag),
            },
        };
        self.log(module, view, post, wall);
        match permission {
            None => {
                if !self.not_now.contains(module) {
                    self.asking.insert(module.to_owned());
                }
                return (Posted::Logged, None);
            }
            Some(Permission::Silent) => return (Posted::Logged, None),
            _ => {}
        }
        let in_front = self.front.is_some_and(|(_, focused)| focused == module);
        if !settings.banners || (in_front && !settings.in_front) {
            return (Posted::Logged, None);
        }
        let burst = f64::from(settings.burst.max(1));
        let bucket = self.buckets.entry(module.to_owned()).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        let refill = now.saturating_duration_since(bucket.at).as_secs_f64() * burst / 60.;
        bucket.tokens = (bucket.tokens + refill).min(burst);
        bucket.at = now;
        if bucket.tokens >= 1. {
            bucket.tokens -= 1.;
            self.more.remove(module);
            return (Posted::Banner, Some(notice));
        }
        let more = self.more.entry(module.to_owned()).or_default();
        *more += 1;
        let more = Notice {
            title: format!("{more} more from {view}"),
            body: "They're waiting in Notifications.".into(),
            tag: format!("more/{module}"),
        };
        (Posted::Logged, Some(more))
    }

    /// Into the log: a notice under a tag the view already has a row for
    /// folds into that row and brings it back to the top, unread.
    fn log(&mut self, module: &str, view: &str, post: Post, wall: i64) {
        let folded = (!post.tag.is_empty())
            .then(|| {
                self.entries
                    .iter()
                    .position(|entry| entry.module == module && entry.tag == post.tag)
            })
            .flatten();
        let entry = match folded {
            Some(index) => {
                let mut entry = self.entries.remove(index);
                entry.count += 1;
                entry.read = false;
                entry.title = post.title;
                entry.body = post.body;
                entry.link = post.link;
                entry.view = view.to_owned();
                entry.at = wall;
                entry
            }
            None => {
                self.next += 1;
                Entry {
                    id: self.next,
                    module: module.to_owned(),
                    view: view.to_owned(),
                    title: post.title,
                    body: post.body,
                    tag: post.tag,
                    link: post.link,
                    at: wall,
                    count: 1,
                    read: false,
                }
            }
        };
        self.entries.push(entry);
        self.prune(wall);
        self.save();
    }

    /// At most [`MAX_ENTRIES`] rows, none older than [`MAX_AGE`].
    fn prune(&mut self, wall: i64) {
        self.entries.retain(|entry| wall - entry.at <= MAX_AGE);
        let over = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..over);
    }

    /// Newest first.
    pub(crate) fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().rev()
    }

    pub(crate) fn unread(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.read).count()
    }

    /// A row picked: read now, and handed back to open.
    pub(crate) fn open(&mut self, id: u64) -> Option<Entry> {
        let entry = self.entries.iter_mut().find(|entry| entry.id == id)?;
        entry.read = true;
        let entry = entry.clone();
        self.save();
        Some(entry)
    }

    pub(crate) fn mark_all_read(&mut self) {
        for entry in &mut self.entries {
            entry.read = true;
        }
        self.save();
    }

    pub(crate) fn clear_read(&mut self) {
        self.entries.retain(|entry| !entry.read);
        self.save();
    }

    /// Whether `module`'s window shows the permission bar.
    pub(crate) fn asking(&self, module: &str) -> bool {
        self.asking.contains(module)
    }

    /// Notices from `module` this past week, folded ones counted.
    pub(crate) fn this_week(&self, module: &str, wall: i64) -> u32 {
        self.entries
            .iter()
            .filter(|entry| entry.module == module && wall - entry.at <= 7 * 86_400)
            .map(|entry| entry.count)
            .sum()
    }

    /// A window drew: whether it is in front, and the view focused in it.
    pub(crate) fn set_front(&mut self, window: WindowKey, active: bool, focused: &'static str) {
        match active {
            true => self.front = Some((window, focused)),
            false if self.front.is_some_and(|(key, _)| key == window) => self.front = None,
            false => {}
        }
    }

    /// The network in hand: its log comes off disk, the policy starts over.
    pub(crate) fn set_network(&mut self, network: &str) {
        if self.network == network {
            return;
        }
        *self = Center {
            network: network.to_owned(),
            front: self.front,
            ..Center::default()
        };
        self.entries = log_path(network)
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        self.next = self.entries.iter().map(|entry| entry.id).max().unwrap_or(0);
        self.prune(wall());
    }

    // ponytail: the whole log rewritten on each change, 500 small rows at most
    fn save(&self) {
        let Some(path) = log_path(&self.network) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(&self.entries) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

/// `<config>/notifications/<chain>.json`, device-local beside the prefs.
fn log_path(network: &str) -> Option<PathBuf> {
    if network.is_empty() {
        return None;
    }
    let file: String = network
        .chars()
        .map(
            |letter| match letter.is_ascii_alphanumeric() || "-_.".contains(letter) {
                true => letter,
                false => '_',
            },
        )
        .collect();
    let dir = crate::backend::config_dir().ok()?.join("notifications");
    Some(dir.join(format!("{file}.json")))
}

/// Unix seconds at the start of the local day `wall` falls in.
pub(crate) fn local_midnight(wall: i64) -> i64 {
    let offset = local_offset(wall);
    (wall + offset).div_euclid(86_400) * 86_400 - offset
}

fn local_offset(wall: i64) -> i64 {
    let time = wall as libc::time_t;
    // SAFETY: `localtime_r` writes only the `tm` it is handed.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        match libc::localtime_r(&time, &mut tm).is_null() {
            true => 0,
            false => tm.tm_gmtoff as i64,
        }
    }
}

/// "now", "14m", "2h", "Yesterday", "3d".
pub(crate) fn ago(at: i64, now: i64) -> String {
    let seconds = (now - at).max(0);
    match seconds {
        ..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        _ if at >= local_midnight(now) => format!("{}h", seconds / 3_600),
        _ if at >= local_midnight(now) - 86_400 => "Yesterday".into(),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Shortened to what a screen shows, on a character boundary; a notice
/// with no words is not one, and a link is a `duck://` link or nothing.
fn shortened(mut post: Post) -> Result<Post, &'static str> {
    if post.title.is_empty() && post.body.is_empty() {
        return Err("a notice carries neither a title nor a body");
    }
    if !post.link.is_empty() && !post.link.starts_with("duck://") {
        return Err("a notice's link is a duck:// link");
    }
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
    // a cut link goes nowhere
    if post.link.len() == MAX_TEXT {
        post.link.clear();
    }
    Ok(post)
}

pub(super) fn answer(
    guest: &mut Guest,
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    let post = match (capability, operation) {
        ("notify", "post") => doors::decode::<Post>(payload),
        // the older door: a post with no link, answered with a bool
        ("notify", "show") => doors::decode::<Notice>(payload).map(|notice| Post {
            title: notice.title,
            body: notice.body,
            tag: notice.tag,
            link: String::new(),
        }),
        _ => return false,
    };
    let post = match post.and_then(|post| shortened(post).map_err(str::to_owned)) {
        Ok(post) => post,
        Err(error) => {
            guest.refuse(id, "malformed_request", error);
            return true;
        }
    };
    let (posted, banner, entry) = {
        let mut center = center();
        let (posted, banner) = center.post(
            &Settings::load(),
            guest.module,
            &guest.name,
            post,
            Instant::now(),
            wall(),
        );
        // the row just logged is the newest: a click on its banner opens it
        let entry = (posted == Posted::Banner)
            .then(|| center.entries().next().map(|entry| entry.id))
            .flatten();
        (posted, banner, entry)
    };
    // the bell and the bar redraw
    guest.intents.push(Intent::Notified);
    let show = operation == "show";
    spawn_device(guest, id, async move {
        let raised = match banner {
            None => false,
            Some(notice) => tokio::task::spawn_blocking(move || platform::post(&notice, entry))
                .await
                .map_err(|error| wire::Refusal::new("host_fault", error.to_string()))?,
        };
        let posted = match posted {
            Posted::Banner if !raised => Posted::Logged,
            posted => posted,
        };
        Ok(match show {
            true => doors::encode(&(posted == Posted::Banner)),
            false => doors::encode(&posted),
        })
    });
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

    pub(super) fn post(notice: &Notice, _entry: Option<u64>) -> bool {
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
            let messages = rule.and_then(|rule| {
                zbus::blocking::MessageIterator::for_match_rule(rule, bus, Some(64))
            });
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
                        let Ok((banner, action)) = message.body().deserialize::<(u32, String)>()
                        else {
                            continue;
                        };
                        let entry = clicks()
                            .lock()
                            .expect("banner clicks")
                            .get(&banner)
                            .copied();
                        if action != "default" {
                            continue;
                        }
                        let Some(entry) = entry.and_then(|entry| super::center().open(entry))
                        else {
                            continue;
                        };
                        let link = match entry.link.is_empty() {
                            true => format!("duck://{}", entry.module),
                            false => entry.link,
                        };
                        crate::shell::open_link(link);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn post(title: &str, tag: &str) -> Post {
        Post {
            title: title.into(),
            body: "b".into(),
            tag: tag.into(),
            link: String::new(),
        }
    }

    fn settings(permission: Option<Permission>) -> Settings {
        Settings {
            banners: true,
            in_front: false,
            burst: 3,
            views: permission
                .map(|permission| ("chat".to_owned(), permission))
                .into_iter()
                .collect(),
        }
    }

    /// The person's word decides first: none yet logs and asks, Block drops
    /// without a trace, Silent logs, Allow raises a banner.
    #[test]
    fn a_views_permission_decides_log_bar_or_banner() {
        let now = Instant::now();
        let mut center = Center::default();
        let (posted, banner) = center.post(&settings(None), "chat", "Chat", post("a", ""), now, 0);
        assert_eq!((posted, banner.is_none()), (Posted::Logged, true));
        assert!(center.asking("chat") && center.entries().count() == 1);

        let blocked = settings(Some(Permission::Block));
        let (posted, banner) = center.post(&blocked, "chat", "Chat", post("b", ""), now, 0);
        assert_eq!((posted, banner), (Posted::Blocked, None));
        assert_eq!(
            center.entries().count(),
            1,
            "a blocked notice is not logged"
        );

        let silent = settings(Some(Permission::Silent));
        let (posted, banner) = center.post(&silent, "chat", "Chat", post("c", ""), now, 0);
        assert_eq!((posted, banner), (Posted::Logged, None));

        let allowed = settings(Some(Permission::Allow));
        let (posted, banner) = center.post(&allowed, "chat", "Chat", post("d", "#room"), now, 0);
        assert_eq!(posted, Posted::Banner);
        assert_eq!(
            banner.unwrap().tag,
            "chat/#room",
            "a view's tags are its own"
        );
        assert_eq!(center.unread(), 3);

        let mut off = allowed.clone();
        off.banners = false;
        assert_eq!(
            center.post(&off, "chat", "Chat", post("e", ""), now, 0).0,
            Posted::Logged
        );

        // "Not now" keeps the bar away for the run
        center.asking.clear();
        center.not_now.insert("chat".into());
        center.post(&settings(None), "chat", "Chat", post("f", ""), now, 0);
        assert!(!center.asking("chat"));
    }

    /// The window in front gets no banner unless the person asked for it.
    #[test]
    fn the_focused_view_is_not_bannered_unless_show_is_set() {
        let now = Instant::now();
        let mut center = Center::default();
        let window = WindowKey::unique();
        let mut allowed = settings(Some(Permission::Allow));
        center.set_front(window, true, "chat");
        assert_eq!(
            center.post(&allowed, "chat", "Chat", post("a", ""), now, 0),
            (Posted::Logged, None)
        );
        allowed.in_front = true;
        assert_eq!(
            center
                .post(&allowed, "chat", "Chat", post("b", ""), now, 0)
                .0,
            Posted::Banner
        );
        allowed.in_front = false;
        // another window losing focus leaves the front alone; this one's clears it
        center.set_front(WindowKey::unique(), false, "");
        assert!(center.front.is_some());
        center.set_front(window, false, "");
        assert_eq!(
            center
                .post(&allowed, "chat", "Chat", post("c", ""), now, 0)
                .0,
            Posted::Banner
        );
    }

    /// A burst of three a minute: the fourth is logged and raises one "more"
    /// banner keyed by the view, counting up; a token back, a banner again.
    #[test]
    fn a_burst_past_the_limit_coalesces_into_one_more_banner() {
        let start = Instant::now();
        let mut center = Center::default();
        let allowed = settings(Some(Permission::Allow));
        for nth in 0..3 {
            let (posted, _) = center.post(&allowed, "chat", "Chat", post("m", ""), start, nth);
            assert_eq!(posted, Posted::Banner);
        }
        let (posted, more) = center.post(&allowed, "chat", "Chat", post("m", ""), start, 3);
        let more = more.unwrap();
        assert_eq!(posted, Posted::Logged);
        assert_eq!(
            (more.title.as_str(), more.tag.as_str()),
            ("1 more from Chat", "more/chat")
        );
        let (_, more) = center.post(&allowed, "chat", "Chat", post("m", ""), start, 4);
        assert_eq!(more.unwrap().title, "2 more from Chat");
        assert_eq!(center.entries().count(), 5, "everything is still logged");
        // another view has its own bucket
        let mut forge = allowed.clone();
        forge.views.insert("forge".into(), Permission::Allow);
        assert_eq!(
            center
                .post(&forge, "forge", "Forge", post("f", ""), start, 5)
                .0,
            Posted::Banner
        );
        // twenty seconds refill one token at three a minute
        let later = start + Duration::from_secs(20);
        assert_eq!(
            center
                .post(&allowed, "chat", "Chat", post("m", ""), later, 6)
                .0,
            Posted::Banner
        );
        assert_eq!(
            center
                .post(&allowed, "chat", "Chat", post("m", ""), later, 7)
                .1
                .unwrap()
                .title,
            "1 more from Chat",
            "the count starts over after a banner"
        );
    }

    /// One tag, one row: it counts what folded into it, comes back to the
    /// top unread, and carries the latest words.
    #[test]
    fn notices_under_one_tag_fold_into_one_row() {
        let now = Instant::now();
        let mut center = Center::default();
        let silent = settings(Some(Permission::Silent));
        center.post(&silent, "chat", "Chat", post("first", "#room"), now, 1);
        center.post(&silent, "chat", "Chat", post("other", ""), now, 2);
        let id = center.entries().last().unwrap().id;
        center.open(id);
        center.post(&silent, "chat", "Chat", post("second", "#room"), now, 3);
        center.post(&silent, "forge", "Forge", post("theirs", "#room"), now, 4);
        let rows: Vec<_> = center
            .entries()
            .map(|entry| (entry.title.as_str(), entry.count, entry.read))
            .collect();
        assert_eq!(
            rows,
            [
                ("theirs", 1, false),
                ("second", 2, false),
                ("other", 1, false)
            ]
        );
        center.mark_all_read();
        assert_eq!(center.unread(), 0);
        center.post(&silent, "chat", "Chat", post("new", ""), now, 5);
        center.clear_read();
        assert_eq!(center.entries().count(), 1);
    }

    /// The log keeps 500 rows and thirty days, whichever is fewer.
    #[test]
    fn the_log_is_capped_by_count_and_age() {
        let now = Instant::now();
        let mut center = Center::default();
        let silent = settings(Some(Permission::Silent));
        for nth in 0..(MAX_ENTRIES as i64 + 20) {
            center.post(
                &silent,
                "chat",
                "Chat",
                post(&nth.to_string(), ""),
                now,
                nth,
            );
        }
        assert_eq!(center.entries().count(), MAX_ENTRIES);
        assert_eq!(center.entries().last().unwrap().title, "20");
        // thirty days on from the row at 100: everything before it ages out
        center.post(
            &silent,
            "chat",
            "Chat",
            post("late", ""),
            now,
            100 + MAX_AGE,
        );
        assert_eq!(center.entries().count(), 520 - 100 + 1);
        assert_eq!(center.entries().last().unwrap().title, "100");
    }

    #[test]
    fn prefs_read_back_as_settings() {
        let read = Settings::of(&serde_json::json!({}));
        assert_eq!((read.banners, read.in_front, read.burst), (true, false, 6));
        let read = Settings::of(&serde_json::json!({
            NOTIFY_PREF: false, FRONT_PREF: "show", BURST_PREF: 12,
            VIEWS_PREF: { "chat": "Silent", "forge": "nonsense" },
        }));
        assert_eq!((read.banners, read.in_front, read.burst), (false, true, 12));
        assert_eq!(read.views.len(), 1);
        assert_eq!(Settings::of(&serde_json::json!({ BURST_PREF: 7 })).burst, 6);
    }

    #[test]
    fn times_read_short() {
        let now = local_midnight(1_790_121_600) + 12 * 3_600;
        assert_eq!(ago(now - 5, now), "now");
        assert_eq!(ago(now - 14 * 60, now), "14m");
        assert_eq!(ago(now - 2 * 3_600, now), "2h");
        assert_eq!(ago(now - 20 * 3_600, now), "Yesterday");
        assert_eq!(ago(now - 4 * 86_400, now), "4d");
    }

    /// The door takes words and a duck:// link or none; bytes past the
    /// post are refused, and a post with no words is not a notice.
    #[test]
    fn a_post_is_words_a_tag_and_a_duck_link() {
        let full = Post {
            title: "a".into(),
            body: "b".into(),
            tag: "t".into(),
            link: "duck://chat/room".into(),
        };
        let mut bytes = doors::encode(&full);
        assert_eq!(doors::decode::<Post>(&bytes).unwrap(), full);
        bytes.extend_from_slice(b"icon");
        assert!(doors::decode::<Post>(&bytes).is_err());
        assert!(shortened(Post::default()).is_err());
        assert!(
            shortened(Post {
                link: "https://example.com".into(),
                ..full.clone()
            })
            .is_err()
        );
        let long = shortened(Post {
            title: "가".repeat(MAX_TEXT),
            body: "b".repeat(MAX_TEXT * 2),
            link: format!("duck://{}", "x".repeat(MAX_TEXT)),
            ..full
        })
        .unwrap();
        assert!(long.title.len() <= MAX_TEXT && long.title.chars().all(|c| c == '가'));
        assert_eq!(long.body.len(), MAX_TEXT);
        assert!(long.link.is_empty(), "a cut link goes nowhere");
    }

    /// A notice's own markup characters reach the freedesktop body escaped,
    /// so a server that renders markup shows them as the view typed them.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_freedesktop_body_escapes_the_notices_markup() {
        assert_eq!(platform::markup_escaped("a <b> & c"), "a &lt;b&gt; &amp; c");
    }
}
