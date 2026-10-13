//! The notification centre: a view of the inbox at the RPC address. It
//! keeps the page of entries read this session (memory only), the inbox's
//! unread count, the account's mutes, and the banner policy — permission,
//! focus, the per-program token bucket — that decides what an arriving
//! entry becomes on this device. The app keeps one, reached through
//! [`center`]; a test builds its own.
//!
//! What is read: one truth, the watermark. Per `(program, tag)` the centre
//! keeps the height the reader has seen through (a view's `notify.seen`,
//! a row opened); the page's entries at or below it are read and the
//! count moves with them here, and the same height goes upstream as one
//! tag read, so the inbox's `read` push finds nothing left to move. An
//! entry arriving under it is read on arrival and the tag is read upstream
//! again, so every device agrees.
//!
//! Lock order: a seat's mutex, then the centre, then the kernel's in-order
//! queues, then `KEPT` (the settings). The window thread holds a seat while
//! a view's `notify.seen` takes the centre, so the inbox runner must never
//! take a seat (the roster) or read the prefs file while it holds the
//! centre: it looks the label and the settings up first.
//!
//! Nothing here knows a program: `program` and `tag` are the inbox's words.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Instant;

use inbox_api::{Entry, Mutes, Read};

use super::settings::{Permission, Settings};
use crate::backend::inbox::{Client, Error};
use crate::runtime::WindowKey;

/// One banner as the host words it; a later one under the same non-empty
/// `tag` replaces the standing one. `opens` names the entry whose row a
/// click opens; a "N more" banner opens nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notice {
    pub title: String,
    pub body: String,
    pub tag: String,
    pub opens: Option<i64>,
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

type Key = (String, String);

