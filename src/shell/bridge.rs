//! The bridge: every entity whose source is still the model's state,
//! written from it at the end of each `Desktop::dispatch` (and after the
//! few writes the `Desktop` makes outside one). While bridged a slice has
//! no other writer, and each write compares first, so a dispatch that
//! moved nothing notifies no slice. Each write goes with the step that
//! moves its source: `Overlays` and `Spotlight` in s8; `Session`, `Chain`,
//! `Account` and `Screen` in s10; the rest in s11.

use super::entities::{
    AccountStep, Chain, Entities, Front, Notifications, Overlay, Prefs, Rail, Screen, Session,
    Slice, Spotlight, WindowEntities,
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
        Self {
            session: cx.new(|_| Slice::new(session(state))),
            chain: cx.new(|_| chain(state)),
            account: cx.new(|_| Slice::new(account(state))),
            screen: cx.new(|_| Slice::new(screen(&state.stage))),
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
        kind: WindowKind,
        cx: &mut Context<Self>,
    ) -> WindowEntities {
        if let Some(own) = self.entities.by_window.get(&key) {
            return own.clone();
        }
        let (state, console) = (&self.state, kind == WindowKind::Console);
        let layout = state.layouts.get(&key).cloned().unwrap_or_default();
        let desk = cx.new(|_| Slice::new(layout));
        let own = WindowEntities {
            console,
            front: Front::of_desk(&desk, cx),
            desk,
            overlays: cx.new(|_| Slice::new(overlay(state, console))),
            spotlight: cx.new(|_| Slice::new(spotlight(state, console))),
            dot: cx.new(|_| Slice::new(None)),
        };
        self.entities.by_window.insert(key, own.clone());
        own
    }

    /// One write per bridged entity from the state as it stands. The
    /// notice settings are read off disk again only when `notify_saved`
    /// (a `SetNotify*` or a permission was saved): never on a beat.
    pub(super) fn bridge(&self, notify_saved: bool, cx: &mut Context<Self>) {
        let (state, entities) = (&self.state, &self.entities);
        set(&entities.session, session(state), cx);
        entities.chain.update(cx, |slice, cx| {
            slice.set(chain(state), cx);
        });
        set(&entities.account, account(state), cx);
        set(&entities.screen, screen(&state.stage), cx);
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
            set(&own.overlays, overlay(state, own.console), cx);
            set(&own.spotlight, spotlight(state, own.console), cx);
        }
    }
}

fn set<T: PartialEq + 'static>(slice: &Entity<Slice<T>>, value: T, cx: &mut App) {
    slice.update(cx, |slice, cx| {
        slice.set(value, cx);
    });
}

fn session(state: &Ducktape) -> Session {
    Session {
        connected: state.connected,
        connecting: state.connecting,
        reconnecting: state.reconnecting(),
        connected_rpc: state.connected_rpc.clone(),
        network: state.network.clone(),
        chain: state.chain.clone(),
        endpoint: state.endpoint.clone(),
        endpoint_error: state.endpoint_error.clone(),
        recent_endpoints: state.recent_endpoints.clone(),
        other_chain: state.other_chain,
        error: state.error.clone(),
    }
}

fn chain(state: &Ducktape) -> Chain {
    Chain {
        height: state.height,
        block_seen: state.block_seen,
        node: state.node.clone(),
        heard: state.heard,
    }
}

fn account(state: &Ducktape) -> super::entities::Account {
    super::entities::Account {
        signer_key: state.signer_key.clone(),
        account: state.account.clone(),
        key_exists: state.key_exists,
        locked: state.sign_in.locked,
        seating: state.sign_in.seating,
        busy: state.sign_in.unlock_busy,
        error: state.sign_in.unlock_error.clone(),
        approve: state.approve_fingerprint(),
        link_code: match &state.stage {
            Stage::Account(step) => step.link_code.clone(),
            _ => String::new(),
        },
        passkey_waiting: matches!(&state.stage, Stage::Account(step) if step.passkey_task.is_some()),
        passkey_qr: state.passkey_qr_shown(),
        welcome: state.welcome,
    }
}

fn screen(stage: &Stage) -> Screen {
    match stage {
        Stage::Connect => Screen::Connect,
        Stage::Unlock(step) => Screen::Unlock {
            awaiting: step.awaiting,
        },
        Stage::Phrase(step) => Screen::Phrase { quiz: step.quiz },
        Stage::Recover(_) => Screen::Recover,
        Stage::Account(step) => Screen::Account {
            step: match (step.passkey_task.is_some(), step.link_code.is_empty()) {
                (true, _) => AccountStep::Passkey,
                (false, false) => AccountStep::Link,
                (false, true) => AccountStep::Name,
            },
        },
        Stage::Desk => Screen::Desk,
    }
}

