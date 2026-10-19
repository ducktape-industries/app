//! The arrangement kept across launches: the windows an app had as it quit, or restarted to update,
//! are the ones the next launch on that network opens, each at its bounds with its panes and routes.
use super::arrangement::{Arrangement, Open, on_a_display};
use super::tests::{desk_of, entities};
use super::{Entities, Screen, SessionState, Windows};
use crate::runtime::WindowKey;
use crate::shell::WindowKind;
use crate::ui::layout::{Frame, Layout};
use gpui_kit::{Bounds, Pixels, TestAppContext, WindowBounds, point, px, size};

const HOME: &str = "home#0a0a0a0a";
const DESK: (f32, f32) = (1180., 744.);

fn launched(chain: &str, cx: &mut TestAppContext) -> (Entities, WindowKey) {
    let app = cx.update(|cx| {
        gpui_kit::init(cx);
        let app = entities(cx);
        Windows::forget_closed(&app.windows, cx);
        app
    });
    app.screen.update(cx, |screen, cx| {
        screen.set(Screen::Connect, cx);
    });
    let console = app.windows.update(cx, |windows, cx| {
        windows.open(WindowKind::Console, None, None, cx)
    });
    cx.run_until_parked();
    let state = SessionState {
        connected: true,
        chain: chain.into(),
        ..SessionState::default()
    };
    app.session
        .update(cx, |session, cx| session.seed(state, cx));
    app.screen.update(cx, |screen, cx| {
        screen.set(Screen::Desk, cx);
    });
    cx.run_until_parked();
    (app, console)
}

