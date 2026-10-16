//! The windows, moved by their methods (windows.rs), and what the app-wide
//! entities around them do on their own: a toast's clock, a pref's write,
//! the theme's sync, the network left.
use super::tests::{active, desk_of, entities, notifies, programs};
use super::{Entities, Overlay, SessionState, Windows};
use crate::runtime::WindowKey;
use crate::shell::WindowKind;
use crate::ui::layout::{EMPTY, Frame};
use gpui_kit::{AppContext as _, Bounds, TestAppContext, VisualTestContext, point, px, size};

const DESK: (f32, f32) = (1280., 764.);

/// The app's entities with a console open on a measured desk showing
/// "chat", its OS window on the test platform; a window that closes is
/// forgotten, as `launch::run` wires it.
fn console(cx: &mut TestAppContext) -> (Entities, WindowKey) {
    let app = cx.update(|cx| {
        gpui_kit::init(cx);
        let app = entities(cx);
        Windows::forget_closed(&app.windows, cx);
        app
    });
    let at = Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(800.)));
    let key = app.windows.update(cx, |windows, cx| {
        windows.open(WindowKind::Console, Some(at), None, cx)
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

fn listed(app: &Entities, programs: &[&str], cx: &mut TestAppContext) {
    let roster = crate::runtime::Roster::listing(programs);
    app.rail.update(cx, |rail, cx| rail.read_off(roster, cx));
}

/// The route waiting for the seat of window `key`'s pane showing `program`,
/// taken; none while no pane shows it.
fn routed(
    app: &Entities,
    key: WindowKey,
    program: &'static str,
    cx: &TestAppContext,
) -> Option<String> {
    let layout = desk_of(app, key, cx).read_with(cx, |desk, _| desk.get().clone());
    let pane = layout.panes.iter().find(|pane| pane.program == program)?;
    crate::runtime::take_route(program, pane.instance)
}

/// A view picked from ⌘K or a menu opens as a bar click does: the focused
/// window keeps its program, and the pick is the program in front.
#[gpui_kit::test]
fn a_picked_view_opens_beside_the_focused_one(cx: &mut TestAppContext) {
    let (app, key) = console(cx);
    let pick = |program, cx: &mut TestAppContext| {
        app.windows
            .update(cx, |windows, cx| windows.select_view(program, cx))
    };
    pick("files", cx);
    assert_eq!(programs(&app, key, cx), ["chat", "files"]);
    assert_eq!(active(&app, cx), Some("files"));
    pick("chat", cx);
    assert_eq!(
        programs(&app, key, cx),
        ["chat", "files"],
        "the window it is in"
    );
    assert_eq!(active(&app, cx), Some("chat"));
    // a desk of many windows opens one more of its own
    let desk = desk_of(&app, key, cx);
    for _ in 0..16 {
        desk.update(cx, |desk, cx| desk.split("files", cx));
    }
    let before = desk.read_with(cx, |desk, _| desk.get().panes.len());
    pick("calendar", cx);
    assert_eq!(
        desk.read_with(cx, |desk, _| desk.get().shown()),
        Some("calendar")
    );
    assert_eq!(
        desk.read_with(cx, |desk, _| desk.get().panes.len()),
        before + 1
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
    app.windows.update(cx, |windows, cx| {
        windows.pop_out(console, 1, None, None, cx)
    });
    cx.run_until_parked();
    assert_eq!(programs(&app, console, cx), ["chat"]);
    let popped_key = popped(&app, console, cx);
    assert_eq!(window_count(&app, cx), 2, "the pop-out's window opened");
    assert_eq!(
        programs(&app, popped_key, cx),
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
        windows.pop_out(console, focused, None, None, cx)
    });
    cx.run_until_parked();
    assert_eq!(window_count(&app, cx), 1);
    assert_eq!(programs(&app, console, cx), ["chat", "files", EMPTY]);
}

/// The AX door names a window for its life: the console closing renames
/// no other window, and the console opening after it is "console" again.
#[gpui_kit::test]
fn a_window_keeps_its_door_name(cx: &mut TestAppContext) {
    let (app, console) = console(cx);
    let desk = desk_of(&app, console, cx);
    desk.update(cx, |desk, cx| desk.split("files", cx));
    app.windows.update(cx, |windows, cx| {
        windows.pop_out(console, 1, None, None, cx)
    });
    cx.run_until_parked();
    let popped_key = popped(&app, console, cx);
    let names = |cx: &TestAppContext| {
        app.windows.read_with(cx, |windows, _| {
            windows
                .served()
                .into_iter()
                .map(|(name, key, _)| (key, name))
                .collect::<Vec<_>>()
        })
    };
    let before = names(cx);
    assert_eq!(before.len(), 2);
    assert_eq!(before[0], (console, "console".to_owned()));
    let popped_name = before[1].1.clone();
    assert_ne!(popped_name, "console");
    let handle = app
        .windows
        .read_with(cx, |windows, _| windows.handles()[&console]);
    cx.update_window(handle, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        names(cx),
        [(popped_key, popped_name.clone())],
        "the pop-out kept its name"
    );
    app.windows
        .update(cx, |windows, cx| windows.raise_console(cx));
    cx.run_until_parked();
    let reopened = app
        .windows
        .read_with(cx, |windows, _| windows.console())
        .expect("a console");
    assert_eq!(
        names(cx),
        [(popped_key, popped_name), (reopened, "console".to_owned())]
    );
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
            .update(cx, |windows, cx| windows.open_link(link, None, cx))
    };
    open("duck://testkit-0a1b2c3d/link-test-here/tx/00ff", cx);
    assert_eq!(active(&app, cx), Some("link-test-here"));
    assert_eq!(programs(&app, key, cx), ["chat", "link-test-here"]);
    assert!(toast(cx).is_empty(), "the view coming forward says it");
    assert_eq!(
        routed(&app, key, "link-test-here", cx).as_deref(),
        Some("tx/00ff"),
        "the pane the link opened was not handed its route"
    );
    // another chain's link opens the seat, routes nothing, and says why
    open("duck://othernet-0a1b2c3d/link-test-away/tx/00ff", cx);
    assert_eq!(active(&app, cx), Some("link-test-away"));
    assert_eq!(routed(&app, key, "link-test-away", cx), None);
    assert!(toast(cx).contains("othernet"), "{:?}", toast(cx));
    // a view nobody lists opens nothing
    app.toast.update(cx, |toast, cx| toast.dismiss(cx));
    open("duck://testkit-0a1b2c3d/link-test-nowhere/x", cx);
    assert_eq!(active(&app, cx), Some("link-test-away"));
    assert!(toast(cx).contains("link-test-nowhere"), "{:?}", toast(cx));
    assert_eq!(
        programs(&app, key, cx),
        ["chat", "link-test-here", "link-test-away"]
    );
}

/// An http(s) link opens in the view whose manifest declares `web`,
/// handed the whole address as its route, in a build with a web view;
/// without one, or for an address no page may load (plain http to another
/// machine), the system browser takes it. The live runner shows one view,
/// so this test is the routing's proof.
#[gpui_kit::test]
fn an_https_link_opens_in_the_web_view_else_in_the_system_browser(cx: &mut TestAppContext) {
    use view_wire::methods::Capability;
    let (app, key) = console(cx);
    listed(&app, &["web-link-test"], cx);
    crate::render::web::seen("web-link-test", &[Capability::Host, Capability::Web]);
    let open = |link: &str, cx: &mut TestAppContext| {
        app.windows
            .update(cx, |windows, cx| windows.open_link(link, None, cx))
    };
    open("https://example.com/a?b=1", cx);
    if cfg!(feature = "web") {
        assert_eq!(active(&app, cx), Some("web-link-test"));
        assert_eq!(
            routed(&app, key, "web-link-test", cx).as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(cx.opened_url(), None);
    } else {
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(routed(&app, key, "web-link-test", cx), None);
    }
    open("http://example.com/b", cx);
    assert_eq!(cx.opened_url().as_deref(), Some("http://example.com/b"));
    assert_eq!(routed(&app, key, "web-link-test", cx), None);
    // a view whose next manifest drops `web` takes no more links
    crate::render::web::seen("web-link-test", &[Capability::Host]);
    open("https://example.com/c", cx);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/c"));
}

/// A click on the update notice (`runtime::notify` posts `OpenUpdates`):
/// Settings opens on the console, on its Updates page.
#[gpui_kit::test]
fn the_update_notice_opens_settings_on_updates(cx: &mut TestAppContext) {
    let (app, key) = console(cx);
    let overlays = app
        .windows
        .read_with(cx, |windows, _| windows.own(key).unwrap().overlays.clone());
    app.windows.update(cx, |windows, cx| {
        windows.posted(super::windows::Posted::OpenUpdates, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        overlays.read_with(cx, |it, _| *it.get()),
        Some(Overlay::Settings(super::SettingsPage::Updates))
    );
}

/// Two windows of one program: a link from outside them (a notice,
/// Spotlight, the OS) goes to the one `open_program` brings forward and
/// to no other; with the program only in a pop-out, to the pop-out it
/// raises, and the desk opens nothing.
#[gpui_kit::test]
fn an_outside_link_reaches_only_the_window_it_brings_forward(cx: &mut TestAppContext) {
    const PROGRAM: &str = "two-window-link";
    let (app, console) = console(cx);
    listed(&app, &[PROGRAM], cx);
    let desk = desk_of(&app, console, cx);
    desk.update(cx, |desk, cx| {
        desk.split(PROGRAM, cx);
        desk.split(PROGRAM, cx);
    });
    let open = |cx: &mut TestAppContext| {
        app.windows.update(cx, |windows, cx| {
            windows.open_link(&format!("duck://{PROGRAM}/room/9"), None, cx)
        })
    };
    open(cx);
    let layout = desk.read_with(cx, |desk, _| desk.get().clone());
    let forward = layout.panes[layout.focused].instance;
    let other = layout
        .panes
        .iter()
        .find(|pane| pane.program == PROGRAM && pane.instance != forward)
        .expect("two windows of the program")
        .instance;
    assert_eq!(
        crate::runtime::take_route(PROGRAM, forward).as_deref(),
        Some("room/9")
    );
    assert_eq!(
        crate::runtime::take_route(PROGRAM, other),
        None,
        "the window left behind was handed the route too"
    );
    // the program in a pop-out only
    let at = |instance, cx: &TestAppContext| {
        let layout = desk.read_with(cx, |desk, _| desk.get().clone());
        layout
            .panes
            .iter()
            .position(|pane| pane.instance == instance)
    };
    let index = at(other, cx).unwrap();
    desk.update(cx, |desk, cx| _ = desk.close(index, cx));
    let index = at(forward, cx).unwrap();
    app.windows.update(cx, |windows, cx| {
        windows.pop_out(console, index, None, None, cx)
    });
    cx.run_until_parked();
    assert_eq!(programs(&app, console, cx), ["chat"]);
    open(cx);
    assert_eq!(
        programs(&app, console, cx),
        ["chat"],
        "the desk opened it again"
    );
    assert_eq!(
        crate::runtime::take_route(PROGRAM, forward).as_deref(),
        Some("room/9"),
        "the pop-out was not handed the route"
    );
}

/// The same link notice while it is up is not shown again, so a view
/// repeating a bad link cannot keep the notice up past its time.
#[gpui_kit::test]
fn a_repeated_link_notice_does_not_restart_the_toast(cx: &mut TestAppContext) {
    let (app, _) = console(cx);
    listed(&app, &["notice-test"], cx);
    let toast = |cx: &TestAppContext| app.toast.read_with(cx, |toast, _| toast.get().clone());
    let open = |cx: &mut TestAppContext| {
        app.windows.update(cx, |windows, cx| {
            windows.open_link("duck://testkit-0a1b2c3d/notice-test-nowhere/x", None, cx)
        })
    };
    open(cx);
    assert!(toast(cx).contains("notice-test-nowhere"), "{:?}", toast(cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(3000));
    cx.run_until_parked();
    open(cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(700));
    cx.run_until_parked();
    assert!(
        toast(cx).is_empty(),
        "the repeated notice restarted the toast"
    );
}

/// A pick and a link both land in the console's desk at once, each of
/// them; leaving the network empties every window, keeping its measure,
/// closes what is open over it, and takes the badges, the active program
/// and the program list with it: a link then opens nothing, never a
/// program of the network left.
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
        windows.open_link("duck://pane-link/x", None, cx);
    });
    assert_eq!(programs(&app, key, cx), ["chat", "forge", "pane-link"]);
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
    assert!(
        app.rail.read_with(cx, |rail, _| rail.rows().is_empty()),
        "the left network's programs are still listed"
    );
    app.windows.update(cx, |windows, cx| {
        windows.open_link("duck://pane-link/x", None, cx)
    });
    assert_eq!(
        active(&app, cx),
        None,
        "a link opened a program of the network left"
    );
    assert!(programs(&app, key, cx).is_empty());
    assert!(!app.toast.read_with(cx, |toast, _| toast.get().is_empty()));
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

/// The door's walk puts the layout back as it found it, not picked, in
/// `Prefs` as in the file.
#[gpui_kit::test]
fn a_kept_layout_put_back_reaches_the_prefs_slice(cx: &mut TestAppContext) {
    use crate::backend::{Layout, load_layout};
    use crate::shell::layers::Kept;
    let (app, key) = console(cx);
    let handle = app
        .windows
        .read_with(cx, |windows, _| windows.handles()[&key]);
    let kept = cx
        .update_window(handle, |_, window, cx| Kept::of(window, cx))
        .unwrap()
        .expect("the console is the shell's");
    app.prefs
        .update(cx, |prefs, cx| prefs.set_layout(Layout::Sidebar, cx));
    cx.update(|cx| kept.restore(cx));
    let layout = app.prefs.read_with(cx, |prefs, _| prefs.get().layout);
    assert_eq!((layout, load_layout()), (None, None));
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
    assert_eq!(programs(&app, key, cx), ["chat", "front-test-view"]);
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
        state.approve = Some(noded_client::join::fingerprint(&key));
        let found = noded_client::join::Request {
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

/// A view's ask waits behind what the person has open: with "Add a
/// device…" up, the ask leaves it there (its card is the same frame, Approve
/// in the same spot: a view in a pop-out window could land it under a press
/// meant for the device); the card shows once that closes, and a bare desk
/// shows it at once.
#[gpui_kit::test]
fn a_views_ask_never_replaces_an_open_dialog(cx: &mut TestAppContext) {
    use crate::runtime::consent;
    let (app, key) = console(cx);
    let overlays = app
        .windows
        .read_with(cx, |windows, _| windows.own(key).unwrap().overlays.clone());
    let open = |cx: &TestAppContext| overlays.read_with(cx, |it, _| *it.get());
    let words = consent::Words {
        title: consent::ACCOUNT,
        said: "chat asks to suspend agent #3.".into(),
        id: None,
        shown: Some("#3".into()),
        described: false,
        card: Default::default(),
    };
    overlays.update(cx, |it, cx| it.open(Overlay::Approve, cx));
    let _told = consent::queue("chat", 0, words.clone()).expect("queued");
    app.windows
        .update(cx, |windows, cx| windows.sync_consent(cx));
    cx.run_until_parked();
    assert_eq!(open(cx), Some(Overlay::Approve), "the ask waits its turn");
    let (asked, _) = consent::front().expect("still waiting");
    overlays.update(cx, |it, cx| it.close(Overlay::Approve, cx));
    cx.run_until_parked();
    assert_eq!(open(cx), Some(Overlay::Consent(asked)), "its turn came");
    assert!(!consent::answer(asked, false));
    cx.run_until_parked();
    // the person's own close refuses it; nothing waits, nothing reopens
    overlays.update(cx, |it, cx| it.close(Overlay::Consent(asked), cx));
    cx.run_until_parked();
    assert_eq!(open(cx), None);
    let _told = consent::queue("chat", 0, words).expect("queued");
    app.windows
        .update(cx, |windows, cx| windows.sync_consent(cx));
    cx.run_until_parked();
    let (asked, _) = consent::front().unwrap();
    assert_eq!(
        open(cx),
        Some(Overlay::Consent(asked)),
        "a bare desk shows it at once"
    );
    consent::answer(asked, false);
    overlays.update(cx, |it, cx| it.close(Overlay::Consent(asked), cx));
    cx.run_until_parked();
}

/// A card whose request is gone (refused after it was queued, or its view
/// torn down) follows it off: the shell's sync closes it, and nothing is
/// left for Approve to tell.
#[gpui_kit::test]
fn a_card_follows_its_request_off(cx: &mut TestAppContext) {
    use crate::runtime::consent;
    let (app, key) = console(cx);
    let overlays = app
        .windows
        .read_with(cx, |windows, _| windows.own(key).unwrap().overlays.clone());
    let told = consent::queue(
        "chat",
        0,
        consent::Words {
            title: consent::ACCOUNT,
            said: "chat asks to suspend agent #3.".into(),
            id: None,
            shown: Some("#3".into()),
            described: false,
            card: Default::default(),
        },
    )
    .expect("queued");
    let (asked, _) = consent::front().unwrap();
    app.windows
        .update(cx, |windows, cx| windows.sync_consent(cx));
    cx.run_until_parked();
    assert_eq!(
        overlays.read_with(cx, |it, _| *it.get()),
        Some(Overlay::Consent(asked))
    );
    drop(told);
    app.windows
        .update(cx, |windows, cx| windows.sync_consent(cx));
    cx.run_until_parked();
    assert_eq!(
        overlays.read_with(cx, |it, _| *it.get()),
        None,
        "the card went with its request"
    );
    assert_eq!(consent::front(), None);
}

/// A window's first frame is the self-update's healthy signal: the window
/// asks for it as it opens, and it is sent once that frame comes round.
#[gpui_kit::test]
fn a_windows_first_frame_tells_the_updater_it_rendered(cx: &mut TestAppContext) {
    let before = crate::update::rendered_here();
    let (app, key) = console(cx);
    let handle = app
        .windows
        .read_with(cx, |windows, _| windows.handles()[&key]);
    cx.update_window(handle, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    assert_eq!(crate::update::rendered_here(), before + 1);
}

// ---------- Keep on Top ----------

/// What the test platform was told to keep above (window, on), since the
/// last look.
fn told_above() -> Vec<(gpui_kit::WindowId, bool)> {
    crate::shell::windows::KEPT_ABOVE.with_borrow_mut(std::mem::take)
}

fn on_top(app: &Entities, key: WindowKey, cx: &TestAppContext) -> bool {
    app.windows.read_with(cx, |windows, _| windows.on_top(key))
}

/// ⌘⇧T in a pop-out keeps that window above other apps' windows and no
/// other; pressed again, it lets it back among them.
#[gpui_kit::test]
fn keep_on_top_keeps_the_window_it_was_pressed_in_and_no_other(cx: &mut TestAppContext) {
    let (app, console, view, mut native) = crate::shell::panes_tests::console(cx);
    let (popped, handle, _) =
        crate::shell::layers::tests::pop_out(&app, console, &view, &mut native);
    told_above();
    let mut window = gpui_kit::VisualTestContext::from_window(handle, &native);
    window.simulate_keystrokes("secondary-shift-t");
    window.run_until_parked();
    assert!(on_top(&app, popped, &window), "the pop-out is kept on top");
    assert!(
        !on_top(&app, console, &window),
        "the console stays among the others"
    );
    assert_eq!(told_above(), [(handle.window_id(), true)]);
    window.simulate_keystrokes("secondary-shift-t");
    window.run_until_parked();
    assert!(!on_top(&app, popped, &window));
    assert_eq!(told_above(), [(handle.window_id(), false)]);
}

/// A window kept on top stays so as it is raised, loses and gains the
/// front and has a pane popped out of it and back (the platform is told
/// nothing again); the macOS menu's check follows the window in front; a
/// window that closes is forgotten.
#[gpui_kit::test]
fn keep_on_top_holds_through_raising_and_panes_moving(cx: &mut TestAppContext) {
    use crate::shell::layers::tests::pop_out;
    let (app, console, view, mut native) = crate::shell::panes_tests::console(cx);
    let console_window = app
        .windows
        .read_with(&native, |windows, _| windows.handles()[&console]);
    native.update(|window, _| window.activate_window());
    native.run_until_parked();
    told_above();
    app.windows.update(&mut native, |windows, cx| {
        windows.toggle_on_top(console, cx)
    });
    native.run_until_parked();
    let menu = |native: &VisualTestContext| {
        app.windows
            .read_with(native, |windows, _| windows.menu_on_top())
    };
    assert!(on_top(&app, console, &native));
    assert!(menu(&native), "the menu checks the window in front");
    assert_eq!(told_above(), [(console_window.window_id(), true)]);

    let (popped, handle, _) = pop_out(&app, console, &view, &mut native);
    native.update(|_, cx| {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .unwrap()
    });
    native.run_until_parked();
    assert!(!menu(&native), "the pop-out in front is not on top");
    app.windows
        .update(&mut native, |windows, cx| windows.toggle_on_top(popped, cx));
    native.run_until_parked();
    assert!(menu(&native));

    app.windows
        .update(&mut native, |windows, cx| windows.raise(console, cx));
    native.run_until_parked();
    assert!(on_top(&app, console, &native));
    assert!(menu(&native), "the console in front again");
    app.windows
        .update(&mut native, |windows, cx| windows.pop_in(popped, cx));
    native.run_until_parked();
    assert!(on_top(&app, console, &native), "its pane came back");
    assert!(
        !on_top(&app, popped, &native),
        "the closed window is forgotten"
    );
    assert_eq!(
        told_above(),
        [(handle.window_id(), true)],
        "nothing but the pop-out's own Keep on Top told the platform"
    );
}

/// Search's "Keep on top" acts on the console, where Search draws, and
/// reports ⌘⇧T as Help lists it (AX-114).
#[gpui_kit::test]
fn search_keeps_the_console_on_top(cx: &mut TestAppContext) {
    use crate::shell::entities::Spot;
    use crate::shell::layers::tests::{run_spot, show};
    let (app, console, view, mut native) = crate::shell::panes_tests::console(cx);
    show(&view, Some(Overlay::Spotlight), &mut native);
    let nodes = native.update(crate::shell::panes_tests::draw);
    let row = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["role"] == "ListBoxOption" && node["name"] == "Keep on top")
        .unwrap_or_else(|| panic!("no Keep on top row: {nodes}"));
    assert_eq!(
        row["keyboard_shortcut"],
        crate::shell::chord_label("⇧T").as_str()
    );
    told_above();
    run_spot(&view, Spot::KeepOnTop, &mut native);
    native.run_until_parked();
    assert!(on_top(&app, console, &native));
    assert_eq!(told_above().len(), 1);
}

// ---------- a view's float ----------

/// The huddle's float: its view laid out 320 by 200, never under 240 by
/// 150, kept on top.
const FLOAT: view_wire::methods::Float = view_wire::methods::Float {
    width: 320,
    height: 200,
    min_width: 240,
    min_height: 150,
    on_top: true,
};

/// What `program`'s view in the pane with `instance` asks next: `float`
/// (`host.float`, kept on top as `on_top` says), or `None` to unfloat
/// (`host.unfloat`).
fn ask(program: &str, instance: u64, float: Option<bool>) {
    use crate::runtime::{Intent, intent_for_test};
    let intent = match float {
        Some(on_top) => Intent::Float(view_wire::methods::Float { on_top, ..FLOAT }),
        None => Intent::Unfloat,
    };
    intent_for_test(program, instance, intent);
}

/// A console showing `program`'s view, its own minimum 560 (the huddle's),
/// alone on the desk and seated: the app, the console, its root, the
/// window, and the pane's instance.
fn floating_console(
    program: &'static str,
    cx: &mut TestAppContext,
) -> (
    Entities,
    WindowKey,
    gpui_kit::Entity<crate::shell::WindowRoot>,
    VisualTestContext,
    u64,
) {
    use crate::shell::PaneMessage;
    use crate::shell::layers::tests::pane;
    let (app, console, view, mut native) = crate::shell::panes_tests::console(cx);
    crate::runtime::seat_for_test(program, 560);
    pane(&view, PaneMessage::Select(program), &mut native);
    turn(&app, &mut native);
    let instance = native.update(|_, cx| {
        let layout = view.read(cx).layout(cx);
        layout.panes[layout.focused].instance
    });
    (app, console, view, native, instance)
}

/// Every seat turned, what they asked for routed, every window drawn.
fn turn(app: &Entities, native: &mut VisualTestContext) {
    for _ in 0..3 {
        app.seats.update(native, |seats, cx| seats.settle(cx));
        native.run_until_parked();
        let handles: Vec<_> = app.windows.read_with(native, |windows, _| {
            windows.handles().values().copied().collect()
        });
        for handle in handles {
            let _ = native.update(|_, cx| {
                handle.update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    window.simulate_next_frame(cx);
                })
            });
        }
        native.run_until_parked();
    }
}

/// The pane with `instance`: the window it is in and its seat's entity id.
fn pane_at(app: &Entities, instance: u64, native: &mut VisualTestContext) -> (WindowKey, u64) {
    native.update(|_, cx| {
        let windows = app.windows.read(cx);
        let key = windows
            .by_window()
            .iter()
            .find(|(_, own)| {
                own.desk
                    .read(cx)
                    .get()
                    .panes
                    .iter()
                    .any(|pane| pane.instance == instance)
            })
            .map(|(key, _)| *key)
            .expect("the pane is on a desk");
        let seat = app.seats.read(cx).seat(instance).expect("seated");
        (key, seat.entity_id().as_u64())
    })
}

/// THE HEADLINE: Join floats the huddle. The view's `host.float` takes its
/// pane, seat and all (the call's state lives there), into a window of its
/// own kept above every other app's; its view hears it floats and is laid
/// out at the float's 320 by 200, not at its own 560. The pin (a float
/// while floating) only lets it among the others and back, and the Window
/// menu's Keep on Top reaches the view too. Leave (`host.unfloat`) puts
/// the pane back on the desk, the same seat, the window gone, the view
/// told.
#[gpui_kit::test]
fn a_view_floats_its_pane_on_top_and_unfloat_puts_it_back(cx: &mut TestAppContext) {
    use crate::runtime::{placement_for_test, viewport_for_test};
    use view_wire::methods::Placement;
    const PROGRAM: &str = "float-view";
    let (app, console, _, mut native, instance) = floating_console(PROGRAM, cx);
    let (_, seat) = pane_at(&app, instance, &mut native);
    assert_eq!(placement_for_test(PROGRAM), Placement::default());
    told_above();

    ask(PROGRAM, instance, Some(true));
    turn(&app, &mut native);
    let (floated, carried) = pane_at(&app, instance, &mut native);
    assert_ne!(floated, console, "the pane is in a window of its own");
    assert_eq!(
        carried, seat,
        "the same seat: the view's state went with it"
    );
    assert!(programs(&app, console, &native).is_empty());
    let handle = app
        .windows
        .read_with(&native, |windows, _| windows.handles()[&floated]);
    assert!(on_top(&app, floated, &native), "kept on top");
    assert_eq!(told_above(), [(handle.window_id(), true)]);
    assert_eq!(
        placement_for_test(PROGRAM),
        Placement {
            floating: true,
            on_top: true
        }
    );
    assert_eq!(
        viewport_for_test(PROGRAM),
        Some((320., 200.)),
        "the float's size, not 560"
    );

    // the pin: floating already, Keep on Top alone follows
    ask(PROGRAM, instance, Some(false));
    turn(&app, &mut native);
    assert_eq!(pane_at(&app, instance, &mut native).0, floated);
    assert!(!on_top(&app, floated, &native));
    assert_eq!(told_above(), [(handle.window_id(), false)]);
    assert_eq!(
        placement_for_test(PROGRAM),
        Placement {
            floating: true,
            on_top: false
        },
        "the pin follows"
    );
    // the Window menu's Keep on Top (⌘⇧T) reaches the view too
    app.windows.update(&mut native, |windows, cx| {
        windows.toggle_on_top(floated, cx)
    });
    turn(&app, &mut native);
    assert!(placement_for_test(PROGRAM).on_top, "the menu's toggle");

    ask(PROGRAM, instance, None);
    turn(&app, &mut native);
    let (back, carried) = pane_at(&app, instance, &mut native);
    assert_eq!(back, console, "back on the desk");
    assert_eq!(carried, seat, "the same seat");
    assert_eq!(app.windows.read_with(&native, |w, _| w.handles().len()), 1);
    assert_eq!(
        app.windows.read_with(&native, |w, _| w.float_min(floated)),
        None,
        "the float is forgotten"
    );
    assert_eq!(placement_for_test(PROGRAM), Placement::default());
    // a second unfloat does nothing
    ask(PROGRAM, instance, None);
    turn(&app, &mut native);
    assert_eq!(pane_at(&app, instance, &mut native).0, console);
}

/// A view asks any size: its float fits the display it opens on, the
/// view laid out no larger, its minimum no larger than that.
#[gpui_kit::test]
fn a_float_fits_its_display(cx: &mut TestAppContext) {
    use crate::runtime::{Intent, intent_for_test, viewport_for_test};
    const PROGRAM: &str = "float-huge-view";
    let (app, console, _, mut native, instance) = floating_console(PROGRAM, cx);
    let huge = view_wire::methods::Float {
        width: 100_000,
        height: 100_000,
        min_width: 90_000,
        min_height: 90_000,
        on_top: false,
    };
    intent_for_test(PROGRAM, instance, Intent::Float(huge));
    turn(&app, &mut native);
    let (floated, _) = pane_at(&app, instance, &mut native);
    assert_ne!(floated, console, "it floats");
    let handle = app
        .windows
        .read_with(&native, |windows, _| windows.handles()[&floated]);
    let (bounds, display) = native.update(|_, cx| {
        handle
            .update(cx, |_, window, cx| {
                let display = window.display(cx).expect("a display").visible_bounds();
                (window.bounds(), display)
            })
            .unwrap()
    });
    assert!(
        bounds.size.width <= display.size.width && bounds.size.height <= display.size.height,
        "{bounds:?} past {display:?}"
    );
    let (width, height) = viewport_for_test(PROGRAM).expect("laid out");
    assert!(width <= f32::from(display.size.width) && height <= f32::from(display.size.height));
    let min = app
        .windows
        .read_with(&native, |windows, _| windows.float_min(floated))
        .expect("floating");
    assert!(min <= f32::from(display.size.width));
}

/// The float's own close button does not end what is in it: the pane
/// goes back on the desk as the window closes. The next float opens where
/// that one was left.
#[gpui_kit::test]
fn closing_a_float_puts_its_pane_back_and_the_next_opens_there(cx: &mut TestAppContext) {
    const PROGRAM: &str = "float-close-view";
    let (app, console, _, mut native, instance) = floating_console(PROGRAM, cx);
    ask(PROGRAM, instance, Some(true));
    turn(&app, &mut native);
    let (floated, _) = pane_at(&app, instance, &mut native);
    let handle = app
        .windows
        .read_with(&native, |windows, _| windows.handles()[&floated]);
    let mut window = VisualTestContext::from_window(handle, &native);
    let moved = Bounds::new(point(px(40.), px(60.)), size(px(400.), px(282.)));
    window.update(|window, _| window.set_bounds(moved));
    window.run_until_parked();
    assert!(window.simulate_close(), "the window closes");
    turn(&app, &mut native);
    assert_eq!(pane_at(&app, instance, &mut native).0, console, "back");
    // the platform closes it (the test platform only asks)
    window.update(|window, _| window.remove_window());
    turn(&app, &mut native);
    assert_eq!(app.windows.read_with(&native, |w, _| w.handles().len()), 1);

    ask(PROGRAM, instance, Some(true));
    turn(&app, &mut native);
    let (again, _) = pane_at(&app, instance, &mut native);
    let handle = app
        .windows
        .read_with(&native, |windows, _| windows.handles()[&again]);
    let bounds = native.update(|_, cx| handle.update(cx, |_, window, _| window.bounds()).unwrap());
    assert_eq!(bounds, moved, "where the last one was left");
}

/// A pop-out's close closes it, its pane with it: only a float's pane
/// goes back.
#[gpui_kit::test]
fn a_popouts_close_takes_its_pane(cx: &mut TestAppContext) {
    let (app, console, view, mut native) = crate::shell::panes_tests::console(cx);
    let before = programs(&app, console, &native).len();
    let (popped, handle, _) =
        crate::shell::layers::tests::pop_out(&app, console, &view, &mut native);
    let mut window = VisualTestContext::from_window(handle, &native);
    assert!(window.simulate_close());
    native.run_until_parked();
    assert_eq!(programs(&app, console, &native).len(), before - 1);
    assert_eq!(programs(&app, popped, &native).len(), 1, "still its own");
}

/// A view popped out by hand floats from its pop-out: the pane moves to
/// the float, and the pop-out, its one pane gone, goes with it.
#[gpui_kit::test]
fn a_float_from_a_popout_leaves_no_empty_window(cx: &mut TestAppContext) {
    const PROGRAM: &str = "float-popout-view";
    let (app, console, view, mut native, instance) = floating_console(PROGRAM, cx);
    let (popped, _, _) = crate::shell::layers::tests::pop_out(&app, console, &view, &mut native);
    turn(&app, &mut native);
    assert_eq!(pane_at(&app, instance, &mut native).0, popped);
    ask(PROGRAM, instance, Some(true));
    turn(&app, &mut native);
    let (floated, _) = pane_at(&app, instance, &mut native);
    assert!(floated != popped && floated != console);
    let keys: Vec<WindowKey> = app.windows.read_with(&native, |windows, _| {
        windows.handles().keys().copied().collect()
    });
    assert_eq!(keys, [console, floated], "the emptied pop-out went");
}
