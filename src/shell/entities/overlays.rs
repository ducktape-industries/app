//! What is open over one window's desk, and the words for it: the dialogs
//! on a scrim (Spotlight, Settings, Approve), the network switcher and the
//! bar's menus. Only the console's opens anything; a pop-out's stays shut.
//! Its methods are the one writer; it closes itself when the network is
//! left (its `Session` observer).
use super::{Session, SessionState};
use gpui_kit::{Context, Entity, Subscription};

/// One thing open over the desk. Settings carries its page, so a page
/// click moves this, and a keystroke in Spotlight does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The command palette, ⌘K.
    Spotlight,
    /// "Add a device…".
    Approve,
    Settings(SettingsPage),
    /// The network switcher.
    Network,
    /// A menu off the bar.
    Menu(Popover),
}

/// A menu hanging off the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Popover {
    /// Node status, off the breathing dot.
    Node,
    /// The account's name: who is signed in, and Lock.
    Account,
    /// The notification centre, off the bell.
    Notifications,
}

/// The Settings window's sections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    #[default]
    Appearance,
    Notifications,
    Networks,
    About,
}

/// What a Spotlight row does when picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    Open(&'static str),
    Switch(String),
    Settings,
    CreateAccount,
    Lock,
    Appearance(crate::backend::Appearance),
    OtherNetwork,
    Help,
    /// The focused window fills the desk (⌘⇧↩).
    FillWindow,
    /// The arrows move and size the focused window (⌘⇧M).
    HoldWindow,
}

/// One Spotlight row, under its group's heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpotRow {
    pub(crate) group: &'static str,
    pub(crate) title: String,
    pub(crate) meta: String,
    pub(crate) spot: Spot,
}

/// What is open over one window's desk: one thing at a time. While it is,
/// the desk's shortcuts are off and Escape closes it (shell/keys.rs).
pub(crate) struct Overlays {
    open: Option<Overlay>,
    /// Settings' page, kept while it is closed: it opens there again. Not
    /// compared (`open` carries the page while it shows).
    page: SettingsPage,
    /// The open menu closed because the keys left it: they stay where they
    /// went instead of going back to what had them before it opened. Not
    /// compared; the overlay layer's handoff reads and clears it, and
    /// anything opening clears it.
    left_by_keys: bool,
    _session: Subscription,
}

impl Overlays {
    /// Nothing open, closing again whenever the network in hand is left:
    /// the connection dropped, or another network (or another chain of the
    /// same name) was taken up.
    pub(crate) fn new(session: &Entity<Session>, cx: &mut Context<Self>) -> Self {
        let on = |session: &SessionState| {
            (
                session.connected,
                session.network.clone(),
                session.chain.clone(),
            )
        };
        let mut was = on(session.read(cx).get());
        let observing = cx.observe(session, move |this: &mut Self, session, cx| {
            let now = on(session.read(cx).get());
            let (connected, network, chain) = std::mem::replace(&mut was, now.clone());
            let left = (connected && !now.0) || network != now.1 || chain != now.2;
            if left {
                this.set(None, cx);
            }
        });
        Self {
            open: None,
            page: SettingsPage::default(),
            left_by_keys: false,
            _session: observing,
        }
    }

    pub(crate) fn get(&self) -> &Option<Overlay> {
        &self.open
    }

    /// Takes `open`; notifies when it moved. Anything opening forgets that
    /// the keys left the last menu.
    fn set(&mut self, open: Option<Overlay>, cx: &mut Context<Self>) {
        if open.is_some() {
            self.left_by_keys = false;
        }
        if self.open == open {
            return;
        }
        if let Some(Overlay::Settings(page)) = open {
            self.page = page;
        }
        self.open = open;
        cx.notify();
    }

    /// `overlay` open, or closed if it was the one open.
    pub(crate) fn toggle(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        let open = (self.open != Some(overlay)).then_some(overlay);
        self.set(open, cx);
    }

    /// `overlay` open, in place of whatever was.
    pub(crate) fn open(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        self.set(Some(overlay), cx);
    }

    /// Settings, on the page it last showed.
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.open(Overlay::Settings(self.page), cx);
    }

    /// `overlay` closed, if it is the one open: Settings on any page, and
    /// for `Menu(_)` whichever menu off the bar.
    pub(crate) fn close(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        let open = match (self.open, overlay) {
            (Some(Overlay::Settings(_)), Overlay::Settings(_))
            | (Some(Overlay::Menu(_)), Overlay::Menu(_)) => true,
            (open, overlay) => open == Some(overlay),
        };
        if open {
            self.set(None, cx);
        }
    }

    /// The keys left the menu `menu` (Tab past its ends, anything else that
    /// took them out): it closes, and they stay where they went. Nothing
    /// unless `menu` is the one open: every close moves the keys out of a
    /// menu, and the bell's Settings row opened Settings as it went.
    pub(crate) fn close_by_keys(&mut self, menu: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.open == Some(menu) {
            self.set(None, cx);
            self.left_by_keys = true;
        }
    }

    /// Whether the last menu closed because the keys left it, forgotten as
    /// it is read.
    pub(crate) fn take_left_by_keys(&mut self) -> bool {
        std::mem::take(&mut self.left_by_keys)
    }

    /// Spotlight's pick run (Enter, a row's click): with one picked
    /// Spotlight closes and hands it back to be run; with none it stays.
    pub(crate) fn submit(&mut self, picked: Option<Spot>, cx: &mut Context<Self>) -> Option<Spot> {
        if picked.is_some() {
            self.close(Overlay::Spotlight, cx);
        }
        picked
    }
}

