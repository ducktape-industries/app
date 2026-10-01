//! The notification centre as the bar and the panes draw it: the unread
//! count, the views asking for permission, and the log's revision, so a
//! change that leaves the count alone (clearing read rows) still moves it.
//! Read off the centre by `refresh`: after each of the centre's methods
//! here, on a view's `Intent::Notified` (`Seats`), and after a banner's
//! click (`Windows`).
use super::{Prefs, Slice};
use crate::runtime::notify::{self, CenterHandle, Entry, Permission};
use gpui_kit::{App, Context, Entity};
use std::collections::BTreeSet;

/// Timed as `reducer.notify`, the name qa's hang rule reads (docs/perf.md).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.notify")
}

/// The person answered `module`'s ask: the word is saved and its bar goes.
/// Two entities move: the centre (`Notifications`) and the saved settings
/// (`Prefs`, read off disk again), so the answer is one call here.
pub(crate) fn permission(
    notifications: &Entity<Notifications>,
    prefs: &Entity<Slice<Prefs>>,
    module: &str,
    permission: Permission,
    cx: &mut App,
) {
    notifications.update(cx, |notifications, cx| {
        notifications.permission(module, permission, cx)
    });
    prefs.update(cx, |prefs, cx| prefs.reload_notify(cx));
}

pub(crate) struct Notifications {
    unread: usize,
    asking: BTreeSet<String>,
    entries_rev: u64,
    /// What it is read off; not compared.
    center: CenterHandle,
}

impl Notifications {
    pub(crate) fn new(center: CenterHandle) -> Self {
        let (unread, asking, entries_rev) = Self::read(&center);
        Self {
            unread,
            asking,
            entries_rev,
            center,
        }
    }

    fn read(center: &CenterHandle) -> (usize, BTreeSet<String>, u64) {
        let center = center.lock();
        (center.unread(), center.asking.clone(), center.rev())
    }

    /// The centre itself, for whoever tells it which window is in front.
    pub(crate) fn center(&self) -> &CenterHandle {
        &self.center
    }

    pub(crate) fn unread(&self) -> usize {
        self.unread
    }

    /// The views waiting for a word on their notices.
    pub(crate) fn asking(&self, module: &str) -> bool {
        self.asking.contains(module)
    }

    /// The log's rows, newest first, as the bell lists them.
    pub(crate) fn entries(&self) -> Vec<crate::runtime::notify::Entry> {
        self.center.lock().entries().cloned().collect()
    }

    /// How many notices `module` posted this week, as of the wall second
    /// `wall` (Settings' Notifications page).
    pub(crate) fn this_week(&self, module: &str, wall: i64) -> u32 {
        self.center.lock().this_week(module, wall)
    }

    /// A row picked in the bell: read, and handed back for the link or
    /// the seat it is about to open (`Windows::open_notice`).
    pub(crate) fn open(&mut self, id: u64, cx: &mut Context<Self>) -> Option<Entry> {
        let _timed = timed();
        let entry = self.center.lock().open(id);
        self.refresh(cx);
        entry
    }

    pub(crate) fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.center.lock().mark_all_read();
        self.refresh(cx);
    }

    pub(crate) fn clear_read(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.center.lock().clear_read();
        self.refresh(cx);
    }

    /// The person's word on `module`'s notices, saved (see [`permission`]).
    fn permission(&mut self, module: &str, permission: Permission, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::set_permission(&mut self.center.lock(), module, permission);
        self.refresh(cx);
    }

    /// "Not now": the bar goes for this run; the view asks again next launch.
    pub(crate) fn not_now(&mut self, module: &str, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::not_now(&mut self.center.lock(), module);
        self.refresh(cx);
    }

    /// The views asking, put back as they stood (the door's walk,
    /// `layers::Kept::restore`).
    pub(crate) fn restore_asking(&mut self, asking: BTreeSet<String>, cx: &mut Context<Self>) {
        self.center.lock().asking = asking;
        self.refresh(cx);
    }

    /// Read off the centre again; notifies only when something moved.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let (unread, asking, entries_rev) = Self::read(&self.center);
        if (unread, &asking, entries_rev) == (self.unread, &self.asking, self.entries_rev) {
            return false;
        }
        (self.unread, self.asking, self.entries_rev) = (unread, asking, entries_rev);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::notify::Settings;
    use gpui_kit::{AppContext as _, TestAppContext};

    #[gpui_kit::test]
    fn clear_read_moves_the_notifications_slice(cx: &mut TestAppContext) {
        let center = CenterHandle::default();
        let settings = Settings {
            banners: true,
            in_front: false,
            burst: 3,
            views: Default::default(),
        };
        let post = view_wire::methods::Notification {
            title: "a".into(),
            body: "b".into(),
            tag: String::new(),
            link: String::new(),
        };
        let now = std::time::Instant::now();
        let _ = center.lock().post(&settings, "chat", "Chat", post, now, 0);
        center.lock().mark_all_read();
        let slice = cx.new(|_| Notifications::new(center.clone()));
        let (seen, _observing) = crate::shell::entities::tests::notifies(&slice, cx);
        assert!(!slice.update(cx, |slice, cx| slice.refresh(cx)));
        assert_eq!(seen.get(), 0, "a centre that moved nothing notified");

        // the read row goes: the count and the askers stay as they were
        center.lock().clear_read();
        assert!(slice.update(cx, |slice, cx| slice.refresh(cx)));
        assert_eq!(seen.get(), 1, "the bell's list lost a row and said nothing");
        assert_eq!(slice.read_with(cx, |slice, _| slice.unread), 0);
    }
}
