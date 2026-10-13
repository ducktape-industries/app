//! The notification centre as the bar and the panes draw it: the unread
//! count, the inbox's state, the programs asking for a word on their
//! banners, and the page's revision, so a change that leaves the count
//! alone (clearing read rows) still moves it. Read off the centre by
//! `refresh`: after each of the centre's methods here, on the inbox
//! runner's wake (`Windows::posted`), on a view's `Intent::Notified`
//! (`Seats`), and as the session connects (`follow`).
//!
//! The inbox runner is started here, once the network is in hand and the
//! seated key holds an account (the login signs with it), and dropped when
//! the network is left or the key goes.
use super::{Account, Prefs, Session, SessionEvent, Slice};
use crate::runtime::notify::{self, CenterHandle, Follower, InboxState, Opened, Permission, Row};
use gpui_kit::{App, Context, Entity};
use std::collections::BTreeSet;

/// Timed as `reducer.notify`, the name qa's hang rule reads (docs/perf.md).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.notify")
}

/// The person answered `program`'s question: the word is saved and its
/// question goes. Two entities move: the centre (`Notifications`) and the
/// saved settings (`Prefs`, read off disk again), so the answer is one
/// call here.
pub(crate) fn permission(
    notifications: &Entity<Notifications>,
    prefs: &Entity<Slice<Prefs>>,
    program: &str,
    permission: Permission,
    cx: &mut App,
) {
    notifications.update(cx, |notifications, cx| {
        notifications.permission(program, permission, cx)
    });
    prefs.update(cx, |prefs, cx| prefs.reload_notify(cx));
}

pub(crate) struct Notifications {
    unread: u64,
    asking: BTreeSet<String>,
    state: InboxState,
    /// The inbox's refusal of the last mutes change, for Settings.
    refused: Option<String>,
    entries_rev: u64,
    /// What it is read off; not compared.
    center: CenterHandle,
    /// The inbox runner of the connection in hand, and what it runs for.
    follower: Option<(String, Follower)>,
}

impl Notifications {
    pub(crate) fn new(center: CenterHandle) -> Self {
        let (unread, asking, state, refused, entries_rev) = Self::read(&center);
        Self {
            unread,
            asking,
            state,
            refused,
            entries_rev,
            center,
            follower: None,
        }
    }

    #[allow(clippy::type_complexity, reason = "the compared tuple")]
    fn read(center: &CenterHandle) -> (u64, BTreeSet<String>, InboxState, Option<String>, u64) {
        let center = center.lock();
        (
            center.unread(),
            center.asking.clone(),
            center.state.clone(),
            center.refused.clone(),
            center.rev(),
        )
    }

    /// The inbox's one-sentence refusal of the last mutes change, if any.
    pub(crate) fn refused(&self) -> Option<&str> {
        self.refused.as_deref()
    }