/// Every write here is timed as `reducer.overlay` (docs/perf.md: a
/// `reducer.*` sample past 250 ms is a hang).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.overlay")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    fn overlays(cx: &mut TestAppContext) -> (Entity<Session>, Entity<Overlays>) {
        let session = cx.update(super::super::tests::session);
        session.update(cx, |session, cx| {
            session.seed(
                SessionState {
                    connected: true,
                    network: "testkit".into(),
                    chain: "testkit#1".into(),
                    ..SessionState::default()
                },
                cx,
            )
        });
        let overlays = cx.new(|cx| Overlays::new(&session, cx));
        (session, overlays)
    }

    /// Settings opens on the page it last showed; a menu closes for any
    /// `Menu(_)`, Settings for any page, and nothing for what is not open.
    #[gpui_kit::test]
    fn settings_opens_on_its_last_page_and_closes_by_kind(cx: &mut TestAppContext) {
        let (_, overlays) = overlays(cx);
        let open = |cx: &mut TestAppContext| overlays.read_with(cx, |it, _| *it.get());
        overlays.update(cx, |it, cx| {
            it.open(Overlay::Settings(SettingsPage::Networks), cx)
        });
        overlays.update(cx, |it, cx| {
            it.close(Overlay::Settings(SettingsPage::About), cx)
        });
        assert_eq!(open(cx), None);
        overlays.update(cx, |it, cx| it.open_settings(cx));
        assert_eq!(open(cx), Some(Overlay::Settings(SettingsPage::Networks)));
        overlays.update(cx, |it, cx| it.toggle(Overlay::Menu(Popover::Node), cx));
        overlays.update(cx, |it, cx| it.close(Overlay::Spotlight, cx));
        assert_eq!(open(cx), Some(Overlay::Menu(Popover::Node)));
        overlays.update(cx, |it, cx| it.close(Overlay::Menu(Popover::Account), cx));
        assert_eq!(open(cx), None);
    }

    /// The keys leaving a menu close it only while it is the one open, and
    /// say so once; anything opening forgets it.
    #[gpui_kit::test]
    fn a_menu_closes_by_keys_only_while_it_is_open(cx: &mut TestAppContext) {
        let (_, overlays) = overlays(cx);
        let bell = Overlay::Menu(Popover::Notifications);
        overlays.update(cx, |it, cx| it.open(bell, cx));
        overlays.update(cx, |it, cx| {
            it.open(Overlay::Settings(SettingsPage::Notifications), cx)
        });
        overlays.update(cx, |it, cx| it.close_by_keys(bell, cx));
        assert_eq!(
            overlays.read_with(cx, |it, _| *it.get()),
            Some(Overlay::Settings(SettingsPage::Notifications)),
            "the bell's belt closed Settings"
        );
        assert!(!overlays.update(cx, |it, _| it.take_left_by_keys()));
        overlays.update(cx, |it, cx| it.open(bell, cx));
        overlays.update(cx, |it, cx| it.close_by_keys(bell, cx));
        assert_eq!(overlays.read_with(cx, |it, _| *it.get()), None);
        assert!(overlays.update(cx, |it, _| it.take_left_by_keys()));
        assert!(!overlays.update(cx, |it, _| it.take_left_by_keys()));
        overlays.update(cx, |it, cx| it.open(bell, cx));
        overlays.update(cx, |it, cx| it.close_by_keys(bell, cx));
        overlays.update(cx, |it, cx| it.open(Overlay::Spotlight, cx));
        assert!(
            !overlays.update(cx, |it, _| it.take_left_by_keys()),
            "an opening kept the last menu's word"
        );
    }

    /// Leaving the network closes what is open; a status that keeps it (a
    /// new height, a switch still reaching) closes nothing, and an equal
    /// write notifies nobody.
    #[gpui_kit::test]
    fn leaving_the_network_closes_the_overlay(cx: &mut TestAppContext) {
        let (session, overlays) = overlays(cx);
        let seen = Rc::new(Cell::new(0));
        let count = seen.clone();
        let _observing =
            cx.update(|cx| cx.observe(&overlays, move |_, _| count.set(count.get() + 1)));
        let edit = |cx: &mut TestAppContext, edit: fn(&mut SessionState)| {
            session.update(cx, |session, cx| {
                let mut state = session.get().clone();
                edit(&mut state);
                session.seed(state, cx);
            });
        };
        overlays.update(cx, |it, cx| {
            it.open(Overlay::Settings(SettingsPage::Networks), cx)
        });
        edit(cx, |session| session.connecting = true);
        assert!(overlays.read_with(cx, |it, _| it.get().is_some()));
        assert_eq!(seen.get(), 1);
        edit(cx, |session| session.connected = false);
        assert_eq!(overlays.read_with(cx, |it, _| *it.get()), None);
        overlays.update(cx, |it, cx| it.open(Overlay::Spotlight, cx));
        edit(cx, |session| {
            session.connected = true;
            session.network = "other".into();
        });
        assert_eq!(overlays.read_with(cx, |it, _| *it.get()), None);
        overlays.update(cx, |it, cx| it.open(Overlay::Approve, cx));
        edit(cx, |session| session.chain = "other#2".into());
        assert_eq!(overlays.read_with(cx, |it, _| *it.get()), None);
        assert_eq!(seen.get(), 6);
    }
}
