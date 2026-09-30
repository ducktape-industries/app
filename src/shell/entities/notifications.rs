//! The notification centre as the bar and the panes draw it: the unread
//! count, the views asking for permission, and the log's revision, so a
//! change that leaves the count alone (clearing read rows) still moves it.
//! Read off the centre by `refresh`: after every dispatch (the bridge,
//! which an `Intent::Notified` reaches too) until s11 gives it the
//! centre's methods.
use crate::runtime::notify::CenterHandle;
use gpui_kit::Context;
use std::collections::BTreeSet;

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
    use std::cell::Cell;
    use std::rc::Rc;

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
        let seen = Rc::new(Cell::new(0));
        let count = seen.clone();
        let _observing = cx.update(|cx| cx.observe(&slice, move |_, _| count.set(count.get() + 1)));
        assert!(!slice.update(cx, |slice, cx| slice.refresh(cx)));
        assert_eq!(seen.get(), 0, "a centre that moved nothing notified");

        // the read row goes: the count and the askers stay as they were
        center.lock().clear_read();
        assert!(slice.update(cx, |slice, cx| slice.refresh(cx)));
        assert_eq!(seen.get(), 1, "the bell's list lost a row and said nothing");
        assert_eq!(slice.read_with(cx, |slice, _| slice.unread), 0);
    }
}