    /// The session connecting (the centre has just taken up that chain)
    /// and the account resolving (the key the login signs with): the
    /// runner starts once both are in hand, and stops with the network.
    pub(crate) fn follow(
        &self,
        session: &Entity<Session>,
        account: &Entity<Account>,
        cx: &mut Context<Self>,
    ) {
        let (watched_session, watched_account) = (session.clone(), account.clone());
        cx.subscribe(
            session,
            move |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Connected => {
                    this.sync_follower(&watched_session, &watched_account, cx);
                    this.refresh(cx);
                }
                SessionEvent::LeftNetwork => this.stop(cx),
                SessionEvent::Toast(_) => {}
            },
        )
        .detach();
        let (watched_session, watched_account) = (session.clone(), account.clone());
        cx.observe(account, move |this, _, cx| {
            this.sync_follower(&watched_session, &watched_account, cx)
        })
        .detach();
    }

    /// The runner for the connection in hand, started when the network is
    /// in hand and the seated key holds an account, left alone while it
    /// already runs for that address, dropped when either goes.
    fn sync_follower(
        &mut self,
        session: &Entity<Session>,
        account: &Entity<Account>,
        cx: &mut Context<Self>,
    ) {
        let session = session.read(cx).get();
        let account = account.read(cx).get();
        let ready = session.connected
            && !session.connected_rpc.is_empty()
            && !account.signer_key.is_empty()
            && matches!(account.account, Some(Some(_)));
        let key = format!(
            "{}|{}|{}",
            session.connected_rpc, session.chain, account.signer_key
        );
        if !ready {
            self.stop(cx);
            return;
        }
        if self
            .follower
            .as_ref()
            .is_some_and(|(running, follower)| *running == key && !follower.ended())
        {
            return;
        }
        self.follower = None;
        let rpc = crate::backend::RpcClient::new(session.connected_rpc.clone());
        let follower = notify::start(
            &rpc,
            &session.chain,
            crate::backend::identity::DEVICE_KEY_LABEL,
            self.center.clone(),
        );
        tracing::info!(target: "ducktape::app", chain = %session.chain, "inbox_follow");
        self.follower = Some((key, follower));
        self.refresh(cx);
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.follower.take().is_some() {
            self.refresh(cx);
        }
    }

    /// The centre itself, for whoever tells it which window is in front.
    pub(crate) fn center(&self) -> &CenterHandle {
        &self.center
    }

    pub(crate) fn unread(&self) -> u64 {
        self.unread
    }

    /// Where the inbox stands, as the bell says it.
    pub(crate) fn state(&self) -> &InboxState {
        &self.state
    }

    /// Whether the inbox is in hand: what a change upstream (a mute, a
    /// clear) is offered on.
    pub(crate) fn ready(&self) -> bool {
        self.state == InboxState::Ready
    }

    /// The inbox's follower stopped or stalled: "inbox stuck at 1200: a hole".
    pub(crate) fn stuck(&self) -> Option<String> {
        self.center
            .lock()
            .stuck
            .as_ref()
            .map(|stuck| format!("inbox stuck at {}: {}", stuck.height, stuck.reason))
    }

    /// The programs waiting for a word on their banners.
    pub(crate) fn asking(&self) -> &BTreeSet<String> {
        &self.asking
    }

    /// The page's rows, folded, newest first, as the bell lists them.
    pub(crate) fn entries(&self) -> Vec<Row> {
        self.center.lock().rows()
    }

    /// Whether the inbox keeps `program` out (Settings' Notifications page).
    pub(crate) fn muted(&self, program: &str) -> bool {
        self.center.lock().muted(program)
    }

    /// The programs the inbox keeps out.
    pub(crate) fn muted_programs(&self) -> Vec<String> {
        self.center.lock().mutes.programs.clone()
    }

    /// How many entries `program` had this week, as of the wall second
    /// `wall` (Settings' Notifications page).
    pub(crate) fn this_week(&self, program: &str, wall: i64) -> u32 {
        self.center.lock().this_week(program, wall)
    }

    /// A row picked in the bell: read through its newest height, here and
    /// upstream, and handed back with its link minted
    /// (`Windows::open_notice`).
    pub(crate) fn open(&mut self, id: i64, cx: &mut Context<Self>) -> Option<Opened> {
        let _timed = timed();
        let opened = self.center.lock().open(id);
        self.refresh(cx);
        opened
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

    /// The person's word on `program`'s banners, saved (see [`permission`]).
    fn permission(&mut self, program: &str, permission: Permission, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::set_permission(program, permission);
        self.center.lock().asking.remove(program);
        self.refresh(cx);
    }

    /// `program` kept out of the inbox, or let back in: the account's
    /// mutes, on every device.
    pub(crate) fn set_muted(&mut self, program: &str, muted: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        self.center.lock().set_muted(program, muted);
        self.refresh(cx);
    }

    /// The programs asking, put back as they stood (the door's walk,
    /// `layers::Kept::restore`).
    #[cfg(any(test, feature = "ax-door"))]
    pub(crate) fn restore_asking(&mut self, asking: BTreeSet<String>, cx: &mut Context<Self>) {
        self.center.lock().asking = asking;
        self.refresh(cx);
    }

    /// Read off the centre again; notifies only when something moved.
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let (unread, asking, state, refused, entries_rev) = Self::read(&self.center);
        if (unread, &asking, &state, &refused, entries_rev)
            == (
                self.unread,
                &self.asking,
                &self.state,
                &self.refused,
                self.entries_rev,
            )
        {
            return false;
        }
        (
            self.unread,
            self.asking,
            self.state,
            self.refused,
            self.entries_rev,
        ) = (unread, asking, state, refused, entries_rev);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::notify::tests::entry;
    use gpui_kit::{AppContext as _, TestAppContext};

    #[gpui_kit::test]
    fn clear_read_moves_the_notifications_slice(cx: &mut TestAppContext) {
        let center = CenterHandle::default();
        center.lock().set_state(InboxState::Ready);
        center.lock().replace(vec![entry(1, "chat", "", 1)], 1);
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

    /// A connect reads the centre again as the session says it connected,
    /// not at the next entry; the runner waits for a key with an account.
    #[gpui_kit::test]
    fn a_connect_reads_the_centre_it_took_up(cx: &mut TestAppContext) {
        let center = CenterHandle::default();
        let app = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::shell::entities::Entities::for_test(Default::default(), center.clone(), cx)
        });
        center.lock().set_state(InboxState::Ready);
        center.lock().replace(vec![entry(1, "chat", "", 1)], 1);
        let unread = |cx: &mut TestAppContext| {
            app.notifications
                .read_with(cx, |notifications, _| notifications.unread())
        };
        assert_eq!(unread(cx), 0);
        app.session
            .update(cx, |_, cx| cx.emit(SessionEvent::Connected));
        assert_eq!(
            unread(cx),
            1,
            "the bell kept the count from before the connect"
        );
        assert!(
            app.notifications
                .read_with(cx, |notifications, _| notifications.follower.is_none()),
            "no key with an account: no runner"
        );
    }

    /// A runner that ended (a refused login) is started again by the next
    /// connect to the same address, not kept as it is.
    #[gpui_kit::test]
    fn a_connect_restarts_a_runner_that_ended(cx: &mut TestAppContext) {
        let handle = crate::runtime::handle();
        let _seat = handle.block_on(crate::runtime::notify::tests::seated(32));
        let fake = handle.block_on(crate::backend::inbox::fake::FakeInbox::start());
        fake.deny_logins(true);
        let center = CenterHandle::default();
        let app = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::shell::entities::Entities::for_test(Default::default(), center.clone(), cx)
        });
        app.session.update(cx, |session, cx| {
            let mut state = crate::shell::entities::SessionState::booted();
            state.connected = true;
            state.connected_rpc = fake.origin();
            state.chain = "net#0".into();
            session.seed(state, cx);
        });
        app.account.update(cx, |account, cx| {
            let state = crate::shell::entities::AccountState {
                signer_key: "ab".into(),
                account: Some(Some((7, "grace".into()))),
                ..Default::default()
            };
            account.seed(state, None, None, cx);
        });
        let wait = |what: &dyn Fn() -> bool| {
            for _ in 0..400 {
                if what() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            panic!("the runner never got there");
        };
        app.session
            .update(cx, |_, cx| cx.emit(SessionEvent::Connected));
        wait(&|| fake.logins() == 1);
        let ended = || {
            app.notifications.read_with(cx, |notifications, _| {
                notifications
                    .follower
                    .as_ref()
                    .is_some_and(|(_, follower)| follower.ended())
            })
        };
        wait(&|| ended());
        app.session
            .update(cx, |_, cx| cx.emit(SessionEvent::Connected));
        wait(&|| fake.logins() == 2);
    }
}