fn prefs(state: &Ducktape, notify: notify::Settings) -> Prefs {
    Prefs {
        appearance: state.appearance,
        system_dark: state.system_dark,
        motion: state.motion,
        notify,
    }
}

/// What is open over the desk, on the console; a pop-out's stays shut.
fn overlay(state: &Ducktape, console: bool) -> Option<Overlay> {
    Some(match state.overlay.filter(|_| console)? {
        crate::Overlay::Spotlight => Overlay::Spotlight,
        crate::Overlay::Approve => Overlay::Approve,
        crate::Overlay::Settings => Overlay::Settings(state.settings_page),
        crate::Overlay::Network => Overlay::Network,
        crate::Overlay::Menu(menu) => Overlay::Menu(menu),
    })
}

fn spotlight(state: &Ducktape, console: bool) -> Spotlight {
    match console {
        true => Spotlight {
            query: state.spotlight_query.clone(),
            pick: state.spotlight_pick,
        },
        false => Spotlight::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::panes_tests::console;
    use super::*;
    use crate::ui::test_support::status;
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

    /// The node answers at the same height twice: the chain moved once,
    /// and the session, which holds no status line, not at all.
    #[gpui_kit::test]
    fn an_unchanged_status_keeps_the_session_slice_still(cx: &mut TestAppContext) {
        let (model, _, _, mut native) = console(cx);
        send(&model, Message::StatusPushed(status(6)), &mut native);
        let (session, chain) = model.read_with(&native, |model, _| {
            (model.entities.session.clone(), model.entities.chain.clone())
        });
        let (session, _session) = notifies(&session, &mut native);
        let (chain, _chain) = notifies(&chain, &mut native);
        for _ in 0..2 {
            send(&model, Message::StatusPushed(status(7)), &mut native);
        }
        assert_eq!(session.get(), 0, "a new block moved the session");
        assert_eq!(chain.get(), 1, "the chain did not move once for one block");
    }

    /// Each bridged source moved: its slice moved with it, once; a view's post
    /// reaches the bell's slice through the dispatch its `Intent::Notified` makes.
    #[gpui_kit::test]
    fn a_moved_source_moves_its_slice_once(cx: &mut TestAppContext) {
        let (model, key, _, mut native) = console(cx);
        let e = model.read_with(&native, |model, _| {
            let e = &model.entities;
            (
                e.session.clone(),
                e.account.clone(),
                e.screen.clone(),
                e.rail.clone(),
                e.toast.clone(),
                e.windows.clone(),
                e.notifications.clone(),
                e.by_window[&key].spotlight.clone(),
            )
        });
        let (session, _a) = notifies(&e.0, &mut native);
        let (account, _b) = notifies(&e.1, &mut native);
        let (screen, _c) = notifies(&e.2, &mut native);
        let (rail, _d) = notifies(&e.3, &mut native);
        let (toast, _e) = notifies(&e.4, &mut native);
        let (windows, _f) = notifies(&e.5, &mut native);
        let (notifications, _g) = notifies(&e.6, &mut native);
        let (spotlight, _h) = notifies(&e.7, &mut native);
        model.update(&mut native, |model, cx| {
            let state = &mut model.state;
            state.error = "refused".into();
            state.sign_in.locked = !state.sign_in.locked;
            state.stage = crate::Stage::Connect;
            state.badges.insert("pane-ax-test", 2);
            state.toast = "said".into();
            state.active = Some("other-view");
            state.spotlight_query = "cha".into();
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
            ("session", session),
            ("account", account),
            ("screen", screen),
            ("rail", rail),
            ("toast", toast),
            ("windows", windows),
            ("notifications", notifications),
            ("spotlight", spotlight),
        ] {
            assert_eq!(seen.get(), 1, "{name} did not follow its source");
        }
    }

    /// A poll at the same height a second later moves only `heard`, which is not compared.
    #[gpui_kit::test]
    fn a_poll_that_only_moves_heard_leaves_the_chain_still(cx: &mut TestAppContext) {
        let (model, _, _, mut native) = console(cx);
        send(&model, Message::StatusPushed(status(7)), &mut native);
        let chain = model.read_with(&native, |model, _| model.entities.chain.clone());
        let (chain, _chain) = notifies(&chain, &mut native);
        send(&model, Message::WallTick, &mut native);
        send(&model, Message::StatusPushed(status(7)), &mut native);
        assert_eq!(
            chain.get(),
            0,
            "a poll that only moved heard notified the chain"
        );
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
