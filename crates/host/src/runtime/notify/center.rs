//! The notification centre: the log of the chain in hand (one JSON file
//! per chain under `<config>/notifications`), the device's own rows beside
//! it (the host's, [`HOST`], one file there too: they show whichever chain
//! is in hand, and before any is) and the banner policy — permission,
//! focus, the per-view token bucket — that decides what a posted notice
//! becomes. The app keeps one, reached through [`center`]; a test builds
//! its own.

use rt::time::Instant;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::settings::{Permission, Settings};
use crate::runtime::WindowKey;
pub use view_wire::methods::NoticeKind;
use view_wire::methods::{Delivery, Notification};

/// One banner as the host words it; a later one under the same non-empty
/// `tag` replaces the standing one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub tag: String,
    /// The program whose notice it is (or whose notices it counts).
    pub program: String,
    /// The `duck://` link the notice opens, as the program gave it; empty
    /// for none.
    pub link: String,
    pub kind: NoticeKind,
    /// Unix seconds past which the notice is stale: never raised past it,
    /// and taken down at it ([`Center::ring_over`]). 0: none.
    pub until: u64,
    /// For a call: the `duck://` link a carrier opens when the person ends
    /// the call they answered from the system's own call screen, as the
    /// program gave it; empty for none. Never for a decline, nor for an
    /// end made in the app or by the program.
    pub end_link: String,
    /// The chain id (`<network>#<salt>`) whose centre let it through: two
    /// listed networks' programs may raise one tag, and each stands.
    pub chain: String,
}

