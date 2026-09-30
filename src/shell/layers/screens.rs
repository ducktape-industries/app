//! The console's screens that still draw from the model: the launcher
//! (connect, key, phrase, recovery, account) before the desk, and "Add a
//! device…" while it is open over the desk (s9 gives each a layer of its
//! own). A cached view over `Desktop` (the bridge's source) and the slices
//! that say what it shows. Here the launcher's native text fields live.

use super::super::entities::{Observed, Overlay, Overlays, Screen, Slice, WindowEntities};
use super::super::{Desktop, Stage, WindowKey, figure, spin};
use super::fields;
use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, Window};
use std::collections::HashMap;

/// The console's launcher screens, or "Add a device…" over its desk.
pub(in crate::shell) struct Screens {
    pub(in crate::shell) model: Entity<Desktop>,
    pub(in crate::shell) key: WindowKey,
    /// The model: what every screen here still draws from (s9 deletes
    /// this view with it), and the slices that say what it shows.
    _desktop: Observed<Desktop>,
    pub(in crate::shell) overlays: Observed<Overlays>,
    screen: Observed<Slice<Screen>>,
    /// The native text fields drawn in this window, by their element id.
    pub(in crate::shell) inputs: HashMap<&'static str, fields::NativeInput>,
    /// The launcher's figure, written at its draw until s9's
    /// `LauncherLayer` writes it from its observers.
    pub(in crate::shell) launcher_spin: Entity<spin::Spin>,
    /// The handle the keys enter a dialog on a scrim by: the overlay
    /// layer's, which hands them over (`OverlayLayer::moved`).
    pub(in crate::shell) modal: gpui_kit::FocusHandle,
}

impl Screens {
    pub(in crate::shell) fn new(
        model: Entity<Desktop>,
        key: WindowKey,
        own: &WindowEntities,
        modal: gpui_kit::FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let screen = model.read(cx).entities.screen.clone();
        let launcher_spin = cx
            .new(|cx| spin::Spin::new(figure::Figure::Roll, false, gpui_kit::Hsla::default(), cx));
        Self {
            _desktop: Observed::new(&model, cx),
            overlays: Observed::new(&own.overlays, cx),
            screen: Observed::new(&screen, cx),
            model,
            key,
            inputs: HashMap::new(),
            launcher_spin,
            modal,
        }
    }
}

impl Render for Screens {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::{ParentElement as _, Styled as _, div};
        let launcher = *self.screen.read(cx).get() != Screen::Desk;
        let counted = match launcher {
            true => "renders.launcher",
            false => "renders.overlays",
        };
        crate::perf::count(crate::perf::Key::Window(self.key), counted, 1);
        if !launcher {
            let state = self.model.read(cx).state.facts();
            let approve = (*self.overlays.read(cx).get() == Some(Overlay::Approve))
                .then(|| self.approve(&state, window, cx));
            // `size_full` too: cached, this view is laid out as a root of
            // its own, where a block's height is its content's (taffy), and
            // the dialog's backdrop is absolute. Sized by its insets alone
            // the root would be 0px high and the scrim culled
            return div().absolute().inset_0().size_full().children(approve);
        }
        let state = self.model.read(cx).state.facts();
        let screen = match self.model.read(cx).state.stage {
            Stage::Connect => self.connect(window, cx),
            Stage::Phrase(_) => self.phrase(&state, window, cx),
            Stage::Unlock(_) => self.unlock(&state, window, cx),
            Stage::Recover(_) => self.recover(&state, window, cx),
            Stage::Account(_) => self.account_step(&state, window, cx),
            Stage::Desk => unreachable!("the desk is the root's"),
        };
        div().size_full().child(screen)
    }
}