#[derive(Default)]
pub(crate) struct Center {
    /// The chain id (`<network>#<salt>`) the page is for.
    network: String,
    /// The inbox client of the connection, once the runner made one.
    client: Option<Arc<Client>>,
    /// The runner the centre is filled by: a late write from a runner that
    /// was dropped is refused (`stamped`).
    stamp: u64,
    pub(crate) state: InboxState,
    /// The inbox's follower stopped or stalled at a height: nothing past
    /// it arrives until its operator acts. Shown beside the count.
    pub(crate) stuck: Option<inbox_api::Stopped>,
    /// Newest first, as the inbox lists them.
    entries: Vec<Entry>,
    /// The inbox's count, kept current by `live`.
    unread: u64,
    pub(crate) mutes: Mutes,
    /// The inbox's one-sentence refusal of the last mutes `PUT` (over
    /// `MAX_MUTES`); gone with the next mutes the inbox sends.
    pub(crate) refused: Option<String>,
    /// This centre, for the task a call upstream runs as, to write back
    /// what the inbox answered (set by [`CenterHandle::lock`]).
    me: Weak<Mutex<Center>>,
    /// The max `through_height` the reader saw per `(program, tag)`: the
    /// one truth of what is read.
    watermarks: HashMap<Key, u64>,
    /// The height the inbox confirmed per `(program, tag)`: a watermark at
    /// or below it is not sent again.
    confirmed: HashMap<Key, u64>,
    /// Tag reads upstream: at most one in flight per `(program, tag)`
    /// (the inbox paces an account), and the latest height waiting
    /// behind it, replacing any earlier one.
    in_flight: std::collections::HashSet<Key>,
    queued: HashMap<Key, u64>,
    /// Calls the person made while the inbox was not in hand (Mark all
    /// read, Clear read, a mute): sent when it is.
    pending: Vec<Call>,
    /// The banner standing per banner tag, by the entry it is for: it
    /// comes down when that entry is read or gone, and for no other.
    pub(super) standing: HashMap<String, i64>,
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
        let mut center = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if center.me.strong_count() == 0 {
            center.me = Arc::downgrade(&self.0);
        }
        center
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
            self.network = network.to_owned();
            self.forget_account();
        }
    }

    /// The account's runner stopped (the key locked, the network left, a
    /// new address or key): its page, count, mutes, watermarks, questions
    /// and banners go with it. The chain and the window in front stay.
    pub(crate) fn forget_account(&mut self) {
        *self = Center {
            network: std::mem::take(&mut self.network),
            front: self.front,
            me: self.me.clone(),
            stamp: self.stamp,
            rev: self.rev + 1,
            ..Center::default()
        };
    }

    /// A runner takes the centre up: the stamp it writes under. The last
    /// runner's writes are refused from now on.
    pub(crate) fn claim(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    /// Whether `stamp` is the runner the centre is filled by.
    pub(crate) fn stamped(&self, stamp: u64) -> bool {
        self.stamp == stamp
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

    fn under_watermark(&self, entry: &Entry) -> bool {
        self.watermarks
            .get(&(entry.program.clone(), entry.tag.clone()))
            .is_some_and(|seen| entry.height <= *seen)
    }

    /// The page the inbox listed (newest first) and its unread count. The
    /// entries under a kept watermark are read, and counted off, here.
    pub(crate) fn replace(&mut self, entries: Vec<Entry>, unread: u64) {
        self.entries = entries;
        self.unread = unread;
        let listed: Vec<i64> = self.entries.iter().map(|entry| entry.id).collect();
        self.standing.retain(|_, id| listed.contains(id));
        self.mark_read(|center, entry| center.under_watermark(entry));
    }

    /// A `live` entry: into the page, counted, and judged by the device's
    /// settings. The banner to raise, if any — the entry's own, or the
    /// program's "N more" once it is past its burst. An entry under a
    /// view's watermark is read on arrival, raises nothing, and the tag is
    /// read upstream again so the inbox agrees.
    pub(crate) fn arrived(
        &mut self,
        settings: &Settings,
        mut entry: Entry,
        now: Instant,
        label: &str,
    ) -> Option<Notice> {
        if self.entries.iter().any(|known| known.id == entry.id) {
            return None;
        }
        if !entry.read && self.under_watermark(&entry) {
            entry.read = true;
            let key = (entry.program.clone(), entry.tag.clone());
            let seen = self.watermarks[&key];
            self.confirmed.remove(&key);
            self.read_tag(&entry.program, &entry.tag, seen);
        }
        self.unread += u64::from(!entry.read);
        self.rev += 1;
        let program = entry.program.clone();
        let read = entry.read;
        let notice = Notice {
            title: entry.title.clone(),
            body: entry.body.clone(),
            tag: banner_tag(&entry.program, &entry.tag),
            opens: Some(entry.id),
        };
        let in_front = self
            .front
            .is_some_and(|(_, focused)| focused == program || focused == entry.link.program);
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
            if !notice.tag.is_empty() {
                self.standing
                    .insert(notice.tag.clone(), notice.opens.unwrap_or_default());
            }
            return Some(notice);
        }
        let more = self.more.entry(program.clone()).or_default();
        *more += 1;
        Some(Notice {
            title: format!("{more} more from {label}"),
            body: "They're waiting in Notifications.".into(),
            tag: format!("more/{program}"),
            opens: None,
        })
    }

    /// Entries marked read elsewhere (`live` `read`): the page follows.
    /// Whether an id is not on this page, in which case the inbox's count
    /// is read again (`entries`).
    pub(crate) fn read_elsewhere(&mut self, ids: &[i64]) -> bool {
        let unknown = ids
            .iter()
            .any(|id| !self.entries.iter().any(|entry| entry.id == *id));
        self.mark_read(|_, entry| ids.contains(&entry.id));
        unknown
    }

    /// Entries retracted by their program (`gone`) or hidden on another
    /// device (`hidden`): off the page, and their banners down.
    pub(crate) fn removed(&mut self, ids: &[i64]) {
        let mut unread = self.unread;
        self.entries.retain(|entry| {
            let gone = ids.contains(&entry.id);
            if gone && !entry.read {
                unread = unread.saturating_sub(1);
            }
            !gone
        });
        self.unread = unread;
        for id in ids {
            self.take_down(*id);
        }
        self.rev += 1;
    }

    /// The banner standing for `id`, if any, comes down.
    fn take_down(&mut self, id: i64) {
        let Some(tag) = self
            .standing
            .iter()
            .find_map(|(tag, standing)| (*standing == id).then(|| tag.clone()))
        else {
            return;
        };
        self.standing.remove(&tag);
        super::withdraw(tag);
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

    /// A row picked (in the bell, or its banner clicked): a watermark at
    /// the row's newest height, so its entries are read here and upstream
    /// (kept until the inbox has it), and the link minted for this chain;
    /// `None` for an id no row has.
    pub(crate) fn open(&mut self, id: i64) -> Option<Opened> {
        let row = self.rows().into_iter().find(|row| row.id == id)?;
        self.watermark(&row.program, &row.tag, row.through_height);
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

    /// The unread entries `which` picks become read: the count moves with
    /// each, and the banner standing for it comes down.
    fn mark_read(&mut self, which: impl Fn(&Center, &Entry) -> bool) {
        let flipped: Vec<i64> = self
            .entries
            .iter()
            .filter(|entry| !entry.read && which(self, entry))
            .map(|entry| entry.id)
            .collect();
        for entry in &mut self.entries {
            if flipped.contains(&entry.id) {
                entry.read = true;
                self.unread = self.unread.saturating_sub(1);
            }
        }
        for id in flipped {
            self.take_down(id);
        }
        self.rev += 1;
    }

    pub(crate) fn mark_all_read(&mut self) {
        self.mark_read(|_, _| true);
        self.unread = 0;
        self.send(Call::Read(Read::All { all: true }));
    }

    /// Read rows off the page, and hidden on every device.
    pub(crate) fn clear_read(&mut self) {
        self.entries.retain(|entry| !entry.read);
        self.rev += 1;
        self.send(Call::HideRead);
    }

    /// The reader has seen `tag`'s rows through `height`: the max per
    /// `(program, tag)` is kept; the page's entries at or below it are
    /// read, and the read goes upstream, only when the max moved.
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
        self.mark_read(|_, entry| {
            entry.program == program && entry.tag == tag && entry.height <= height
        });
        self.read_tag(program, tag, height);
        true
    }

    /// `read {program, tag, through_height}` upstream, one in flight per
    /// `(program, tag)`: while one is out, the newest height waits and
    /// goes when it answers (a scrolling view's burst is two requests).
    fn read_tag(&mut self, program: &str, tag: &str, height: u64) {
        let key = (program.to_owned(), tag.to_owned());
        if self.in_flight.contains(&key) {
            self.queued.insert(key, height);
            return;
        }
        let sent = self.send(Call::Read(Read::Tag {
            program: program.to_owned(),
            tag: tag.to_owned(),
            through_height: height,
        }));
        if sent {
            self.in_flight.insert(key);
        }
        // not sent (no client yet, or the inbox away): the kept watermark
        // goes out on the next connect (`resend_watermarks`)
    }

    /// A connection made (or made again): every watermark the inbox has
    /// not confirmed goes upstream once more, one in flight per tag.
    /// Covers a view's `notify.seen` before the first page, a row opened
    /// while the inbox was away, and a request that never answered.
    pub(crate) fn resend_watermarks(&mut self) {
        self.in_flight.clear();
        self.queued.clear();
        let kept: Vec<(Key, u64)> = self
            .watermarks
            .iter()
            .filter(|(key, height)| self.confirmed.get(*key).is_none_or(|done| done < height))
            .map(|(key, height)| (key.clone(), *height))
            .collect();
        for ((program, tag), height) in kept {
            self.read_tag(&program, &tag, height);
        }
        for call in std::mem::take(&mut self.pending) {
            self.send(call);
        }
    }

    /// A tag read answered (`confirmed` through the height) or failed:
    /// the next queued height goes out.
    fn read_tag_done(&mut self, program: &str, tag: &str, confirmed: Option<u64>) {
        let key = (program.to_owned(), tag.to_owned());
        if let Some(height) = confirmed {
            let done = self.confirmed.entry(key.clone()).or_default();
            *done = (*done).max(height);
        }
        self.in_flight.remove(&key);
        if let Some(height) = self.queued.remove(&key) {
            self.read_tag(program, tag, height);
        }
    }

    /// The mutes as the inbox keeps them (a `Prefs` push, or the answer
    /// to a refused `PUT`): the last refusal is over.
    pub(crate) fn set_mutes(&mut self, mutes: Mutes) {
        self.mutes = mutes;
        self.refused = None;
        self.rev += 1;
    }

    /// Whether the inbox keeps `program`'s entries out.
    pub(crate) fn muted(&self, program: &str) -> bool {
        self.mutes.programs.iter().any(|muted| muted == program)
    }

    /// `program` kept out of the inbox (or let back in): the account's
    /// mutes replaced upstream. One race across devices is accepted: the
    /// last `PUT` wins.
    pub(crate) fn set_muted(&mut self, program: &str, muted: bool) {
        if program.len() > notices::MAX_LINK_PROGRAM_BYTES {
            // the inbox's cap (the notices' own): refused here, in a sentence
            self.refused = Some(format!(
                "a muted program's name is at most {} bytes",
                notices::MAX_LINK_PROGRAM_BYTES
            ));
            self.rev += 1;
            return;
        }
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

    /// The calls waiting for the inbox, for a test.
    #[cfg(test)]
    pub(crate) fn pending_for_test(&self) -> usize {
        self.pending.len()
    }

    /// A call upstream, on the runtime, after this lock is released; a
    /// failure is logged by kind. Without the inbox in hand (none yet, or
    /// away) nothing is sent, `false`: a tag read is covered by its kept
    /// watermark, any other call waits for the next connect. A call the
    /// inbox did not take waits too.
    fn send(&mut self, call: Call) -> bool {
        let client = match (&self.client, &self.state) {
            (Some(client), InboxState::Ready) => client.clone(),
            _ => {
                self.keep(call);
                return false;
            }
        };
        let me = self.me.clone();
        crate::runtime::handle().spawn(async move {
            let failed = match &call {
                Call::Read(read) => {
                    let answer = client.read(read).await;
                    if let Read::Tag {
                        program,
                        tag,
                        through_height,
                    } = read
                        && let Some(center) = me.upgrade()
                    {
                        let mut center = center
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        // not for a later runner's account
                        if center
                            .client
                            .as_ref()
                            .is_some_and(|now| Arc::ptr_eq(now, &client))
                        {
                            center.read_tag_done(
                                program,
                                tag,
                                answer.is_ok().then_some(*through_height),
                            );
                        }
                    }
                    answer.err()
                }
                Call::HideRead => client.hide_read().await.err(),
                Call::Mutes(mutes) => match client.set_mutes(mutes).await {
                    // the stored set comes back over `live` as well
                    Ok(_) => None,
                    Err(Error::Failed {
                        status: 400,
                        sentence,
                    }) => {
                        // refused (over MAX_MUTES): said, and the mutes put
                        // back as the inbox keeps them
                        let kept = client.mutes().await.ok();
                        if let Some(center) = me.upgrade() {
                            let mut center = center
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            if let Some(kept) = kept {
                                center.set_mutes(kept);
                            }
                            center.refused = Some(sentence);
                            center.rev += 1;
                        }
                        super::wake();
                        None
                    }
                    Err(error) => Some(error),
                },
            };
            if let Some(error) = failed {
                tracing::warn!(target: "ducktape::app", kind = %error.kind(), "the inbox did not take a change");
                if matches!(error, Error::Unreachable(_))
                    && let Some(center) = me.upgrade()
                {
                    // the inbox went away under the call: it goes again
                    // with the next connect
                    center
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .keep(call);
                }
            }
        });
        true
    }

    /// `call` waits for the next connect; a tag read does not (its
    /// watermark is kept), and one mutes set replaces another.
    fn keep(&mut self, call: Call) {
        match call {
            Call::Read(Read::Tag { .. }) => {}
            Call::Mutes(_) => {
                self.pending.retain(|kept| !matches!(kept, Call::Mutes(_)));
                self.pending.push(call);
            }
            call => self.pending.push(call),
        }
    }
}

enum Call {
    Read(Read),
    HideRead,
    Mutes(Mutes),
}
