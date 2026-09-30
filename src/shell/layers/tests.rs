//! The layers' fixture: a console window drawn from a `Seed` (the
//! reducer's state and the entities' values), as `panes_tests::console`
//! and `screens_tests::open` build theirs.
use super::super::entities::{self, AccountState, Chain, Overlay, Screen, Secret, SessionState};
use super::super::{Desktop, WindowKey, WindowKind, keys};
use super::WindowRoot;
use crate::Ducktape;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, px, size};

/// A screen state to draw: the reducer's state, and what `Session`,
/// `Chain`, `Account` and `Screen` hold.
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
        }
    }
}

/// A console window over `seed`, drawn once, its first frame's callbacks
/// delivered (the desk's size and its seed reach the model from there,
/// not from the draw).
pub(in crate::shell) fn open_console(
    seed: impl Into<Seed>,
    cx: &mut TestAppContext,
) -> (
    Entity<Desktop>,
    WindowKey,
    Entity<WindowRoot>,
    VisualTestContext,
) {
    let Seed {
        state,
        session,
        chain,
        account,
        screen,
        phrase,
        found,
    } = seed.into();
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    // a state that has a console window (with a desk laid out for it) is drawn in it
    let key = state.console_win.unwrap_or_else(WindowKey::unique);
    let model = cx.new(|cx| {
        let entities = entities::Entities::for_test(&state, cx);
        entities.session.update(cx, |it, cx| it.seed(session, cx));
        entities.chain.update(cx, |it, cx| {
            it.set(chain, cx);
        });
        entities
            .account
            .update(cx, |it, cx| it.seed(account, phrase, found, cx));
        entities.screen.update(cx, |it, cx| {
            it.set(screen, cx);
        });
        Desktop::new(state, crate::tray::init(cx).0, entities, cx)
    });
    let mut view = None;
    let handle = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
        let root =
            cx.new(|cx| WindowRoot::new(model.clone(), key, WindowKind::Console, window, cx));
        view = Some(root.clone());
        gpui_kit::component::Root::new(root, window, cx)
    });
    let view = view.unwrap();
    model.update(cx, |model, _| {
        model.windows.insert(key, handle.into());
        model.views.insert(key, view.downgrade());
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    (
        model,
        key,
        view,
        VisualTestContext::from_window(handle.into(), cx),
    )
}

/// The node's status poll answered `status`, as `Session`'s poll lands it.
pub(in crate::shell) fn polled(
    model: &Entity<Desktop>,
    status: crate::backend::NodeStatus,
    native: &mut VisualTestContext,
) {
    let session = model.read_with(native, |model, _| model.entities.session.clone());
    session.update(native, |session, cx| {
        session.status_answered(Ok(status), cx)
    });
    native.run_until_parked();
}

/// The console's entities, off `view`'s model.
pub(in crate::shell) fn entities(
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
) -> entities::Entities {
    native.update(|_, cx| view.read(cx).model.read(cx).entities.clone())
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
        let (overlays, model) = (view.read(cx).overlays(), view.read(cx).model.clone());
        super::overlays::run(&overlays, &model, Some(spot), cx);
    });
}
