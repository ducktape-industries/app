//! The layers' fixture: a console window drawn from a `Ducktape` state, as
//! `panes_tests::console` and `screens_tests::open` build theirs.
use super::super::{Desktop, WindowKey, WindowKind, entities, keys};
use super::WindowRoot;
use crate::Ducktape;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, px, size};

/// A console window over `state`, drawn once, its first frame's callbacks
/// delivered (the desk's size and its seed reach the model from there,
/// not from the draw).
pub(in crate::shell) fn open_console(
    state: Ducktape,
    cx: &mut TestAppContext,
) -> (
    Entity<Desktop>,
    WindowKey,
    Entity<WindowRoot>,
    VisualTestContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    // a state that has a console window (with a desk laid out for it) is drawn in it
    let key = state.console_win.unwrap_or_else(WindowKey::unique);
    let model = cx.new(|cx| {
        let entities = entities::Entities::for_test(&state, cx);
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
