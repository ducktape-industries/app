//! The bridge: every entity whose source is still the model's state,
//! written from it at the end of each `Desktop::dispatch` (and after the
//! few writes the `Desktop` makes outside one). While bridged a slice has
//! no other writer, and each write compares first, so a dispatch that
//! moved nothing notifies no slice. Each write goes with the step that
//! moves its source, s11 for all that are left. (`Session`, `Chain`,
//! `Account` and `Screen` are their own flows' since s10; `Overlays` and
//! `Spotlight` have their own methods; what the model still keeps for them
//! follows them from here.)

use super::entities::{
    Account, Chain, Entities, Front, Notifications, Overlay, Overlays, Prefs, Rail, Screen,
    Session, Slice, Spotlight, WindowEntities,
};
use super::*;
use crate::runtime::notify;
use gpui_kit::App;

impl Entities {
    /// Every app-wide entity, read off `state`; the rail's rows read now
    /// and again on every message `changes` brings.
    pub(super) fn new(
        state: &Ducktape,
        changes: mpsc::UnboundedReceiver<()>,
        cx: &mut App,
    ) -> Self {
        let chain = cx.new(|_| Chain::default());
        let screen = cx.new(|_| Slice::new(Screen::Connect));
        let account = cx.new(|_| Account::new(screen.clone()));
        let session = {
            let (chain, account, screen) = (chain.clone(), account.clone(), screen.clone());
            cx.new(|_| Session::new(chain, account, screen, state.center.clone()))
        };
        Self {
            session,
            chain,
            account,
            screen,
            rail: cx.new(|cx| Rail::new(state.roster.clone(), state.badges.clone(), changes, cx)),
            notifications: cx.new(|_| Notifications::new(state.center.clone())),
            toast: cx.new(|_| Slice::new(state.toast.clone())),
            prefs: cx.new(|_| Slice::new(prefs(state, notify::Settings::load()))),
            windows: cx.new(|_| Slice::new(state.active)),
            by_window: BTreeMap::new(),
        }
    }

    /// As `new`, with a rail no loader thread wakes: a test refreshes it.
    #[cfg(test)]
    pub(super) fn for_test(state: &Ducktape, cx: &mut App) -> Self {
        Self::new(state, mpsc::unbounded().1, cx)
    }
}

impl Desktop {
    /// Window `key`'s own entities, made the first time they are asked
    /// for, read off the state as it stands.
    pub(super) fn window_entities(
        &mut self,
        key: WindowKey,
        cx: &mut Context<Self>,
    ) -> WindowEntities {
        if let Some(own) = self.entities.by_window.get(&key) {
            return own.clone();
        }
        let layout = self.state.layouts.get(&key).cloned().unwrap_or_default();
        let desk = cx.new(|_| Slice::new(layout));
        let session = self.entities.session.clone();
        let own = WindowEntities {
            front: Front::of_desk(&desk, cx),
            desk,
            overlays: cx.new(|cx| Overlays::new(&session, cx)),
            spotlight: cx.new(|_| Slice::new(Spotlight::default())),
            dot: cx.new(|_| Slice::new(None)),
        };
        self.follow_overlays(key, &own, cx);
        self.entities.by_window.insert(key, own.clone());
        own
    }

    /// What follows what opens over window `key`'s desk: the keyboard lets
    /// go of a window it held as anything opens (the model's, until s11:
    /// `Desk` does), and "Add a device…" closing, however it did, forgets
    /// what it found (`Account`).
    fn follow_overlays(&self, key: WindowKey, own: &WindowEntities, cx: &mut Context<Self>) {
        let desk = own.desk.clone();
        let mut was = None;
        cx.observe(&own.overlays, move |desktop, overlays, cx| {
            let open = *overlays.read(cx).get();
            let before = std::mem::replace(&mut was, open);
            if before == Some(Overlay::Approve) && open != before {
                desktop
                    .entities
                    .account
                    .update(cx, |account, cx| account.approve_closed(cx));
            }
            if open.is_some() && desk.read(cx).get().held.is_some() {
                let release = PaneMessage::Release { keep: true };
                desktop.dispatch(Message::Pane(key, release), cx);
            }
        })
        .detach();
    }

    /// A device approved (`AccountEvent::Approved`): its dialog closes.
    /// `Account` is the app's and the dialog is the console's, so the
    /// close is here (s11: `Windows`).
    pub(super) fn approved(&self, cx: &mut Context<Self>) {
        let console = self.state.console_win;
        if let Some(own) = console.and_then(|key| self.entities.by_window.get(&key)) {
            own.overlays
                .update(cx, |overlays, cx| overlays.close(Overlay::Approve, cx));
        }
    }

