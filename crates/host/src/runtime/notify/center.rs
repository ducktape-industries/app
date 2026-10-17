//! The notification centre: the log of the chain in hand (one JSON file
//! per chain under `<config>/notifications`), the device's own rows beside
//! it (the host's, [`HOST`], one file there too: they show whichever chain
//! is in hand, and before any is) and the banner policy — permission,
//! focus, the per-view token bucket — that decides what a posted notice
//! becomes. The app keeps one, reached through [`center`]; a test builds
//! its own.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

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

/// The bit every device row's id carries and no chain row's does: the two
/// logs number their rows apart.
const DEVICE: u64 = 1 << 63;

impl Entry {
    /// One of the host's own rows, the device's ([`Center::post_host`]),
    /// not a view's.
    pub fn host(&self) -> bool {
        self.id & DEVICE != 0
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
    /// The chain id (`<network>#<salt>`) the log is for.
    network: String,
    /// Where the log is saved: none with no network in hand, or when its
    /// file could not be read (nothing then writes over it).
    log: Option<PathBuf>,
    entries: Vec<Entry>,
    next: u64,
    /// The device's own rows, the host's: kept whichever chain is in hand.
    device: Vec<Entry>,
    /// Where they are saved: none until the host opens them
    /// ([`Center::open_device`]), or when their file could not be read.
    device_log: Option<PathBuf>,
    buckets: HashMap<String, Bucket>,
    /// Notices past the burst limit since the view's last banner.
    more: HashMap<String, u32>,
    /// The window in front and the view focused in it.
    pub front: Option<(WindowKey, &'static str)>,
    /// Views that posted before the person said anything about them.
    pub asking: BTreeSet<String>,
    pub(super) not_now: BTreeSet<String>,
    /// Views that opened a live track, their window's line not yet put
    /// away (the consent line, [`Center::went_live`]).
    pub(crate) sharing: BTreeSet<String>,
    /// Views whose live line the person put away this run.
    told: BTreeSet<String>,
    /// The `until` of each banner standing under a tag that has one.
    until: HashMap<String, u64>,
    /// Each program's last call rung, by its tag: it stands while its tag
    /// is in `until` and not past it.
    calls: HashMap<String, String>,
    /// The notices the push server carried here ([`Center::carried`]) that
    /// this device's own run of their notifier has not posted yet: its
    /// post of one is that notice again ([`Center::log`]).
    // ponytail: one the run here never posts (the program skipped it, on
    // screen here) waits until the network changes and takes one later
    // post saying the same; a carried notice's seq in the payload if felt
    carried: Vec<Entry>,
    /// Moves with every change to the log: the bell's list is read again.
    rev: u64,
    /// The `rev` last handed to the disk ([`Center::flush`]).
    saved: u64,
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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
    /// is a plain notice. A notice past its `until` is logged only.
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
        // a ring has an end and a tag to take it down by
        let kind = match until != 0 && !post.tag.is_empty() {
            true => post.kind,
            false => NoticeKind::Notice,
        };
        let seconds = u64::try_from(wall).unwrap_or(0);
        let permission = settings.views.get(program).copied().or(settings.unasked);
        if permission == Some(Permission::Block) {
            return (Delivery::Blocked, None);
        }
        // a view's tags are its own: two views never replace each other's
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
        };
        self.log(program, view, post, wall);
        if until != 0 && until <= seconds {
            return (Delivery::Logged, None);
        }
        let call = kind == NoticeKind::Call;
        match permission {
            None => {
                if !self.not_now.contains(program) {
                    self.asking.insert(program.to_owned());
                }
                // a call rings while the person has not said: a missed call
                // is not one they chose to miss
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
            ..Notice::default()
        };
        (Delivery::Logged, Some(more))
    }

    /// `notice`'s banner goes up: it stands under its tag until its
    /// `until`, or with none when it has none.
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

    /// Into the log: a notice under a tag the view already has a row for
    /// folds into that row and brings it back to the top, unread. Past the
    /// view's `share` of rows its own oldest go, never another view's.
    /// Written at the end of the view's redraw ([`Center::flush`]). A post
    /// saying what a notice the push server carried here said, under its
    /// tag ([`Center::carried`]), is this device's run of that same notice:
    /// its row stands as it is, read or not, counted once.
    fn log(&mut self, program: &str, view: &str, post: Notification, wall: i64) {
        let twin = self.carried.iter().position(|copy| {
            copy.program == program
                && copy.tag == post.tag
                && (&copy.title, &copy.body, &copy.link) == (&post.title, &post.body, &post.link)
        });
        if let Some(twin) = twin {
            self.carried.swap_remove(twin);
            return;
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
        // the row it folds into: its tag's, or with none the newest saying it
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
                // newest last, as `entries` reads them: it may be older than rows logged since
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
                last.map_or(DEVICE | 1, |id| id + 1)
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

    /// The device's rows moved: the bell reads again, and they are handed
    /// to their file now (no view's redraw writes them), the chain's log
    /// no dirtier for it.
    fn device_moved(&mut self) {
        let clean = self.saved == self.rev;
        self.rev += 1;
        if clean {
            self.saved = self.rev;
        }
        if let Some(path) = &self.device_log {
            save(path.clone(), self.device.clone());
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

    /// Both logs' rows, in no order.
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
        // its banner is taken down (`notify::seen`): no timer of its stays
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

    /// `program` opened a live track: its window says so in a line, once a
    /// run. `true` when the line is new.
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

    /// The network in hand: its log comes off disk, the policy starts over.
    pub fn set_network(&mut self, network: &str) {
        if self.network != network {
            // the last network's rows go to its own file first
            self.flush();
            self.open_log(network, log_path(network));
        }
    }

    /// `network` in hand with its log at `path`. A log that cannot be read
    /// leaves the centre empty and with no `log`, so no save replaces it.
    /// The device's rows stay.
    pub(super) fn open_log(&mut self, network: &str, path: Option<PathBuf>) {
        *self = Center {
            network: network.to_owned(),
            front: self.front,
            rev: self.rev + 1,
            saved: self.rev + 1,
            device: std::mem::take(&mut self.device),
            device_log: self.device_log.take(),
            ..Center::default()
        };
        if let Some((entries, path)) = path.and_then(|path| Some((opened(&path)?, path))) {
            (self.entries, self.log) = (entries, Some(path));
        }
        self.next = self.entries.iter().map(|entry| entry.id).max().unwrap_or(0);
    }

    /// The device's own rows off disk, as the host starts:
    /// `<config>/notifications/@device.json` (`@` is in no chain's file
    /// name, [`log_path`]).
    pub fn open_device(&mut self) {
        self.open_device_log(notifications_dir().join("@device.json"));
    }

    /// The device's rows from the log at `path`. One that cannot be read
    /// leaves them empty and with no file, so no save replaces it.
    pub(super) fn open_device_log(&mut self, path: PathBuf) {
        (self.device, self.device_log) = match opened(&path) {
            Some(device) => (device, Some(path)),
            None => (Vec::new(), None),
        };
        // read, not changed: nothing to write back
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
            save(path.clone(), self.entries.clone());
        }
    }
}

/// `entries` written whole to `path` on the blocking pool, after every
/// write to it queued before (`kernel::in_order` under the path).
fn save(path: PathBuf, entries: Vec<Entry>) {
    let name = path.to_string_lossy().into_owned();
    crate::runtime::kernel::in_order(&name, move || {
        let _timed = crate::perf::time(crate::perf::Key::Shell, "io.notify_save");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(&entries) {
            let _ = noded_client::fs::atomic_write(&path, &bytes, false);
        }
    });
}

/// The rows of the log at `path` ([`read_log`]), or none when it cannot be
/// read, said in app.log.
fn opened(path: &Path) -> Option<Vec<Entry>> {
    read_log(path)
        .inspect_err(|error| {
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "notification log unreadable, not saving over it");
        })
        .ok()
}

/// The log at `path`; empty when it is missing. One that will not parse is
/// set aside as `.bad`, so the next save does not overwrite it. Any other
/// read error stays an error.
pub(super) fn read_log(path: &Path) -> std::io::Result<Vec<Entry>> {
    crate::backend::read_or_set_aside(path, |bytes| serde_json::from_slice(bytes))
        .map(Option::unwrap_or_default)
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
    Some(notifications_dir().join(format!("{file}.json")))
}

/// `<config>/notifications`, where every log is.
fn notifications_dir() -> PathBuf {
    crate::backend::dirs().config.join("notifications")
}
