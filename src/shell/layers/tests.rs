//! The layers' fixture: a console window drawn from a `Seed` (the app's
//! stores and the entities' values), opened the way the app opens it
//! (`Windows::open`), as `panes_tests::console` and `screens_tests::open`
//! build theirs; and the calls the tests make on the entities, as the
//! controls make them.
use super::super::entities::{
    self, AccountState, Chain, Entities, Overlay, Prefs, Screen, Secret, SessionState,
};
use super::super::{PaneMessage, WindowKey, WindowKind, keys};
use super::WindowRoot;
use crate::runtime::Roster;
use crate::runtime::notify::CenterHandle;
use crate::ui::layout::Layout;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Styled as _, TestAppContext,
    VisualTestContext, point, px, size,
};
use view_wire as wire;

pub(in crate::shell) struct Seed {
    pub(in crate::shell) roster: Roster,
    pub(in crate::shell) center: CenterHandle,
    pub(in crate::shell) session: SessionState,
    pub(in crate::shell) chain: Chain,
    pub(in crate::shell) account: AccountState,
    pub(in crate::shell) screen: Screen,
    pub(in crate::shell) phrase: Option<Secret>,
    pub(in crate::shell) found: Option<noded_client::join::Request>,
    pub(in crate::shell) prefs: Prefs,
    pub(in crate::shell) active: Option<&'static str>,
    pub(in crate::shell) layout: Option<Layout>,
    pub(in crate::shell) toast: String,
}

impl Seed {
    pub(in crate::shell) fn boot() -> Self {
        Self {
            roster: crate::runtime::roster().clone(),
            center: crate::runtime::notify::center().clone(),
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

pub(in crate::shell) const WINDOW: (f32, f32) = (1280., 800.);

pub(in crate::shell) fn open_console(
    seed: impl Into<Seed>,
    cx: &mut TestAppContext,
) -> (Entities, WindowKey, Entity<WindowRoot>, VisualTestContext) {
    let at = Bounds::new(point(px(0.), px(0.)), size(px(WINDOW.0), px(WINDOW.1)));
    open_with(seed, cx, |windows, cx| {
        windows.open(WindowKind::Console, Some(at), None, cx)
    })
}

pub(in crate::shell) fn open_live(
    seed: impl Into<Seed>,
    program: &'static str,
    cx: &mut TestAppContext,
) -> (Entities, WindowKey, Entity<WindowRoot>, VisualTestContext) {
    open_with(seed, cx, |windows, cx| {
        windows.open_live(program, cx);
        windows.console().expect("the live window is the console")
    })
}

fn open_with(
    seed: impl Into<Seed>,
    cx: &mut TestAppContext,
    open: impl FnOnce(&mut entities::Windows, &mut gpui_kit::Context<entities::Windows>) -> WindowKey,
) -> (Entities, WindowKey, Entity<WindowRoot>, VisualTestContext) {
    let Seed {
        roster,
        center,
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
        let app = entities::Entities::for_test(roster, center, cx);
        entities::Windows::forget_closed(&app.windows, cx);
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
    let key = app.windows.update(cx, open);
    if let Some(layout) = layout {
        let desk = entities::tests::desk_of(&app, key, cx);
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

pub(in crate::shell) fn entities(
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> entities::Entities {
    native.update(|_, cx| view.read(cx).app.clone())
}

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

pub(in crate::shell) fn open_now(
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> Option<Overlay> {
    native.update(|_, cx| *view.read(cx).overlays().read(cx).get())
}

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

pub(in crate::shell) fn select_view(
    app: &Entities,
    program: &'static str,
    native: &mut VisualTestContext,
) {
    app.windows
        .update(native, |windows, cx| windows.select_view(program, cx));
    native.run_until_parked();
}

pub(in crate::shell) fn open_help(app: &Entities, native: &mut VisualTestContext) {
    app.windows
        .update(native, |windows, cx| windows.help_asked(cx));
    native.run_until_parked();
}

pub(in crate::shell) fn toast(app: &Entities, said: &str, native: &mut VisualTestContext) {
    app.toast
        .update(native, |toast, cx| toast.show(said.to_owned(), cx));
    native.run_until_parked();
}

pub(in crate::shell) fn set_motion(app: &Entities, on: bool, native: &mut VisualTestContext) {
    app.prefs
        .update(native, |prefs, cx| prefs.set_motion(on, cx));
    native.run_until_parked();
}

pub(in crate::shell) fn pop_out(
    app: &Entities,
    console: WindowKey,
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> (WindowKey, AnyWindowHandle, Entity<WindowRoot>) {
    let index = native.update(|_, cx| view.read(cx).layout(cx).focused);
    app.windows.update(native, |windows, cx| {
        windows.pop_out(console, index, None, None, cx)
    });
    native.run_until_parked();
    popped(app, console, native)
}

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

pub(in crate::shell) fn frame(native: &mut VisualTestContext) {
    native.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    native.run_until_parked();
}

pub(in crate::shell) fn line(text: &str) -> wire::Node {
    wire::Node::RichText {
        id: Some(wire::ElementIdWire::Name("line".into())),
        style: crate::render::test_style(gpui_kit::div().h(px(20.)).style().clone()),
        text: text.into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    }
}

pub(in crate::shell) fn tree_renders(program: &str) -> u64 {
    crate::perf::snapshot(false)["views"][program]["renders"]
        .as_u64()
        .unwrap_or_else(|| panic!("{program} counts its renders"))
}
