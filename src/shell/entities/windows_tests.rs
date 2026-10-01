//! The windows, moved by their methods (windows.rs), and what the app-wide
//! entities around them do on their own: a toast's clock, a pref's write,
//! the theme's sync, the network left.
use super::tests::{active, desk_of, entities, modules, notifies};
use super::{Entities, Overlay, SessionState};
use crate::runtime::WindowKey;
use crate::shell::WindowKind;
use crate::ui::layout::{EMPTY, Frame, MAX_PANES};
use gpui_kit::{AppContext as _, Bounds, TestAppContext, point, px, size};

const DESK: (f32, f32) = (1280., 764.);

/// The app's entities with a console open on a measured desk showing
/// "chat", its OS window on the test platform; a window that closes is
/// forgotten, as `launch::run` wires it.
fn console(cx: &mut TestAppContext) -> (Entities, WindowKey) {
    let app = cx.update(|cx| {
        gpui_kit::init(cx);
        let app = entities(cx);
        let windows = app.windows.downgrade();
        cx.on_window_closed(move |cx, id| {
            let windows = windows.clone();
            cx.defer(move |cx| {
                let _ = windows.update(cx, |windows, cx| windows.closed_id(id, cx));
            });
        })
        .detach();
        app
    });
    let at = Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(800.)));
    let key = app.windows.update(cx, |windows, cx| {
        windows.open(WindowKind::Console, Some(at), cx)
    });
    cx.run_until_parked();
    desk_of(&app, key, cx).update(cx, |desk, cx| {
        desk.resize(DESK, cx);
        desk.seed("chat", cx);
    });
    (app, key)
}

fn window_count(app: &Entities, cx: &TestAppContext) -> usize {
    app.windows
        .read_with(cx, |windows, _| windows.handles().len())
}

/// The one window that is not the console.
fn popped(app: &Entities, console: WindowKey, cx: &TestAppContext) -> WindowKey {
    app.windows.read_with(cx, |windows, _| {
        *windows
            .by_window()
            .keys()
            .find(|key| **key != console)
            .expect("a window of its own")
    })
}

fn listed(app: &Entities, modules: &[&str], cx: &mut TestAppContext) {
    let roster = crate::runtime::Roster::listing(modules);
    app.rail.update(cx, |rail, cx| rail.read_off(roster, cx));
}

/// A view picked from ⌘K or a menu opens as a bar click does: the focused
/// window keeps its program, and the pick is the program in front.
#[gpui_kit::test]
fn a_picked_view_opens_beside_the_focused_one(cx: &mut TestAppContext) {
    let (app, key) = console(cx);
    let pick = |module, cx: &mut TestAppContext| {
        app.windows
            .update(cx, |windows, cx| windows.select_view(module, cx))
    };
    pick("files", cx);
    assert_eq!(modules(&app, key, cx), ["chat", "files"]);
    assert_eq!(active(&app, cx), Some("files"));
    pick("chat", cx);
    assert_eq!(
        modules(&app, key, cx),
        ["chat", "files"],
        "the window it is in"
    );
    assert_eq!(active(&app, cx), Some("chat"));
    // a full desk has no room for one of its own: the focused window's
    // place, not nothing
    let desk = desk_of(&app, key, cx);
    while desk.read_with(cx, |desk, _| desk.get().panes.len()) < MAX_PANES {
        desk.update(cx, |desk, cx| desk.split("files", cx));
    }
    pick("calendar", cx);
    assert_eq!(
        desk.read_with(cx, |desk, _| desk.get().shown()),
        Some("calendar")
    );
}

/// A pane popped out keeps its instance (its view) in a window of its
/// own; popped back in, it lands on the desk and its window goes.
#[gpui_kit::test]
fn pop_out_and_back_in_keep_the_pane(cx: &mut TestAppContext) {
    let (app, console) = console(cx);
    let desk = desk_of(&app, console, cx);
    desk.update(cx, |desk, cx| desk.split("files", cx));
    let files = desk.read_with(cx, |desk, _| desk.get().panes[1].instance);
    app.windows
        .update(cx, |windows, cx| windows.pop_out(console, 1, None, cx));
    cx.run_until_parked();
    assert_eq!(modules(&app, console, cx), ["chat"]);
    let popped_key = popped(&app, console, cx);
    assert_eq!(window_count(&app, cx), 2, "the pop-out's window opened");
    assert_eq!(
        modules(&app, popped_key, cx),
        ["files"],
        "the pane is in its own window"
    );
    let popped_desk = desk_of(&app, popped_key, cx);
    assert_eq!(
        popped_desk.read_with(cx, |desk, _| desk.get().panes[0].instance),
        files
    );
    app.windows
        .update(cx, |windows, cx| windows.pop_in(popped_key, cx));
    cx.run_until_parked();
    assert_eq!(
        desk.read_with(cx, |desk, _| desk.get().panes[1].instance),
        files
    );
    assert!(
        app.windows
            .read_with(cx, |windows, _| windows.own(popped_key).is_none()),
        "the pop-out's window is still listed"
    );
    assert_eq!(window_count(&app, cx), 1);
    // an empty window has no view to carry out
    desk.update(cx, |desk, cx| desk.split(EMPTY, cx));
    let focused = desk.read_with(cx, |desk, _| desk.get().focused);
    app.windows.update(cx, |windows, cx| {
        windows.pop_out(console, focused, None, cx)
    });
    cx.run_until_parked();
    assert_eq!(window_count(&app, cx), 1);
    assert_eq!(modules(&app, console, cx), ["chat", "files", EMPTY]);
}

