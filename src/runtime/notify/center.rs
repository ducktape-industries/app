//! The notification centre: a view of the inbox at the RPC address. It
//! keeps the page of entries read this session (memory only), the inbox's
//! unread count, the account's mutes, and the banner policy — permission,
//! focus, the per-program token bucket — that decides what an arriving
//! entry becomes on this device. The app keeps one, reached through
//! [`center`]; a test builds its own.
//!
//! Nothing here knows a program: `program` and `tag` are the inbox's words.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use inbox_api::{Entry, Mutes, Read};

use super::settings::{Permission, Settings};
use crate::backend::inbox::Client;
use crate::runtime::WindowKey;

/// One banner as the host words it; a later one under the same non-empty
/// `tag` replaces the standing one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notice {
    pub title: String,
    pub body: String,
    pub tag: String,
}

/// One row of the centre: the inbox's entries under one `(program, tag)`,
/// folded at display time; an entry with no tag is a row of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// The newest entry's id: what names the row.
    pub(crate) id: i64,
    pub(crate) program: String,
    pub(crate) tag: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) link: notices::Link,
    /// Unix seconds of the newest entry folded in.
    pub(crate) at: i64,
    pub(crate) count: u32,
    /// Every entry folded in is read.
    pub(crate) read: bool,
    /// The newest height folded in: what opening the row reads through.
    pub(crate) through_height: u64,
}

/// Where the inbox stands, as the bell says it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum InboxState {
    /// No network in hand, or the client not started yet.
    #[default]
    Off,
    Ready,
    /// `status` answered 404: this address keeps no notifications.
    NoInbox,
    /// The inbox did not answer; the page read this session stands. Unix
    /// seconds since.
    Unreachable {
        since: i64,
    },
    /// The inbox refused the login: shown, not retried.
    Denied(String),
}

/// What a row's pick or a banner's click does: the read sent upstream,
/// and the link minted, or none when the notice's link does not mint.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Opened {
    pub(crate) link: Option<String>,
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

#[derive(Default)]
pub(crate) struct Center {
    /// The chain id (`<network>#<salt>`) the page is for.
    network: String,
    /// The inbox client of the connection, once the runner made one.
    client: Option<Arc<Client>>,
    pub(crate) state: InboxState,
    /// The inbox's follower stopped or stalled at a height: nothing past
    /// it arrives until its operator acts. Shown beside the count.
    pub(crate) stuck: Option<inbox_api::Stopped>,
    /// Newest first, as the inbox lists them.
    entries: Vec<Entry>,
    /// The inbox's count, kept current by `live`.
    unread: u64,
    pub(crate) mutes: Mutes,
    /// The max `through_height` a view said it saw per `(program, tag)`.
    watermarks: HashMap<(String, String), u64>,
    buckets: HashMap<String, Bucket>,
    /// Entries past the burst limit since the program's last banner.
    more: HashMap<String, u32>,
    /// The window in front and the view focused in it.
    pub(crate) front: Option<(WindowKey, &'static str)>,
    /// Programs whose entries arrived before the person said a word on
    /// their banners: the question at the top of the centre.
    pub(crate) asking: BTreeSet<String>,
    /// Moves with every change: the bell's list is read again.
    rev: u64,
}

/// A notification centre, shared by whoever holds a clone: the app's one
/// ([`center`]), which the inbox runner fills and its windows draw, or a
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

/// The standing banner's tag for an entry: a program's tags are its own,
/// so two programs never replace each other's banner.
pub(super) fn banner_tag(program: &str, tag: &str) -> String {
    match tag.is_empty() {
        true => String::new(),
        false => format!("{program}/{tag}"),
    }
}

impl Center {
    /// The network in hand: the last one's page goes, the policy starts
    /// over. The runner fills it again once it connects.
    pub(crate) fn set_network(&mut self, network: &str) {
        if self.network != network {
            *self = Center {
                network: network.to_owned(),
                front: self.front,
                rev: self.rev + 1,
                ..Center::default()
            };
        }
    }

    /// The runner's client, for the calls the person's actions send.
    pub(crate) fn set_client(&mut self, client: Option<Arc<Client>>) {
        self.client = client;
    }

    /// What `status` said of the follower: stopped or stalled, or neither.
    pub(crate) fn set_stuck(&mut self, stuck: Option<inbox_api::Stopped>) {
        if self.stuck != stuck {
            self.stuck = stuck;
            self.rev += 1;
        }
    }

    pub(crate) fn set_state(&mut self, state: InboxState) {
        if self.state != state {
            self.state = state;
            self.rev += 1;
        }
    }

    /// The page the inbox listed (newest first) and its unread count.
    pub(crate) fn replace(&mut self, entries: Vec<Entry>, unread: u64) {
        self.entries = entries;
        self.unread = unread;
        for entry in &mut self.entries {
            let key = (entry.program.clone(), entry.tag.clone());
            if self
                .watermarks
                .get(&key)
                .is_some_and(|seen| entry.height <= *seen)
            {
                entry.read = true;
            }
        }
        self.rev += 1;
    }