impl Notice {
    /// Its id in a phone OS's list (APNs's collapse id, a local
    /// notification's identifier, Android's notification tag), whoever
    /// raises it there: the same tag and link, the same notice, which
    /// replaces itself; any other stands beside it. Always hashed: 64
    /// bytes, APNs's limit, and a header value whatever the program's tag
    /// holds.
    pub fn os_id(&self) -> String {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(format!("{}\n{}", self.tag, self.link));
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// One row of the centre.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub id: u64,
    /// The program whose view posted (its roster name).
    pub program: String,
    /// That view's own name when it posted.
    pub view: String,
    pub title: String,
    pub body: String,
    pub tag: String,
    pub link: String,
    /// Unix seconds of the latest notice folded into the row.
    pub at: i64,
    /// Notices folded into the row under its tag.
    pub count: u32,
    pub read: bool,
}

/// The program the host's own rows are under, and the first part of its
/// banners' tags: no view's.
pub const HOST: &str = "ducktape";

const DEVICE_ROW_ID_BIT: u64 = 1 << 63;

pub(super) const CARRIED_KEPT: usize = 128;

impl Entry {
    /// One of the host's own rows, the device's ([`Center::post_host`]),
    /// not a view's.
    pub fn host(&self) -> bool {
        self.id & DEVICE_ROW_ID_BIT != 0
    }
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

/// The notification centre: the log of the network in hand, and what the
/// policy remembers between notices.
#[derive(Default)]
pub struct Center {
    network: String,
    log: Option<PathBuf>,
    entries: Vec<Entry>,
    next: u64,
    device: Vec<Entry>,
    device_log: Option<PathBuf>,
    buckets: HashMap<String, Bucket>,
    more: HashMap<String, u32>,
    /// The window in front and the view focused in it.
    pub front: Option<(WindowKey, &'static str)>,
    /// Views that posted before the person said anything about them.
    pub asking: BTreeSet<String>,
    pub(super) not_now: BTreeSet<String>,
    pub(crate) sharing: BTreeSet<String>,
    told: BTreeSet<String>,
    until: HashMap<String, u64>,
    calls: HashMap<String, String>,
    carried: Vec<Entry>,
    carried_moved: bool,
    rev: u64,
    saved: u64,
    #[cfg(test)]
    pub(super) saves: Arc<std::sync::atomic::AtomicUsize>,
}

/// A notification centre, shared by whoever holds a clone: the app's one
/// ([`center`]), which its views post into and its windows draw, or a
/// test's own.
#[derive(Clone, Default)]
pub struct CenterHandle(Arc<Mutex<Center>>);

impl CenterHandle {
    pub fn lock(&self) -> MutexGuard<'_, Center> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The app's one centre.
pub fn center() -> &'static CenterHandle {
    static CENTER: OnceLock<CenterHandle> = OnceLock::new();
    CENTER.get_or_init(CenterHandle::default)
}

/// Unix seconds now.
pub fn wall() -> i64 {
    rt::time::SystemTime::now()
        .duration_since(rt::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

impl Center {
    /// What to do with one notice from `program` (named `view`): the answer
    /// for the view, and the banner to raise, if any — the notice's own, or
    /// the view's "N more" once it is past its burst. A call is never held
    /// back by the burst nor folded into "N more" but rings only while no
    /// other call of its program stands, under any tag (a default the owner
    /// may veto), and also before the person said anything about its view.
    /// It stands until its `until`, `notify.seen` of its tag, or a decline
    /// on its banner ([`Center::declined`]); without a tag or an `until` it
    /// is a plain notice. A notice past its `until` is logged only, and so
    /// is this device's run of one the push server carried here: the OS
    /// showed that one already ([`Center::carried`]).
    pub fn post(
        &mut self,
        settings: &Settings,
        program: &str,
        view: &str,
        post: Notification,
        now: Instant,
        wall: i64,
    ) -> (Delivery, Option<Notice>) {
        let until = post.until;
        let kind = match until != 0 && !post.tag.is_empty() {
            true => post.kind,
            false => NoticeKind::Notice,
        };
        let seconds = u64::try_from(wall).unwrap_or(0);
        let permission = settings.views.get(program).copied().or(settings.unasked);
        if permission == Some(Permission::Block) {
            return (Delivery::Blocked, None);
        }
        let notice = Notice {
            title: post.title.clone(),
            body: post.body.clone(),
            tag: match post.tag.is_empty() {
                true => String::new(),
                false => format!("{program}/{}", post.tag),
            },
            program: program.to_owned(),
            link: post.link.clone(),
            kind,
            until,
            end_link: post.end_link.clone(),
            chain: self.network.clone(),
        };
        let shown = self.log(program, view, post, wall);
        if shown || (until != 0 && until <= seconds) {
            return (Delivery::Logged, None);
        }
        let call = kind == NoticeKind::Call;
        match permission {
            None => {
                if !self.not_now.contains(program) {
                    self.asking.insert(program.to_owned());
                }
                if !call {
                    return (Delivery::Logged, None);
                }
            }
            Some(Permission::Silent) => return (Delivery::Logged, None),
            _ => {}
        }
        let in_front = self.front.is_some_and(|(_, focused)| focused == program);
        if !settings.banners || (in_front && !settings.in_front) {
            return (Delivery::Logged, None);
        }
        if call {
            let standing = self.calls.get(program).and_then(|tag| self.until.get(tag));
            if standing.is_some_and(|&until| until > seconds) {
                return (Delivery::Logged, None);
            }
            self.calls.insert(program.to_owned(), notice.tag.clone());
            self.raised(&notice);
            return (Delivery::Banner, Some(notice));
        }
        let burst = f64::from(settings.burst.max(1));
        let bucket = self.buckets.entry(program.to_owned()).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        let refill = now.saturating_duration_since(bucket.at).as_secs_f64() * burst / 60.;
        bucket.tokens = (bucket.tokens + refill).min(burst);
        bucket.at = now;
        if bucket.tokens >= 1. {
            bucket.tokens -= 1.;
            self.more.remove(program);
            self.raised(&notice);
            return (Delivery::Banner, Some(notice));
        }
        let more = self.more.entry(program.to_owned()).or_default();
        *more += 1;
        let more = Notice {
            title: format!("{more} more from {view}"),
            body: "They're waiting in Notifications.".into(),
            tag: format!("more/{program}"),
            program: program.to_owned(),
            link: String::new(),
            chain: self.network.clone(),
            ..Notice::default()
        };
        (Delivery::Logged, Some(more))
    }

    fn raised(&mut self, notice: &Notice) {
        if notice.tag.is_empty() {
            return;
        }
        match notice.until {
            0 => self.until.remove(&notice.tag),
            until => self.until.insert(notice.tag.clone(), until),
        };
    }

    /// The host's timer at `until` for the banner raised under `tag`
    /// (`notify::withdraw_at`): whether that banner is still the one
    /// standing there, now taken off the list. A later banner under the
    /// tag (a call rung again), or one already taken down, is not.
    pub fn ring_over(&mut self, tag: &str, until: u64) -> bool {
        let over = self.until.get(tag) == Some(&until);
        if over {
            self.until.remove(tag);
        }
        over
    }

    /// The call raised under `tag` declined on its banner (this device
    /// alone, SPEC D11): it stands no more, so its program's next call
    /// rings at once, and its timer finds nothing to take down.
    pub fn declined(&mut self, tag: &str) {
        self.until.remove(tag);
    }

    fn log(&mut self, program: &str, view: &str, post: Notification, wall: i64) -> bool {
        let twin = self.carried.iter().position(|copy| {
            copy.program == program
                && copy.tag == post.tag
                && (&copy.title, &copy.body, &copy.link) == (&post.title, &post.body, &post.link)
        });
        if let Some(twin) = twin {
            self.carried.remove(twin);
            (self.carried_moved, self.rev) = (true, self.rev + 1);
            return true;
        }
        let folded = (!post.tag.is_empty())
            .then(|| {
                self.entries
                    .iter()
                    .position(|entry| entry.program == program && entry.tag == post.tag)
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
                    program: program.to_owned(),
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
        self.rev += 1;
        false
    }

    /// A notice its program's notifier posted for this person on a host
    /// with no screen (the push server, `crates/pushd`), which reached this
    /// device as one of the OS's own notices: `notice` as that host's centre
    /// let it through (its tag `<program>/<tag>`, as [`Center::post`] words
    /// a banner), shown at `at` (Unix seconds); `opened` when the person
    /// opened it there. Logged as a post is, at `at`, unless the log has it
    /// already: the row it folds into is as new or newer (an earlier read
    /// of the OS's notices, or this device's own run of the notifier since)
    /// or is unread and says the same; opened, that row is read. A blocked
    /// program's is dropped, as its post is; so is one whose link is no
    /// `duck://` link, and one under a tag that is not the program's (that
    /// centre's own "N more" past the burst, which counts notices this
    /// device never got and no centre logs). Written now: no view's redraw
    /// follows it. Whether the log moved.
    pub fn carried(&mut self, settings: &Settings, notice: &Notice, at: i64, opened: bool) -> bool {
        let program = notice.program.as_str();
        let permission = settings.views.get(program).copied().or(settings.unasked);
        let linked = notice.link.is_empty() || notice.link.starts_with("duck://");
        if program.is_empty() || permission == Some(Permission::Block) || !linked {
            return false;
        }
        let tag = match notice.tag.is_empty() {
            true => Some(""),
            false => notice.tag.strip_prefix(&format!("{program}/")),
        };
        let Some(tag) = tag else {
            return false;
        };
        let says = |entry: &Entry| {
            (&entry.title, &entry.body, &entry.link) == (&notice.title, &notice.body, &notice.link)
        };
        let row = self.entries.iter().rposition(|entry| {
            entry.program == program && entry.tag == tag && (!tag.is_empty() || says(entry))
        });
        let logged = row.filter(|&index| {
            let entry = &self.entries[index];
            entry.at >= at || (!entry.read && says(entry))
        });
        match logged {
            Some(index) => {
                let entry = &mut self.entries[index];
                if !opened || entry.read {
                    return false;
                }
                entry.read = true;
            }
            None => {
                let folded = row
                    .filter(|_| !tag.is_empty())
                    .map(|index| self.entries.remove(index));
                let (id, count) = match folded {
                    Some(folded) => (folded.id, folded.count + 1),
                    None => {
                        self.next += 1;
                        (self.next, 1)
                    }
                };
                let entry = Entry {
                    id,
                    program: program.to_owned(),
                    view: program.to_owned(),
                    title: notice.title.clone(),
                    body: notice.body.clone(),
                    tag: tag.to_owned(),
                    link: notice.link.clone(),
                    at,
                    count,
                    read: opened,
                };
                self.carried.push(entry.clone());
                if self.carried.len() > CARRIED_KEPT {
                    self.carried.remove(0);
                }
                self.carried_moved = true;
                let place = self
                    .entries
                    .iter()
                    .rposition(|row| row.at <= at)
                    .map_or(0, |index| index + 1);
                self.entries.insert(place, entry);
            }
        }
        self.rev += 1;
        self.flush();
        true
    }

    /// The host's own notice: a row of the device's, under `tag`, where a
    /// row there already takes its words and comes back to the top, unread
    /// (a newer release's "ready").
    /// Its banner is the shell's to raise; written now, no view's redraw
    /// writes it.
    pub fn post_host(&mut self, title: &str, body: &str, tag: &str, wall: i64) {
        let at = self.device.iter().position(|entry| entry.tag == tag);
        let id = match at {
            Some(index) => self.device.remove(index).id,
            None => {
                let last = self.device.iter().map(|entry| entry.id).max();
                last.map_or(DEVICE_ROW_ID_BIT | 1, |id| id + 1)
            }
        };
        self.device.push(Entry {
            id,
            program: HOST.to_owned(),
            view: "Ducktape".to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            tag: tag.to_owned(),
            link: String::new(),
            at: wall,
            count: 1,
            read: false,
        });
        self.device_moved();
    }

    /// The host's row under `tag` taken out: what it said holds no more.
    pub fn withdraw_host(&mut self, tag: &str) {
        let before = self.device.len();
        self.device.retain(|entry| entry.tag != tag);
        if self.device.len() != before {
            self.device_moved();
        }
    }

    fn device_moved(&mut self) {
        let clean = self.saved == self.rev;
        self.rev += 1;
        if clean {
            self.saved = self.rev;
        }
        if let Some(path) = &self.device_log {
            self.save(path.clone(), self.device.clone());
        }
    }

    /// Newest first: the chain's rows and the device's, merged by when
    /// they came (each log in its own order).
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        let mut chain = self.entries.iter().rev().peekable();
        let mut device = self.device.iter().rev().peekable();
        std::iter::from_fn(move || {
            let host = match (chain.peek(), device.peek()) {
                (Some(view), Some(host)) => host.at > view.at,
                (chain, _) => chain.is_none(),
            };
            match host {
                true => device.next(),
                false => chain.next(),
            }
        })
    }

    /// The row a view's notice last went into.
    pub fn logged(&self) -> Option<u64> {
        self.entries.last().map(|entry| entry.id)
    }

    pub fn unread(&self) -> usize {
        self.rows().filter(|entry| !entry.read).count()
    }

    fn rows(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().chain(&self.device)
    }

    /// How many times the log changed: equal, it lists what it listed.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// A row picked: read now, and handed back to open.
    pub fn open(&mut self, id: u64) -> Option<Entry> {
        let entry =
            (self.entries.iter_mut().chain(&mut self.device)).find(|entry| entry.id == id)?;
        entry.read = true;
        let entry = entry.clone();
        match entry.host() {
            true => self.device_moved(),
            false => {
                self.rev += 1;
                self.flush();
            }
        }
        Some(entry)
    }

    /// `program` says the reader has seen what it posted under `tag`: its
    /// rows under it are read, and no other view's. Whether any changed.
    /// Written at the end of the view's redraw ([`Center::flush`]).
    pub fn read_tag(&mut self, program: &str, tag: &str) -> bool {
        self.until.remove(&format!("{program}/{tag}"));
        let mut changed = false;
        for entry in &mut self.entries {
            if !tag.is_empty() && entry.program == program && entry.tag == tag && !entry.read {
                entry.read = true;
                changed = true;
            }
        }
        if changed {
            self.rev += 1;
        }
        changed
    }

    pub fn mark_all_read(&mut self) {
        for entry in self.entries.iter_mut().chain(&mut self.device) {
            entry.read = true;
        }
        self.rev += 1;
        self.flush();
        self.device_moved();
    }

    pub fn clear_read(&mut self) {
        self.entries.retain(|entry| !entry.read);
        self.device.retain(|entry| !entry.read);
        self.rev += 1;
        self.flush();
        self.device_moved();
    }

    /// Whether `program`'s window shows the permission bar.
    #[cfg(any(test, feature = "test-support"))]
    pub fn asking(&self, program: &str) -> bool {
        self.asking.contains(program)
    }

    /// `program`'s window asks, with nothing logged: the permission bar
    /// drawn without an unread count on the bell.
    #[cfg(any(test, feature = "test-support"))]
    pub fn ask_for_test(&mut self, program: &str) {
        self.asking.insert(program.to_owned());
    }

    pub(crate) fn went_live(&mut self, program: &str) -> bool {
        !self.told.contains(program) && self.sharing.insert(program.to_owned())
    }

    /// The person read `program`'s live line: it goes for this run.
    pub fn put_away_live(&mut self, program: &str) {
        self.sharing.remove(program);
        self.told.insert(program.to_owned());
    }

    /// Notices from `program` this past week, folded ones counted.
    pub fn this_week(&self, program: &str, wall: i64) -> u32 {
        self.entries
            .iter()
            .filter(|entry| entry.program == program && wall - entry.at <= 7 * 86_400)
            .map(|entry| entry.count)
            .sum()
    }

    /// A window came to the front or left it, or its panes moved
    /// (`WindowRoot`'s activation and `Desk` observers): whether it is in
    /// front, and the view focused in it.
    pub fn set_front(&mut self, window: WindowKey, active: bool, focused: &'static str) {
        match active {
            true => self.front = Some((window, focused)),
            false if self.front.is_some_and(|(key, _)| key == window) => self.front = None,
            false => {}
        }
    }

    /// The chain whose log this is; empty with none (the Connect screen, a
    /// followed set whose chain was taken up in hand).
    pub fn network(&self) -> &str {
        &self.network
    }

    #[cfg(test)]
    pub(super) fn open_log(&mut self, network: &str, path: Option<PathBuf>) {
        self.put_log(Log::read(network, path));
    }

    pub(crate) fn take_log(&mut self) -> Log {
        self.flush();
        let log = Log {
            network: std::mem::take(&mut self.network),
            log: self.log.take(),
            entries: std::mem::take(&mut self.entries),
            next: std::mem::take(&mut self.next),
            carried: std::mem::take(&mut self.carried),
            until: std::mem::take(&mut self.until),
            calls: std::mem::take(&mut self.calls),
        };
        self.rev += 1;
        self.saved = self.rev;
        log
    }

    pub(crate) fn put_log(&mut self, log: Log) -> Log {
        let left = self.take_log();
        *self = Center {
            network: log.network,
            log: log.log,
            entries: log.entries,
            next: log.next,
            carried: log.carried,
            until: log.until,
            calls: log.calls,
            front: self.front,
            rev: self.rev + 1,
            saved: self.rev + 1,
            device: std::mem::take(&mut self.device),
            device_log: self.device_log.take(),
            #[cfg(test)]
            saves: self.saves.clone(),
            ..Center::default()
        };
        left
    }

    /// The device's own rows off disk, as the host starts:
    /// `<config>/notifications/@device.json` (`@` is in no chain's file
    /// name, [`log_path`]).
    pub fn open_device(&mut self) {
        self.open_device_log(notifications_dir().join("@device.json"));
    }

    pub(super) fn open_device_log(&mut self, path: PathBuf) {
        (self.device, self.device_log) = match opened(&path) {
            Some(device) => (device, Some(path)),
            None => (Vec::new(), None),
        };
        let saved = self.saved == self.rev;
        self.rev += 1;
        if saved {
            self.saved = self.rev;
        }
    }

    /// The chain's log handed to the disk if it moved since it last was:
    /// once per person's action here (open, mark all read, clear read), and
    /// once at the end of each view redraw for whatever its requests logged
    /// (`View::redraw`), never per post. (The device's rows go as they
    /// move.) Written whole through
    /// `noded_client::fs::atomic_write` on the blocking pool, one write after
    /// another (`kernel::in_order` under the log's path).
    pub fn flush(&mut self) {
        if self.saved == self.rev {
            return;
        }
        self.saved = self.rev;
        if let Some(path) = &self.log {
            self.save(path.clone(), self.entries.clone());
            if std::mem::take(&mut self.carried_moved) {
                self.save(pending_carried_copies_path(path), self.carried.clone());
            }
        }
    }

    fn save(&self, path: PathBuf, entries: Vec<Entry>) {
        let name = path.to_string_lossy().into_owned();
        #[cfg(test)]
        let saves = self.saves.clone();
        crate::runtime::kernel::in_order(&name, move || {
            let _timed = crate::perf::time(crate::perf::Key::Shell, "io.notify_save");
            if let Ok(bytes) = serde_json::to_vec(&entries) {
                let _ = crate::contract::os().disk().write(&path, &bytes, false);
            }
            #[cfg(test)]
            saves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
    }
}

#[derive(Default)]
pub(crate) struct Log {
    network: String,
    log: Option<PathBuf>,
    entries: Vec<Entry>,
    next: u64,
    carried: Vec<Entry>,
    until: HashMap<String, u64>,
    calls: HashMap<String, String>,
}

impl Log {
    pub(crate) fn of(network: &str) -> Self {
        Self::read(network, log_path(network))
    }

    pub(crate) fn read(network: &str, path: Option<PathBuf>) -> Self {
        let mut log = Log {
            network: network.to_owned(),
            ..Log::default()
        };
        if let Some((entries, path)) = path.and_then(|path| Some((opened(&path)?, path))) {
            log.carried = opened(&pending_carried_copies_path(&path)).unwrap_or_default();
            (log.entries, log.log) = (entries, Some(path));
        }
        log.next = log.entries.iter().map(|entry| entry.id).max().unwrap_or(0);
        log
    }

    pub(crate) fn file(&self) -> Option<&Path> {
        self.log.as_deref()
    }
}

fn opened(path: &Path) -> Option<Vec<Entry>> {
    read_log(path)
        .inspect_err(|error| {
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "notification log unreadable, not saving over it");
        })
        .ok()
}

pub(super) fn read_log(path: &Path) -> std::io::Result<Vec<Entry>> {
    crate::backend::read_or_set_aside(path, |bytes| serde_json::from_slice(bytes))
        .map(Option::unwrap_or_default)
}

pub(crate) fn log_path(network: &str) -> Option<PathBuf> {
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
    Some(notifications_dir().join(format!("{file}.json")))
}

fn pending_carried_copies_path(log: &Path) -> PathBuf {
    let chain = log.file_stem().unwrap_or_default().to_string_lossy();
    log.with_file_name(format!("{chain}@carried.json"))
}

fn notifications_dir() -> PathBuf {
    crate::backend::dirs().config.join("notifications")
}