/// A link, read against the chain in hand: on this chain the seat opens
/// and its view is handed the route; another chain's link opens the seat,
/// routes nothing, and says why; a view nobody lists opens nothing.
#[gpui_kit::test]
fn a_link_opens_the_seat_and_a_bad_one_toasts(cx: &mut TestAppContext) {
    let (app, key) = console(cx);
    app.session.update(cx, |session, cx| {
        let mut state = SessionState::booted();
        state.chain = "testkit#0a1b2c3d".into();
        session.seed(state, cx);
    });
    listed(&app, &["link-test-here", "link-test-away"], cx);
    let toast = |cx: &TestAppContext| app.toast.read_with(cx, |toast, _| toast.get().clone());
    let open = |link: &str, cx: &mut TestAppContext| {
        app.windows
            .update(cx, |windows, cx| windows.open_link(link, cx))
    };
    open("duck://testkit-0a1b2c3d/link-test-here/tx/00ff", cx);
    assert_eq!(active(&app, cx), Some("link-test-here"));
    assert_eq!(modules(&app, key, cx), ["chat", "link-test-here"]);
    assert!(toast(cx).is_empty(), "the view coming forward says it");
    assert_eq!(
        crate::runtime::take_route("link-test-here").as_deref(),
        Some("tx/00ff")
    );
    // another chain's link opens the seat, routes nothing, and says why
    open("duck://othernet-0a1b2c3d/link-test-away/tx/00ff", cx);
    assert_eq!(active(&app, cx), Some("link-test-away"));
    assert_eq!(crate::runtime::take_route("link-test-away"), None);
    assert!(toast(cx).contains("othernet"), "{:?}", toast(cx));
    // a view nobody lists opens nothing
    app.toast.update(cx, |toast, cx| toast.dismiss(cx));
    open("duck://testkit-0a1b2c3d/link-test-nowhere/x", cx);
    assert_eq!(active(&app, cx), Some("link-test-away"));
    assert!(toast(cx).contains("link-test-nowhere"), "{:?}", toast(cx));
    assert_eq!(
        modules(&app, key, cx),
        ["chat", "link-test-here", "link-test-away"]
    );
}

/// A pick and a link both land in the console's desk at once, each of
/// them; leaving the network empties every window, keeping its measure,
/// closes what is open over it, and takes the badges and the active
/// program with it.
#[gpui_kit::test]
fn spotlight_and_links_both_land_in_the_desk_and_leaving_clears_it(cx: &mut TestAppContext) {
    // leaving locks the signer: the seat is one for the process
    let _seat = crate::backend::seat_serial();
    let (app, key) = console(cx);
    listed(&app, &["pane-link"], cx);
    app.rail
        .update(cx, |rail, cx| rail.set_badge("pane-link", 2, cx));
    app.windows.update(cx, |windows, cx| {
        windows.select_view("forge", cx);
        windows.open_link("duck://pane-link/x", cx);
    });
    assert_eq!(modules(&app, key, cx), ["chat", "forge", "pane-link"]);
    assert_eq!(active(&app, cx), Some("pane-link"));
    let overlays = app
        .windows
        .read_with(cx, |windows, _| windows.own(key).unwrap().overlays.clone());
    overlays.update(cx, |it, cx| it.open(Overlay::Spotlight, cx));
    app.session.update(cx, |session, cx| session.disconnect(cx));
    cx.run_until_parked();
    let layout = desk_of(&app, key, cx).read_with(cx, |desk, _| desk.get().clone());
    assert!(layout.panes.is_empty(), "leaving left panes");
    assert_eq!(layout.desk, Some(DESK));
    assert!(!layout.initialized);
    assert_eq!(active(&app, cx), None);
    assert_eq!(overlays.read_with(cx, |it, _| *it.get()), None);
    assert!(app.rail.read_with(cx, |rail, _| rail.badges().is_empty()));
}