    /// One write per bridged entity from the state as it stands. The
    /// notice settings are read off disk again only when `notify_saved`
    /// (a `SetNotify*` or a permission was saved): never on a beat.
    pub(super) fn bridge(&self, notify_saved: bool, cx: &mut Context<Self>) {
        let (state, entities) = (&self.state, &self.entities);
        entities.rail.update(cx, |rail, cx| {
            rail.set_badges(state.badges.clone(), cx);
        });
        entities.notifications.update(cx, |slice, cx| {
            slice.refresh(cx);
        });
        set(&entities.toast, state.toast.clone(), cx);
        let notify = match notify_saved {
            true => notify::Settings::load(),
            false => entities.prefs.read(cx).get().notify.clone(),
        };
        set(&entities.prefs, prefs(state, notify), cx);
        set(&entities.windows, state.active, cx);
        // a closed window's desk reads empty (`Layout::default()`)
        for (key, own) in &entities.by_window {
            let layout = state.layouts.get(key).cloned().unwrap_or_default();
            set(&own.desk, layout, cx);
        }
    }
}

fn set<T: PartialEq + 'static>(slice: &Entity<Slice<T>>, value: T, cx: &mut App) {
    slice.update(cx, |slice, cx| {
        slice.set(value, cx);
    });
}

fn prefs(state: &Ducktape, notify: notify::Settings) -> Prefs {
    Prefs {
        appearance: state.appearance,
        system_dark: state.system_dark,
        motion: state.motion,
        notify,
    }
}

#[cfg(test)]
mod tests {
    use super::super::panes_tests::console;
    use super::*;
    use gpui_kit::{Subscription, TestAppContext, VisualTestContext};
    use std::cell::Cell;
    use std::rc::Rc;

