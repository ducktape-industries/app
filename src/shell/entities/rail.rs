//! The programs the bar lists, each by the name its view gives itself, and
//! each view's unread count. The rows are read off the roster once per
//! change a loader thread, a seat coming or going, or a retry reports on the
//! roster's channel, never at a draw; the badges come from each view's
//! `Intent::Badge` (`Seats`), and go when the network is left.
use super::{Session, SessionEvent};
use crate::runtime::{Link, RailRow, Roster};
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::{Context, Entity, Subscription, Task};
use std::collections::BTreeMap;

pub(crate) struct Rail {
    rows: Vec<RailRow>,
    badges: BTreeMap<&'static str, i64>,
    /// What the rows are read off; not compared.
    roster: Roster,
    _drain: Task<()>,
    _session: Subscription,
}

impl Rail {
    /// The rail of `roster`, read now and again on every message `changes`
    /// brings (a burst read once); its badges cleared as the network in
    /// hand is left.
    pub(crate) fn new(
        roster: Roster,
        mut changes: UnboundedReceiver<()>,
        session: &Entity<Session>,
        cx: &mut Context<Self>,
    ) -> Self {
        let leaving = cx.subscribe(session, |this, _, event: &SessionEvent, cx| {
            if let SessionEvent::LeftNetwork = event {
                this.set_badges(BTreeMap::new(), cx);
            }
        });
        let drain = cx.spawn(async move |this, cx| {
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                if this.update(cx, |rail, cx| rail.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            rows: roster.rail(),
            badges: BTreeMap::new(),
            roster,
            _drain: drain,
            _session: leaving,
        }
    }

    /// A link read against the programs the node lists (`Roster::parse_link`).
    pub(crate) fn parse_link(&self, link: &str) -> Link {
        self.roster.parse_link(link)
    }

    /// Whether the node lists `module`.
    pub(crate) fn lists(&self, module: &str) -> bool {
        self.roster.lists(module)
    }

    pub(crate) fn rows(&self) -> &[RailRow] {
        &self.rows
    }

    /// Each view's unread count.
    pub(crate) fn badges(&self) -> &BTreeMap<&'static str, i64> {
        &self.badges
    }

    /// `module`'s unread count; 0 with none.
    pub(crate) fn badge(&self, module: &str) -> i64 {
        self.badges.get(module).copied().unwrap_or(0)
    }

    /// The rows read off the roster again; notifies only when they moved.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let rows = self.roster.rail();
        if rows == self.rows {
            return false;
        }
        self.rows = rows;
        cx.notify();
        true
    }

    /// The rows of `roster` from now on: a test that swaps the model's.
    #[cfg(test)]
    pub(crate) fn read_off(&mut self, roster: Roster, cx: &mut Context<Self>) {
        self.roster = roster;
        self.refresh(cx);
    }

    /// `module`'s unread count (`host.badge`); 0 or less takes it off.
    /// Notifies only when it moved.
    pub(crate) fn set_badge(&mut self, module: &'static str, count: i64, cx: &mut Context<Self>) {
        let was = match count > 0 {
            true => self.badges.insert(module, count),
            false => self.badges.remove(module),
        };
        if was != (count > 0).then_some(count) {
            cx.notify();
        }
    }

    /// Each view's unread count; notifies only when one moved.
    fn set_badges(&mut self, badges: BTreeMap<&'static str, i64>, cx: &mut Context<Self>) -> bool {
        if badges == self.badges {
            return false;
        }
        self.badges = badges;
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};

    #[gpui_kit::test]
    fn a_roster_change_recomputes_the_rail_once_and_notifies_only_on_change(
        cx: &mut TestAppContext,
    ) {
        const MODULE: &str = "rail-drain-view";
        let roster = Roster::listing(&[MODULE]);
        let (send, changes) = futures::channel::mpsc::unbounded();
        let session = cx.update(crate::shell::entities::tests::session);
        let rail = cx.new(|cx| Rail::new(roster.clone(), changes, &session, cx));
        let (seen, _observing) = crate::shell::entities::tests::notifies(&rail, cx);
        let note = |cx: &mut TestAppContext| rail.read_with(cx, |rail, _| rail.rows()[0].note);
        assert_eq!(note(cx), Some("Loading"));

        // the view landed on a loader thread, which says so on the channel
        crate::runtime::seat_for_test(MODULE, 400);
        send.unbounded_send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(seen.get(), 1, "the rail moved and said nothing");
        assert_eq!(note(cx), None, "the rail still says the view loads");
        assert!(rail.read_with(cx, |rail, _| rail.rows() == roster.rail()));

        // a change that leaves the rows as they are, twice: read, not told
        send.unbounded_send(()).unwrap();
        send.unbounded_send(()).unwrap();
        cx.run_until_parked();
        assert_eq!(seen.get(), 1, "rows that did not move notified");
    }
}