fn at(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

fn handle(app: &Entities, key: WindowKey, cx: &TestAppContext) -> gpui_kit::AnyWindowHandle {
    app.windows
        .read_with(cx, |windows, _| windows.handles()[&key])
}

fn bounds_of(app: &Entities, key: WindowKey, cx: &mut TestAppContext) -> Bounds<Pixels> {
    let handle = handle(app, key, cx);
    cx.update(|cx| handle.update(cx, |_, window, _| window.bounds()))
        .unwrap()
}

fn place(app: &Entities, key: WindowKey, to: Bounds<Pixels>, cx: &mut TestAppContext) {
    let handle = handle(app, key, cx);
    cx.update(|cx| {
        handle.update(cx, |_, window, cx| {
            window.set_bounds(to);
            window.bounds_changed(cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
}

fn close(app: &Entities, key: WindowKey, cx: &mut TestAppContext) {
    let handle = handle(app, key, cx);
    cx.update(|cx| handle.update(cx, |_, window, _| window.remove_window()))
        .unwrap();
    cx.run_until_parked();
}

type Shape = (
    Vec<(&'static str, Option<Frame>, u64, bool, Option<String>)>,
    usize,
    bool,
);

fn shape(app: &Entities, key: WindowKey, cx: &TestAppContext) -> Shape {
    desk_of(app, key, cx).read_with(cx, |desk, _| {
        let layout = desk.get();
        let panes = layout.panes.iter().map(|pane| {
            let showing = pane.showing.clone();
            (pane.program, pane.frame, pane.z, pane.by_hand, showing)
        });
        (panes.collect(), layout.focused, layout.filled)
    })
}

fn routed(app: &Entities, key: WindowKey, cx: &TestAppContext) -> Vec<Option<String>> {
    desk_of(app, key, cx).read_with(cx, |desk, _| {
        let panes = desk.get().panes.iter();
        panes
            .map(|pane| crate::runtime::take_route(pane.program, pane.instance))
            .collect()
    })
}

fn keys(app: &Entities, cx: &TestAppContext) -> Vec<WindowKey> {
    app.windows
        .read_with(cx, |windows, _| windows.handles().keys().copied().collect())
}

#[gpui_kit::test]
fn a_restart_opens_the_windows_it_quit_with(cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
    let (before, console) = launched(HOME, cx_a);
    let desk = desk_of(&before, console, cx_a);
    desk.update(cx_a, |desk, cx| {
        desk.resize(DESK, cx);
        desk.seed("chat", cx);
        desk.split("calendar", cx);
        desk.split("files", cx);
        let left = Frame {
            x: 40.,
            y: 30.,
            w: 700.,
            h: 500.,
        };
        let right = Frame {
            x: 520.,
            y: 180.,
            w: 640.,
            h: 480.,
        };
        desk.set_frame(0, left, cx);
        desk.set_frame(1, right, cx);
    });
    let instances = desk.read_with(cx_a, |desk, _| {
        let panes = desk.get().panes.iter();
        panes.map(|pane| pane.instance).collect::<Vec<_>>()
    });
    desk.update(cx_a, |desk, cx| {
        for (instance, route) in instances.iter().zip(["general", "2026-10", "home/tech"]) {
            desk.set_showing(*instance, route.into(), cx);
        }
    });
    let popped = before
        .windows
        .update(cx_a, |windows, cx| {
            windows.pop_out(console, 2, Some(at(900., 120., 640., 520.)), None, cx)
        })
        .expect("a window of its own");
    cx_a.run_until_parked();
    desk.update(cx_a, |desk, cx| desk.focus(0, cx));
    let (console_at, popped_at) = (at(60., 40., 1200., 780.), at(980., 200., 600., 500.));
    place(&before, console, console_at, cx_a);
    place(&before, popped, popped_at, cx_a);
    before
        .windows
        .update(cx_a, |windows, cx| windows.set_on_top(popped, true, cx));
    cx_a.run_until_parked();
    let on_top = |app: &Entities, key, cx: &TestAppContext| {
        app.windows.read_with(cx, |windows, _| windows.on_top(key))
    };
    assert!(
        on_top(&before, popped, cx_a),
        "the platform took Keep on Top"
    );
    let kept = (shape(&before, console, cx_a), shape(&before, popped, cx_a));
    assert_eq!(kept.0.0.len(), 2, "two panes on the desk");
    before.windows.update(cx_a, |windows, cx| windows.quit(cx));
    close(&before, popped, cx_a);
    close(&before, console, cx_a);

    let (after, console) = launched(HOME, cx_b);
    let opened = keys(&after, cx_b);
    assert_eq!(opened.len(), 2, "the pop-out came back: {opened:?}");
    let popped = *opened.iter().find(|key| **key != console).unwrap();
    assert_eq!(bounds_of(&after, console, cx_b), console_at);
    assert_eq!(bounds_of(&after, popped, cx_b), popped_at);
    assert_eq!(
        (shape(&after, console, cx_b), shape(&after, popped, cx_b)),
        kept
    );
    assert_eq!(
        routed(&after, console, cx_b),
        [Some("general".to_owned()), Some("2026-10".to_owned())],
        "each pane's view is handed the room it showed"
    );
    assert_eq!(routed(&after, popped, cx_b), [Some("home/tech".to_owned())]);
    assert!(
        on_top(&after, popped, cx_b),
        "the pop-out is kept on top again"
    );
    assert!(!on_top(&after, console, cx_b), "the console was not");
}

#[gpui_kit::test]
fn each_network_keeps_its_own_windows(cx_a: &mut TestAppContext, cx_b: &mut TestAppContext) {
    let (before, console) = launched(HOME, cx_a);
    let desk = desk_of(&before, console, cx_a);
    desk.update(cx_a, |desk, cx| {
        desk.resize(DESK, cx);
        desk.seed("chat", cx);
        desk.split("files", cx);
    });
    before.windows.update(cx_a, |windows, cx| {
        windows.pop_out(console, 1, None, None, cx)
    });
    cx_a.run_until_parked();
    assert_eq!(keys(&before, cx_a).len(), 2);
    before.windows.update(cx_a, |windows, cx| windows.quit(cx));

    let (after, console) = launched("there#0b0b0b0b", cx_b);
    assert_eq!(keys(&after, cx_b), [console], "another network's windows");
    assert!(shape(&after, console, cx_b).0.is_empty());
}

#[test]
fn full_size_and_a_float_read_back_as_they_were_kept() {
    let frame = at(10., 20., 800., 600.);
    let mut console = Layout::default();
    console.split("chat");
    let mut call = Layout::default();
    call.split("call");
    let mut files = Layout::default();
    files.split("files");
    let empty = Layout::default();
    let open = |console, floated, bounds, layout| Open {
        console,
        floated,
        on_top: console,
        bounds,
        layout,
    };
    let arrangement = Arrangement::of([
        open(true, false, WindowBounds::Fullscreen(frame), &console),
        open(false, true, WindowBounds::Windowed(frame), &call),
        open(false, false, WindowBounds::Maximized(frame), &files),
        open(false, false, WindowBounds::Windowed(frame), &empty),
    ]);
    let bytes = serde_json::to_vec(&arrangement).unwrap();
    let read: Arrangement = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(read, arrangement);
    let bounds: Vec<_> = read.windows.iter().map(|window| window.bounds()).collect();
    assert_eq!(
        bounds,
        [
            WindowBounds::Fullscreen(frame),
            WindowBounds::Maximized(frame)
        ],
        "an empty window is not kept"
    );
    let programs = |index: usize| -> Vec<&'static str> {
        let layout = read.windows[index].desk.layout();
        layout.panes.iter().map(|pane| pane.program).collect()
    };
    assert_eq!(programs(0), ["chat", "call"], "a float lands on the desk");
    assert_eq!(programs(1), ["files"]);
    let on_top: Vec<bool> = read.windows.iter().map(|window| window.on_top).collect();
    assert_eq!(on_top, [true, false], "Keep on Top reads back as kept");
    let mut older = serde_json::to_value(&read.windows[0]).unwrap();
    older.as_object_mut().unwrap().remove("on_top");
    let older: super::arrangement::KeptWindow = serde_json::from_value(older).unwrap();
    assert!(!older.on_top, "a record kept before it is among the others");
    let display = at(0., 0., 1920., 1080.);
    assert!(on_a_display(frame, &[display]));
    assert!(!on_a_display(at(4000., 0., 800., 600.), &[display]));
    assert!(on_a_display(frame, &[]), "no display known: kept as it was");
}
