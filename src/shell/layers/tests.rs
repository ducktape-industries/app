//! The layers' fixture: a console window drawn from a `Seed` (the app's
//! stores and the entities' values), opened the way the app opens it
//! (`Windows::open`), as `panes_tests::console` and `screens_tests::open`
//! build theirs; and the calls the tests make on the entities, as the
//! controls make them.
use super::super::entities::{
    self, AccountState, Chain, Desk, Entities, Overlay, Prefs, Screen, Secret, SessionState,
};
use super::super::{PaneMessage, WindowKey, WindowKind, keys};
use super::WindowRoot;
use crate::Ducktape;
use crate::ui::layout::Layout;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, point, px,
    size,
};

/// A screen state to draw: the app's stores (roster, centre), what
/// `Session`, `Chain`, `Account`, `Screen`, `Prefs` and `Toast` hold, the
/// program in front, and the console's desk as it stands.
pub(in crate::shell) struct Seed {
    pub(in crate::shell) state: Ducktape,
    pub(in crate::shell) session: SessionState,
    pub(in crate::shell) chain: Chain,
    pub(in crate::shell) account: AccountState,
    pub(in crate::shell) screen: Screen,
    /// A new recovery key's words, on the phrase screens.
    pub(in crate::shell) phrase: Option<Secret>,
    /// The request "Add a device…" found.
    pub(in crate::shell) found: Option<crate::backend::join::Request>,
    /// This device's prefs (a test's file starts empty: System, motion on).
    pub(in crate::shell) prefs: Prefs,
    /// The program in front (`Windows.active`): what an untouched desk opens.
    pub(in crate::shell) active: Option<&'static str>,
    /// The console's desk as it stands; `None` starts it empty.
    pub(in crate::shell) layout: Option<Layout>,
    /// The notice up.
    pub(in crate::shell) toast: String,
}

impl Seed {
    /// A fresh boot: the Connect screen, off every network.
    pub(in crate::shell) fn boot() -> Self {
        Ducktape::boot().into()
    }
}

impl From<Ducktape> for Seed {
    fn from(state: Ducktape) -> Self {
        Self {
            state,
            session: SessionState::booted(),
            chain: Chain::default(),
            account: AccountState::default(),
            screen: Screen::Connect,
            phrase: None,
            found: None,
            prefs: Prefs::load(),
            active: None,
            layout: None,
            toast: String::new(),
        }
    }
}

/// The window every test draws in: the desk's size.
pub(in crate::shell) const WINDOW: (f32, f32) = (1280., 800.);