    /// Counts `entity`'s notifies while the subscription lives.
    fn notifies<T: 'static>(
        entity: &Entity<T>,
        native: &mut VisualTestContext,
    ) -> (Rc<Cell<usize>>, Subscription) {
        let seen = Rc::new(Cell::new(0));
        let count = seen.clone();
        let observing =
            native.update(|_, cx| cx.observe(entity, move |_, _| count.set(count.get() + 1)));
        (seen, observing)
    }

    fn send(model: &Entity<Desktop>, message: Message, native: &mut VisualTestContext) {
        model.update(native, |model, cx| model.dispatch(message, cx));
        native.run_until_parked();
    }

    /// What the reducer still keeps follows what opens over a window's
    /// desk, with no window drawing to move the keys: anything opening lets
    /// go of the desk's hold; a device approved closes "Add a device…",
    /// and its closing forgets what it found.
    #[gpui_kit::test]
    fn the_model_follows_what_opens_over_a_window(cx: &mut TestAppContext) {
        use crate::ui::layout::{EMPTY, Layout, PaneMessage};
        let mut state = Ducktape::boot();
        state.roster = Default::default();
        state.center = Default::default();
        let key = WindowKey::unique();
        let mut layout = Layout::default();
        layout.split(EMPTY);
        layout.measure((1280., 764.));
        layout.settle();
        layout.initialized = true;
        state.console_win = Some(key);
        state.layouts.insert(key, layout);
        let model = cx.new(|cx| {
            let entities = Entities::for_test(&state, cx);
            entities.screen.update(cx, |screen, cx| {
                screen.set(Screen::Desk, cx);
            });
            Desktop::new(state, crate::tray::init(cx).0, entities, cx)
        });
        let own = model.update(cx, |model, cx| model.window_entities(key, cx));
        let dispatch = |message: Message, cx: &mut TestAppContext| {
            model.update(cx, |model, cx| model.dispatch(message, cx))
        };
        let held = |cx: &mut TestAppContext| own.desk.read_with(cx, |desk, _| desk.get().held);
        dispatch(Message::Pane(key, PaneMessage::Hold(0)), cx);
        assert!(held(cx).is_some());
        own.overlays
            .update(cx, |it, cx| it.open(Overlay::Spotlight, cx));
        assert_eq!(held(cx), None, "Search opened over a hold");

        own.overlays
            .update(cx, |it, cx| it.open(Overlay::Approve, cx));
        let account = model.read_with(cx, |model, _| model.entities.account.clone());
        account.update(cx, |account, cx| {
            let key = vec![7; 32];
            let mut state = account.get().clone();
            state.approve = Some(crate::backend::join::fingerprint(&key));
            let found = crate::backend::join::Request {
                network: "testkit".into(),
                key,
            };
            account.seed(state, None, Some(found), cx);
        });
        account.update(cx, |account, cx| account.approve_done(Ok(()), cx));
        cx.run_until_parked();
        assert_eq!(own.overlays.read_with(cx, |it, _| *it.get()), None);
        let found = account.read_with(cx, |account, _| account.get().approve.clone());
        assert_eq!(found, None, "Approve closed and kept what it found");
        let toast = model.read_with(cx, |model, _| model.state.toast.clone());
        assert!(toast.starts_with("Approved."), "{toast:?}");
    }

    /// The clock beats on a still app: the bridge writes every slice, and
    /// none of them notifies, so nothing observing one draws.
    #[gpui_kit::test]
    fn a_bridge_write_of_an_equal_value_notifies_no_slice(cx: &mut TestAppContext) {
        let (model, key, _, mut native) = console(cx);
        // a first beat and its frame: whatever a draw writes is in
        send(&model, Message::WallTick, &mut native);
        // every slice's notifies, counted from here
        macro_rules! counted {
            ($($field:ident),+) => {
                vec![$({
                    let entity = model.read_with(&native, |model, _| model.entities.$field.clone());
                    let (seen, observing) = notifies(&entity, &mut native);
                    (stringify!($field), seen, observing)
                }),+]
            };
        }
        let counts = counted![
            session,
            chain,
            account,
            screen,
            rail,
            notifications,
            toast,
            prefs,
            windows
        ];
        let window = model.read_with(&native, |model, _| model.entities.by_window[&key].clone());
        let (desk, _desk) = notifies(&window.desk, &mut native);
        let (overlays, _overlays) = notifies(&window.overlays, &mut native);
        let (spotlight, _spotlight) = notifies(&window.spotlight, &mut native);
        let (front, _front) = notifies(&window.front, &mut native);
        for _ in 0..3 {
            send(&model, Message::WallTick, &mut native);
        }
        let window = [
            ("desk", desk),
            ("overlays", overlays),
            ("spotlight", spotlight),
            ("front", front),
        ];
        let counts = counts.iter().map(|(name, seen, _)| (*name, seen.clone()));
        for (name, seen) in counts.chain(window) {
            assert_eq!(seen.get(), 0, "a beat that moved nothing notified {name}");
        }
    }

    /// A notice setting saved reaches `Prefs` in the dispatch that saved it;
    /// a beat after it reads nothing off disk and moves nothing.
    #[gpui_kit::test]
    fn a_saved_notice_setting_reaches_the_prefs_slice(cx: &mut TestAppContext) {
        let (model, _, _, mut native) = console(cx);
        let prefs = model.read_with(&native, |model, _| model.entities.prefs.clone());
        let burst = |native: &mut VisualTestContext| {
            prefs.read_with(native, |prefs, _| prefs.get().notify.burst)
        };
        let before = burst(&mut native);
        let next = notify::BURSTS.into_iter().find(|it| *it != before).unwrap();
        let (seen, _seen) = notifies(&prefs, &mut native);
        send(&model, Message::SetNotifyBurst(next), &mut native);
        assert_eq!(
            burst(&mut native),
            next,
            "the saved burst never reached Prefs"
        );
        send(&model, Message::WallTick, &mut native);
        assert_eq!(seen.get(), 1, "the save and the beat after it");
    }

    /// The door's walk puts a saved notice setting back: `Prefs` follows
    /// the file back, not only the next save.
    #[gpui_kit::test]
    fn a_kept_notice_setting_put_back_reaches_the_prefs_slice(cx: &mut TestAppContext) {
        let (model, _, _, mut native) = console(cx);
        let prefs = model.read_with(&native, |model, _| model.entities.prefs.clone());
        let burst = |native: &mut VisualTestContext| {
            prefs.read_with(native, |prefs, _| prefs.get().notify.burst)
        };
        let kept = native
            .update(|window, cx| Kept::of(window, cx))
            .expect("the console is the shell's");
        let before = burst(&mut native);
        let other = notify::BURSTS.into_iter().find(|it| *it != before).unwrap();
        send(&model, Message::SetNotifyBurst(other), &mut native);
        native.update(|_, cx| kept.restore(cx));
        assert_eq!(
            burst(&mut native),
            before,
            "Prefs kept the burst the walk put back"
        );
    }

    /// The theme synced outside a dispatch reaches `Prefs` with it.
    #[gpui_kit::test]
    fn an_appearance_synced_outside_a_dispatch_reaches_the_prefs_slice(cx: &mut TestAppContext) {
        let (model, _, _, mut native) = console(cx);
        let before = model.read_with(&native, |m, cx| m.entities.prefs.read(cx).get().system_dark);
        let target = if before {
            crate::Appearance::Light
        } else {
            crate::Appearance::Dark
        };
        model.update(&mut native, |m, cx| {
            m.state.appearance = target;
            m.sync_appearance(cx);
        });
        let (state_dark, slice_dark) = model.read_with(&native, |m, cx| {
            (
                m.state.system_dark,
                m.entities.prefs.read(cx).get().system_dark,
            )
        });
        assert_ne!(state_dark, before, "the theme did not move");
        assert_eq!(slice_dark, state_dark, "Prefs.system_dark lags the state");
    }

    /// Each bridged source moved: its slice moved with it, once; a view's post
    /// reaches the bell's slice through the dispatch its `Intent::Notified` makes.
    #[gpui_kit::test]
    fn a_moved_source_moves_its_slice_once(cx: &mut TestAppContext) {
        let (model, _, _, mut native) = console(cx);
        let e = model.read_with(&native, |model, _| {
            let e = &model.entities;
            (
                e.rail.clone(),
                e.toast.clone(),
                e.windows.clone(),
                e.notifications.clone(),
            )
        });
        let (rail, _d) = notifies(&e.0, &mut native);
        let (toast, _e) = notifies(&e.1, &mut native);
        let (windows, _f) = notifies(&e.2, &mut native);
        let (notifications, _g) = notifies(&e.3, &mut native);
        model.update(&mut native, |model, cx| {
            let state = &mut model.state;
            state.badges.insert("pane-ax-test", 2);
            state.toast = "said".into();
            state.active = Some("other-view");
            model.bridge(false, cx);
        });
        model.update(&mut native, |model, _| {
            let settings = notify::Settings {
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
            let _ = model.state.center.lock().post(
                &settings,
                "pane-ax-test",
                "Pane",
                post,
                std::time::Instant::now(),
                0,
            );
        });
        send(
            &model,
            Message::ViewEvent("pane-ax-test", crate::runtime::Intent::Notified),
            &mut native,
        );
        for (name, seen) in [
            ("rail", rail),
            ("toast", toast),
            ("windows", windows),
            ("notifications", notifications),
        ] {
            assert_eq!(seen.get(), 1, "{name} did not follow its source");
        }
    }

    /// A drag frame moves the desk and leaves the front still; a focus that
    /// brings another program forward moves it once.
    #[gpui_kit::test]
    fn the_front_slice_follows_focus_and_not_frames(cx: &mut TestAppContext) {
        use crate::ui::layout::PaneMessage;
        let (model, key, _, mut native) = console(cx);
        send(
            &model,
            Message::Pane(key, PaneMessage::Open("front-test-view")),
            &mut native,
        );
        let (desk, front) = model.read_with(&native, |model, _| {
            let own = &model.entities.by_window[&key];
            (own.desk.clone(), own.front.clone())
        });
        let layout = desk.read_with(&native, |desk, _| desk.get().clone());
        let modules: Vec<_> = layout.panes.iter().map(|pane| pane.module).collect();
        assert_eq!(modules, ["pane-ax-test", "front-test-view"]);
        assert_eq!(layout.focused, 1);
        let (desks, _desks) = notifies(&desk, &mut native);
        let (fronts, _fronts) = notifies(&front, &mut native);

        let mut frame = layout.panes[1].frame.expect("a pane on the desk");
        frame.x += 10.;
        send(
            &model,
            Message::Pane(key, PaneMessage::Frame(1, frame)),
            &mut native,
        );
        assert_eq!(desks.get(), 1, "the frame did not move the desk");
        assert_eq!(fronts.get(), 0, "a drag frame moved the front");

        send(
            &model,
            Message::Pane(key, PaneMessage::Focus(0)),
            &mut native,
        );
        assert_eq!(fronts.get(), 1, "the focus moved and the front did not");
        let focused = front.read_with(&native, |front, _| front.get().focused);
        assert_eq!(focused, Some("pane-ax-test"));
    }
}