    /// A `live` entry: into the page, counted, and judged by the device's
    /// settings. The banner to raise, if any — the entry's own, or the
    /// program's "N more" once it is past its burst. An entry under a
    /// view's watermark is already read and raises nothing.
    pub(crate) fn arrived(
        &mut self,
        settings: &Settings,
        mut entry: Entry,
        now: Instant,
    ) -> Option<Notice> {
        let key = (entry.program.clone(), entry.tag.clone());
        if self
            .watermarks
            .get(&key)
            .is_some_and(|seen| entry.height <= *seen)
        {
            entry.read = true;
        }
        if self.entries.iter().any(|known| known.id == entry.id) {
            return None;
        }
        self.unread += u64::from(!entry.read);
        self.rev += 1;
        let program = entry.program.clone();
        let read = entry.read;
        let notice = Notice {
            title: entry.title.clone(),
            body: entry.body.clone(),
            tag: banner_tag(&entry.program, &entry.tag),
        };
        self.entries.insert(0, entry);
        if read {
            return None;
        }
        let permission = settings.programs.get(&program).copied();
        match permission {
            None => {
                self.asking.insert(program);
                return None;
            }
            Some(Permission::Silent) => return None,
            Some(Permission::Allow) => {}
        }
        let in_front = self.front.is_some_and(|(_, focused)| focused == program);
        if !settings.banners || (in_front && !settings.in_front) {
            return None;
        }
        let burst = f64::from(settings.burst.max(1));
        let bucket = self.buckets.entry(program.clone()).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        let refill = now.saturating_duration_since(bucket.at).as_secs_f64() * burst / 60.;
        bucket.tokens = (bucket.tokens + refill).min(burst);
        bucket.at = now;
        if bucket.tokens >= 1. {
            bucket.tokens -= 1.;
            self.more.remove(&program);
            return Some(notice);
        }
        let more = self.more.entry(program.clone()).or_default();
        *more += 1;
        Some(Notice {
            title: format!("{more} more from {program}"),
            body: "They're waiting in Notifications.".into(),
            tag: format!("more/{program}"),
        })
    }

    /// Entries marked read elsewhere (`live` `read`): the page follows.
    /// Whether an id is not on this page, in which case the inbox's count
    /// is read again (`entries`), and the banner tags to take down.
    pub(crate) fn read_elsewhere(&mut self, ids: &[i64]) -> (bool, Vec<String>) {
        let mut unknown = false;
        let mut tags = Vec::new();
        for id in ids {
            match self.entries.iter_mut().find(|entry| entry.id == *id) {
                Some(entry) => {
                    if !entry.read {
                        entry.read = true;
                        self.unread = self.unread.saturating_sub(1);
                        tags.push(banner_tag(&entry.program, &entry.tag));
                    }
                }
                None => unknown = true,
            }
        }
        self.rev += 1;
        (unknown, tags)
    }

    /// Entries retracted by their program (`gone`) or hidden on another
    /// device (`hidden`): off the page. The banner tags to take down.
    pub(crate) fn removed(&mut self, ids: &[i64]) -> Vec<String> {
        let mut tags = Vec::new();
        self.entries.retain(|entry| {
            let gone = ids.contains(&entry.id);
            if gone {
                if !entry.read {
                    self.unread = self.unread.saturating_sub(1);
                }
                tags.push(banner_tag(&entry.program, &entry.tag));
            }
            !gone
        });
        self.rev += 1;
        tags
    }

    /// The page folded by `(program, tag)`, newest first.
    pub(crate) fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        for entry in &self.entries {
            let folded = (!entry.tag.is_empty())
                .then(|| {
                    rows.iter_mut()
                        .find(|row| row.program == entry.program && row.tag == entry.tag)
                })
                .flatten();
            match folded {
                Some(row) => {
                    row.count += 1;
                    row.read &= entry.read;
                    row.through_height = row.through_height.max(entry.height);
                }
                None => rows.push(Row {
                    id: entry.id,
                    program: entry.program.clone(),
                    tag: entry.tag.clone(),
                    title: entry.title.clone(),
                    body: entry.body.clone(),
                    link: entry.link.clone(),
                    at: (entry.at_ms / 1000) as i64,
                    count: 1,
                    read: entry.read,
                    through_height: entry.height,
                }),
            }
        }
        rows
    }

    /// The inbox's unread count; none without an inbox.
    pub(crate) fn unread(&self) -> u64 {
        match self.state {
            InboxState::Off | InboxState::NoInbox | InboxState::Denied(_) => 0,
            _ => self.unread,
        }
    }

    /// How many times the page or its state changed: equal, it lists
    /// what it listed.
    pub(crate) fn rev(&self) -> u64 {
        self.rev
    }

