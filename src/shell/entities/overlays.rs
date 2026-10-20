//! What is open over one window's desk, and the words for it: the dialogs
//! on a scrim (Spotlight, Settings, Approve), the network switcher and the
//! bar's menus. Only the console's opens anything; a pop-out's stays shut.
//! Its methods are the one writer; it closes itself when the network is
//! left (its `Session` observer).
use super::{Session, SessionState};
use gpui_kit::{Context, Entity, Subscription};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlay {
    Spotlight,
    Approve,
    Consent(u64),
    Settings(SettingsPage),
    Network,
    Menu(Popover),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Popover {
    Node,
    Account,
    Notifications,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    #[default]
    Appearance,
    Notifications,
    Networks,
    Updates,
    About,
}

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
    FillWindow,
    HoldWindow,
    KeepOnTop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpotRow {
    pub(crate) group: &'static str,
    pub(crate) title: String,
    pub(crate) meta: String,
    pub(crate) spot: Spot,
}

pub(crate) struct Overlays {
    open: Option<Overlay>,
    page: SettingsPage,
    left_by_keys: bool,
    _session: Subscription,
}

impl Overlays {
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

    pub(crate) fn toggle(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        let open = (self.open != Some(overlay)).then_some(overlay);
        self.set(open, cx);
    }

    pub(crate) fn open(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        self.set(Some(overlay), cx);
    }

    pub(crate) fn follow_consent(&mut self, front: Option<u64>, cx: &mut Context<Self>) {
        let _timed = timed();
        if matches!(self.open, None | Some(Overlay::Consent(_))) {
            self.set(front.map(Overlay::Consent), cx);
        }
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.open(Overlay::Settings(self.page), cx);
    }

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

    pub(crate) fn close_by_keys(&mut self, menu: Overlay, cx: &mut Context<Self>) {
        let _timed = timed();
        if self.open == Some(menu) {
            self.set(None, cx);
            self.left_by_keys = true;
        }
    }

    pub(crate) fn take_left_by_keys(&mut self) -> bool {
        std::mem::take(&mut self.left_by_keys)
    }

    pub(crate) fn submit(&mut self, picked: Option<Spot>, cx: &mut Context<Self>) -> Option<Spot> {
        if picked.is_some() {
            self.close(Overlay::Spotlight, cx);
        }
        picked
    }
}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.overlay")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};

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

    #[gpui_kit::test]
    fn leaving_the_network_closes_the_overlay(cx: &mut TestAppContext) {
        let (session, overlays) = overlays(cx);
        let (seen, _observing) = crate::shell::entities::tests::notifies(&overlays, cx);
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
