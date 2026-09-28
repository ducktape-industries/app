//! The notification centre: the log of the chain in hand (one JSON file
//! per chain under `<config>/notifications`) and the banner policy —
//! permission, focus, the per-view token bucket — that decides what a
//! posted notice becomes. The app keeps one, reached through [`center`];
//! a test builds its own.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use super::settings::{Permission, Settings};
use crate::runtime::WindowKey;
use view_wire::methods::{Delivery, Notification};

/// One banner as the host words it; a later one under the same non-empty
/// `tag` replaces the standing one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notice {
    pub title: String,
    pub body: String,
    pub tag: String,
}

/// The centre keeps this many rows, and none older than this.
pub(super) const MAX_ENTRIES: usize = 500;
pub(super) const MAX_AGE: i64 = 30 * 86_400;

/// One row of the centre.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: u64,
    /// The program whose view posted (its roster name).
    pub(crate) module: String,
    /// That view's own name when it posted.
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
    /// The chain id (`<network>#<salt>`) the log is for; none, and nothing
    /// is written to disk.
    network: String,
    entries: Vec<Entry>,
    next: u64,
    buckets: HashMap<String, Bucket>,
    /// Notices past the burst limit since the view's last banner.
    more: HashMap<String, u32>,
    /// The window in front and the view focused in it.
    pub(super) front: Option<(WindowKey, &'static str)>,
    /// Views that posted before the person said anything about them.
    pub(super) asking: BTreeSet<String>,
    pub(super) not_now: BTreeSet<String>,
}

/// A notification centre, shared by whoever holds a clone: the app's one
/// ([`center`]), which its views post into and its windows draw, or a
/// test's own.
#[derive(Clone, Default)]
pub(crate) struct CenterHandle(Arc<Mutex<Center>>);

impl CenterHandle {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Center> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The app's one centre.
pub(crate) fn center() -> &'static CenterHandle {
    static CENTER: OnceLock<CenterHandle> = OnceLock::new();
    CENTER.get_or_init(CenterHandle::default)
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
        post: Notification,
        now: Instant,
        wall: i64,
    ) -> (Delivery, Option<Notice>) {
        let permission = settings.views.get(module).copied();
        if permission == Some(Permission::Block) {
            return (Delivery::Blocked, None);
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
                return (Delivery::Logged, None);
            }
            Some(Permission::Silent) => return (Delivery::Logged, None),
            _ => {}
        }
        let in_front = self.front.is_some_and(|(_, focused)| focused == module);
        if !settings.banners || (in_front && !settings.in_front) {
            return (Delivery::Logged, None);
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
            return (Delivery::Banner, Some(notice));
        }
        let more = self.more.entry(module.to_owned()).or_default();
        *more += 1;
        let more = Notice {
            title: format!("{more} more from {view}"),
            body: "They're waiting in Notifications.".into(),
            tag: format!("more/{module}"),
        };
        (Delivery::Logged, Some(more))
    }

    /// Into the log: a notice under a tag the view already has a row for
    /// folds into that row and brings it back to the top, unread.
    fn log(&mut self, module: &str, view: &str, post: Notification, wall: i64) {
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

    /// `module` says the reader has seen what it posted under `tag`: its
    /// rows under it are read, and no other view's. Whether any changed.
    pub(crate) fn read_tag(&mut self, module: &str, tag: &str) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if !tag.is_empty() && entry.module == module && entry.tag == tag && !entry.read {
                entry.read = true;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
        changed
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

    /// `module`'s window asks, with nothing logged: the permission bar
    /// drawn without an unread count on the bell.
    #[cfg(test)]
    pub(crate) fn ask_for_test(&mut self, module: &str) {
        self.asking.insert(module.to_owned());
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