/// A badge set before its view opens stays until the view clears it.
#[gpui_kit::test]
fn opening_a_view_keeps_its_badge_until_the_view_clears_it(cx: &mut TestAppContext) {
    let (app, _) = console(cx);
    app.rail
        .update(cx, |rail, cx| rail.set_badge("chat", 3, cx));
    app.windows
        .update(cx, |windows, cx| windows.select_view("chat", cx));
    assert_eq!(app.rail.read_with(cx, |rail, _| rail.badge("chat")), 3);
    app.rail
        .update(cx, |rail, cx| rail.set_badge("chat", 0, cx));
    assert!(app.rail.read_with(cx, |rail, _| rail.badges().is_empty()));
}

/// A toast shows for 3.6 s on its own clock and takes itself down: one
/// notify up, one down, none between.
#[gpui_kit::test]
fn a_toast_dismisses_itself_after_3_6_s_with_two_notifies(cx: &mut TestAppContext) {
    let app = cx.update(entities);
    let (seen, _seen) = notifies(&app.toast, cx);
    app.toast
        .update(cx, |toast, cx| toast.show("Saved.".into(), cx));
    assert_eq!(
        app.toast.read_with(cx, |toast, _| toast.get().clone()),
        "Saved."
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(3500));
    cx.run_until_parked();
    assert_eq!(seen.get(), 1, "the toast moved before its time");
    assert_eq!(
        app.toast.read_with(cx, |toast, _| toast.get().clone()),
        "Saved."
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert!(app.toast.read_with(cx, |toast, _| toast.get().is_empty()));
    assert_eq!(seen.get(), 2);
    // the same notice again notifies nothing on its way up
    app.toast
        .update(cx, |toast, cx| toast.show("Again.".into(), cx));
    app.toast
        .update(cx, |toast, cx| toast.show("Again.".into(), cx));
    assert_eq!(seen.get(), 3);
}

/// A node answering (`SessionEvent::Connected`) brings the console back:
/// with its window closed to the tray, a new one opens.
#[gpui_kit::test]
fn a_node_answering_reopens_a_closed_console(cx: &mut TestAppContext) {
    use super::SessionEvent;
    let (app, key) = console(cx);
    let handle = app
        .windows
        .read_with(cx, |windows, _| windows.handles()[&key]);
    cx.update_window(handle, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(window_count(&app, cx), 0, "the console never closed");
    app.session
        .update(cx, |_, cx| cx.emit(SessionEvent::Connected));
    cx.run_until_parked();
    assert_eq!(
        window_count(&app, cx),
        1,
        "the node answered and no console came"
    );
}

/// A notice setting saved reaches the file and `Prefs` in the call that
/// saved it, with one notify; a reload after it moves nothing.
#[gpui_kit::test]
fn a_pref_write_notifies_once_and_reaches_disk(cx: &mut TestAppContext) {
    use crate::runtime::notify;
    let app = cx.update(entities);
    let burst = |cx: &TestAppContext| app.prefs.read_with(cx, |prefs, _| prefs.get().notify.burst);
    let before = burst(cx);
    let next = notify::BURSTS.into_iter().find(|it| *it != before).unwrap();
    let (seen, _seen) = notifies(&app.prefs, cx);
    app.prefs
        .update(cx, |prefs, cx| prefs.set_notify_burst(next, cx));
    assert_eq!(burst(cx), next, "the saved burst never reached Prefs");
    assert_eq!(
        notify::Settings::load().burst,
        next,
        "the saved burst never reached the file"
    );
    app.prefs.update(cx, |prefs, cx| prefs.reload(cx));
    assert_eq!(seen.get(), 1, "the save, and nothing after it");
}

/// The door's walk puts a saved notice setting back: `Prefs` follows the
/// file back, not only the next save.
#[gpui_kit::test]
fn a_kept_notice_setting_put_back_reaches_the_prefs_slice(cx: &mut TestAppContext) {
    use crate::runtime::notify;
    use crate::shell::layers::Kept;
    let (app, key) = console(cx);
    let burst = |cx: &TestAppContext| app.prefs.read_with(cx, |prefs, _| prefs.get().notify.burst);
    let handle = app
        .windows
        .read_with(cx, |windows, _| windows.handles()[&key]);
    let kept = cx
        .update_window(handle, |_, window, cx| Kept::of(window, cx))
        .unwrap()
        .expect("the console is the shell's");
    let before = burst(cx);
    let other = notify::BURSTS.into_iter().find(|it| *it != before).unwrap();
    app.prefs
        .update(cx, |prefs, cx| prefs.set_notify_burst(other, cx));
    cx.update(|cx| kept.restore(cx));
    assert_eq!(burst(cx), before, "Prefs kept the burst the walk put back");
}

/// The appearance chosen reaches the theme, and what the theme says the
/// OS is comes back into `Prefs`.
#[gpui_kit::test]
fn an_appearance_change_syncs_the_theme(cx: &mut TestAppContext) {
    use gpui_kit::component::Theme;
    let (app, _) = console(cx);
    let dark = |cx: &mut TestAppContext| {
        (
            cx.update(|cx| Theme::global(cx).is_dark()),
            app.prefs.read_with(cx, |prefs, _| prefs.get().system_dark),
        )
    };
    for (mode, expected) in [
        (crate::backend::Appearance::Dark, true),
        (crate::backend::Appearance::Light, false),
    ] {
        app.prefs
            .update(cx, |prefs, cx| prefs.set_appearance(mode, cx));
        cx.run_until_parked();
        assert_eq!(dark(cx), (expected, expected), "{mode:?}");
        assert_eq!(
            crate::backend::load_appearance(),
            mode,
            "the appearance never reached the file"
        );
    }
}

/// A drag frame moves the desk and leaves the front still; a focus that
/// brings another program forward moves it once.
#[gpui_kit::test]
fn the_front_follows_focus_and_not_frames(cx: &mut TestAppContext) {
    let (app, key) = console(cx);
    let desk = desk_of(&app, key, cx);
    desk.update(cx, |desk, cx| desk.open("front-test-view", cx));
    let front = app
        .windows
        .read_with(cx, |windows, _| windows.own(key).unwrap().front.clone());
    let layout = desk.read_with(cx, |desk, _| desk.get().clone());
    assert_eq!(modules(&app, key, cx), ["chat", "front-test-view"]);
    assert_eq!(layout.focused, 1);
    let (desks, _desks) = notifies(&desk, cx);
    let (fronts, _fronts) = notifies(&front, cx);
    let mut frame: Frame = layout.panes[1].frame.expect("a pane on the desk");
    frame.x += 10.;
    desk.update(cx, |desk, cx| desk.set_frame(1, frame, cx));
    assert_eq!(desks.get(), 1, "the frame did not move the desk");
    assert_eq!(fronts.get(), 0, "a drag frame moved the front");
    desk.update(cx, |desk, cx| desk.focus(0, cx));
    assert_eq!(fronts.get(), 1, "the focus moved and the front did not");
    assert_eq!(
        front.read_with(cx, |front, _| front.get().focused),
        Some("chat")
    );
    assert_eq!(active(&app, cx), Some("chat"));
}

/// What opens over a window's desk is heard here: anything opening lets
/// go of the desk's hold; a device approved closes "Add a device…", and
/// its closing forgets what it found.
#[gpui_kit::test]
fn the_windows_follow_what_opens_over_a_desk(cx: &mut TestAppContext) {
    let (app, key) = console(cx);
    let (desk, overlays) = app.windows.read_with(cx, |windows, _| {
        let own = windows.own(key).unwrap();
        (own.desk.clone(), own.overlays.clone())
    });
    let held = |cx: &TestAppContext| desk.read_with(cx, |desk, _| desk.get().held);
    desk.update(cx, |desk, cx| desk.hold(0, cx));
    assert!(held(cx).is_some());
    overlays.update(cx, |it, cx| it.open(Overlay::Spotlight, cx));
    assert_eq!(held(cx), None, "Search opened over a hold");

    overlays.update(cx, |it, cx| it.open(Overlay::Approve, cx));
    app.account.update(cx, |account, cx| {
        let key = vec![7; 32];
        let mut state = account.get().clone();
        state.approve = Some(crate::backend::join::fingerprint(&key));
        let found = crate::backend::join::Request {
            network: "testkit".into(),
            key,
        };
        account.seed(state, None, Some(found), cx);
    });
    app.account
        .update(cx, |account, cx| account.approve_done(Ok(()), cx));
    cx.run_until_parked();
    assert_eq!(overlays.read_with(cx, |it, _| *it.get()), None);
    let found = app
        .account
        .read_with(cx, |account, _| account.get().approve.clone());
    assert_eq!(found, None, "Approve closed and kept what it found");
    let toast = app.toast.read_with(cx, |toast, _| toast.get().clone());
    assert!(toast.starts_with("Approved."), "{toast:?}");
}