    /// A row picked (in the bell, or its banner clicked): its entries read
    /// through the row's newest height, here and upstream, and the link
    /// minted for this chain; `None` for an id no row has.
    pub(crate) fn open(&mut self, id: i64) -> Option<Opened> {
        let row = self.rows().into_iter().find(|row| row.id == id)?;
        self.mark_read(|entry| {
            entry.program == row.program
                && entry.tag == row.tag
                && entry.height <= row.through_height
        });
        self.send(Call::Read(Read::Tag {
            program: row.program.clone(),
            tag: row.tag.clone(),
            through_height: row.through_height,
        }));
        let segments: Vec<&str> = row.link.segments.iter().map(String::as_str).collect();
        let link = ducklink::mint(&self.network, &row.link.program, &segments);
        Some(Opened { link })
    }

    /// The row a banner's click names: the one its entry folds into.
    pub(crate) fn row_of(&self, entry: i64) -> Option<i64> {
        let entry = self.entries.iter().find(|known| known.id == entry)?;
        self.rows()
            .into_iter()
            .find(|row| row.program == entry.program && row.tag == entry.tag)
            .map(|row| row.id)
    }

    fn mark_read(&mut self, which: impl Fn(&Entry) -> bool) {
        for entry in &mut self.entries {
            if !entry.read && which(entry) {
                entry.read = true;
                self.unread = self.unread.saturating_sub(1);
            }
        }
        self.rev += 1;
    }

    pub(crate) fn mark_all_read(&mut self) {
        self.mark_read(|_| true);
        self.unread = 0;
        self.send(Call::Read(Read::All { all: true }));
    }

    /// Read rows off the page, and hidden on every device.
    pub(crate) fn clear_read(&mut self) {
        self.entries.retain(|entry| !entry.read);
        self.rev += 1;
        self.send(Call::HideRead);
    }

    /// A view says the reader has seen `tag`'s rows through `height`: the
    /// max per `(program, tag)` is kept; the page's entries at or below it
    /// are read, and the read goes upstream, only when the max moved.
    pub(crate) fn watermark(&mut self, program: &str, tag: &str, height: u64) -> bool {
        let key = (program.to_owned(), tag.to_owned());
        if self
            .watermarks
            .get(&key)
            .is_some_and(|seen| height <= *seen)
        {
            return false;
        }
        self.watermarks.insert(key, height);
        self.mark_read(|entry| {
            entry.program == program && entry.tag == tag && entry.height <= height
        });
        self.send(Call::Read(Read::Tag {
            program: program.to_owned(),
            tag: tag.to_owned(),
            through_height: height,
        }));
        true
    }

    /// Whether the inbox keeps `program`'s entries out.
    pub(crate) fn muted(&self, program: &str) -> bool {
        self.mutes.programs.iter().any(|muted| muted == program)
    }

    /// `program` kept out of the inbox (or let back in): the account's
    /// mutes replaced upstream. One race across devices is accepted: the
    /// last `PUT` wins.
    pub(crate) fn set_muted(&mut self, program: &str, muted: bool) {
        self.mutes.programs.retain(|known| known != program);
        if muted {
            self.mutes.programs.push(program.to_owned());
        }
        self.asking.remove(program);
        self.rev += 1;
        self.send(Call::Mutes(self.mutes.clone()));
    }

    /// Entries from `program` this past week, folded ones counted.
    pub(crate) fn this_week(&self, program: &str, wall: i64) -> u32 {
        self.entries
            .iter()
            .filter(|entry| {
                entry.program == program && wall - (entry.at_ms / 1000) as i64 <= 7 * 86_400
            })
            .count() as u32
    }

    /// A window came to the front or left it, or its panes moved
    /// (`WindowRoot`'s activation and `Desk` observers): whether it is in
    /// front, and the view focused in it.
    pub(crate) fn set_front(&mut self, window: WindowKey, active: bool, focused: &'static str) {
        match active {
            true => self.front = Some((window, focused)),
            false if self.front.is_some_and(|(key, _)| key == window) => self.front = None,
            false => {}
        }
    }

    /// `program`'s question at the top of the centre, with nothing on the
    /// page: drawn without an unread count on the bell.
    #[cfg(test)]
    pub(crate) fn ask_for_test(&mut self, program: &str) {
        self.asking.insert(program.to_owned());
    }

    /// A call upstream, on the runtime, after this lock is released; a
    /// failure is logged, the page stands as the person left it. Nothing
    /// is sent without a client (no inbox, or none yet).
    fn send(&self, call: Call) {
        let Some(client) = self.client.clone() else {
            return;
        };
        crate::runtime::handle().spawn(async move {
            let failed = match call {
                Call::Read(read) => client.read(&read).await.err(),
                Call::HideRead => client.hide_read().await.err(),
                Call::Mutes(mutes) => client.set_mutes(&mutes).await.err(),
            };
            if let Some(error) = failed {
                tracing::warn!(target: "ducktape::app", %error, "the inbox did not take a change");
            }
        });
    }
}

enum Call {
    Read(Read),
    HideRead,
    Mutes(Mutes),
}