/// A console window over `seed`, opened as the app opens it
/// (`Windows::open`), drawn once, its first frame's callbacks delivered
/// (the desk's size and its seed reach the `Desk` from there, not from
/// the draw).
pub(in crate::shell) fn open_console(
    seed: impl Into<Seed>,
    cx: &mut TestAppContext,
) -> (Entities, WindowKey, Entity<WindowRoot>, VisualTestContext) {
    let Seed {
        state,
        session,
        chain,
        account,
        screen,
        phrase,
        found,
        prefs,
        active,
        layout,
        toast,
    } = seed.into();
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let app = cx.update(|cx| {
        let app = entities::Entities::for_test(&state, cx);
        // a window that closes is forgotten, as `launch::run` wires it
        let windows = app.windows.downgrade();
        cx.on_window_closed(move |cx, id| {
            let windows = windows.clone();
            cx.defer(move |cx| {
                let _ = windows.update(cx, |windows, cx| windows.closed_id(id, cx));
            });
        })
        .detach();
        app.session.update(cx, |it, cx| it.seed(session, cx));
        app.chain.update(cx, |it, cx| {
            it.set(chain, cx);
        });
        app.account
            .update(cx, |it, cx| it.seed(account, phrase, found, cx));
        app.screen.update(cx, |it, cx| {
            it.set(screen, cx);
        });
        app.prefs.update(cx, |it, cx| {
            it.set(prefs, cx);
        });
        if !toast.is_empty() {
            app.toast.update(cx, |it, cx| it.show(toast, cx));
        }
        app.windows.update(cx, |it, _| it.set_active(active));
        app
    });
    let at = Bounds::new(point(px(0.), px(0.)), size(px(WINDOW.0), px(WINDOW.1)));
    let key = app.windows.update(cx, |windows, cx| {
        windows.open(WindowKind::Console, Some(at), cx)
    });
    if let Some(layout) = layout {
        let desk = desk_of(&app, key, cx);
        desk.update(cx, |desk, cx| {
            desk.set(layout, cx);
        });
    }
    let (handle, view) = app.windows.read_with(cx, |windows, _| {
        (
            windows.handles()[&key],
            windows.views()[&key]
                .upgrade()
                .expect("the console's root lives"),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    (app, key, view, VisualTestContext::from_window(handle, cx))
}

/// Window `key`'s desk.
fn desk_of(app: &Entities, key: WindowKey, cx: &TestAppContext) -> Entity<Desk> {
    app.windows.read_with(cx, |windows, _| {
        windows.own(key).expect("its window").desk.clone()
    })
}

/// The node's status poll answered `status`, as `Session`'s poll lands it.
pub(in crate::shell) fn polled(
    app: &Entities,
    status: crate::backend::NodeStatus,
    native: &mut VisualTestContext,
) {
    app.session.update(native, |session, cx| {
        session.status_answered(Ok(status), cx)
    });
    native.run_until_parked();
}

/// The app's entities, off `view`.
pub(in crate::shell) fn entities(
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> entities::Entities {
    native.update(|_, cx| view.read(cx).app.clone())
}

/// `screen` on the console, the way the account's flows put it there.
pub(in crate::shell) fn set_screen(
    view: &Entity<WindowRoot>,
    screen: Screen,
    native: &mut VisualTestContext,
) {
    let entities = entities(view, native);
    native.update(|_, cx| {
        entities.screen.update(cx, |it, cx| {
            it.set(screen, cx);
        })
    });
}

/// `overlay` open over `view`'s desk, or whatever is open closed, the way
/// the bar and the keys do it.
pub(in crate::shell) fn show(
    view: &Entity<WindowRoot>,
    overlay: Option<Overlay>,
    native: &mut VisualTestContext,
) {
    native.update(|_, cx| {
        let overlays = view.read(cx).overlays();
        overlays.update(cx, |it, cx| match (overlay, *it.get()) {
            (Some(overlay), _) => it.open(overlay, cx),
            (None, Some(open)) => it.close(open, cx),
            (None, None) => {}
        });
    });
}

/// What is open over `view`'s desk.
pub(in crate::shell) fn open_now(
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> Option<Overlay> {
    native.update(|_, cx| *view.read(cx).overlays().read(cx).get())
}

/// `spot` run from Search over `view`'s desk, as its row's click runs it:
/// Search closes, and what it picked runs.
pub(in crate::shell) fn run_spot(
    view: &Entity<WindowRoot>,
    spot: entities::Spot,
    native: &mut VisualTestContext,
) {
    native.update(|_, cx| {
        let (overlays, app) = (view.read(cx).overlays(), view.read(cx).app.clone());
        super::overlays::run(&overlays, &app, Some(spot), cx);
    });
}

/// `message` on `view`'s desk, the way a control moves it with no window
/// drawing between (`WindowRoot::pane_message` marks the panes moved too;
/// this is the desk alone).
pub(in crate::shell) fn pane(
    view: &Entity<WindowRoot>,
    message: PaneMessage,
    native: &mut VisualTestContext,
) {
    native.update(|_, cx| {
        let desk = view.read(cx).desk.clone();
        desk.update(cx, |desk, cx| desk.moved_by(message, cx));
    });
    native.run_until_parked();
}

/// A program picked (Spotlight, a menu): `Windows::select_view`.
pub(in crate::shell) fn select_view(
    app: &Entities,
    module: &'static str,
    native: &mut VisualTestContext,
) {
    app.windows
        .update(native, |windows, cx| windows.select_view(module, cx));
    native.run_until_parked();
}

/// Help asked for (⌘/, a menu): `Windows::help_asked`.
pub(in crate::shell) fn open_help(app: &Entities, native: &mut VisualTestContext) {
    app.windows
        .update(native, |windows, cx| windows.help_asked(cx));
    native.run_until_parked();
}

/// A notice up: `Toast::show`.
pub(in crate::shell) fn toast(app: &Entities, said: &str, native: &mut VisualTestContext) {
    app.toast
        .update(native, |toast, cx| toast.show(said.to_owned(), cx));
    native.run_until_parked();
}

/// Motion on or off: `Prefs::set_motion`.
pub(in crate::shell) fn set_motion(app: &Entities, on: bool, native: &mut VisualTestContext) {
    app.prefs
        .update(native, |prefs, cx| prefs.set_motion(on, cx));
    native.run_until_parked();
}

/// The program in front (`Windows.active`).
pub(in crate::shell) fn active(
    app: &Entities,
    native: &mut VisualTestContext,
) -> Option<&'static str> {
    app.windows.read_with(native, |windows, _| windows.active())
}

/// The console's front pane popped out to a window of its own, as its
/// strip's button does it: the new window's key, handle and root.
pub(in crate::shell) fn pop_out(
    app: &Entities,
    console: WindowKey,
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> (WindowKey, AnyWindowHandle, Entity<WindowRoot>) {
    let index = native.update(|_, cx| view.read(cx).layout(cx).focused);
    app.windows.update(native, |windows, cx| {
        windows.pop_out(console, index, None, cx)
    });
    native.run_until_parked();
    popped(app, console, native)
}

/// The one window that is not the console: its key, handle and root.
pub(in crate::shell) fn popped(
    app: &Entities,
    console: WindowKey,
    native: &mut VisualTestContext,
) -> (WindowKey, AnyWindowHandle, Entity<WindowRoot>) {
    app.windows.read_with(native, |windows, _| {
        let (&key, handle) = windows
            .handles()
            .iter()
            .find(|(candidate, _)| **candidate != console)
            .expect("a window of its own opened");
        let view = windows.views()[&key]
            .upgrade()
            .expect("the pop-out's root lives");
        (key, *handle, view)
    })
}
