//! The console's screens that still draw from the model: the launcher
//! (connect, key, phrase, recovery, account) before the desk, and the one
//! dialog open over it (Spotlight, Settings, Approve) on the desk. A
//! cached view over `Desktop` (the bridge's source, until s9 gives the
//! launcher its own layer and s8 the overlays theirs) and the slices that
//! say what it shows. Here the keys go where a dialog opening or closing
//! sends them, and the window's native text fields live. Also the pieces
//! every dialog is built from: `overlay` (scrim and card), `dialog_fit`.

use super::super::entities::{Observed, Overlays, Screen, Slice, Spotlight, WindowEntities};
use super::super::{Desktop, Stage, WindowKey, figure, settings, spin, text_field};
use super::BAR;
use crate::Overlay;
use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, Window};
use std::collections::HashMap;

/// The console's launcher screens, or the dialog open over its desk.
pub(in crate::shell) struct Screens {
    pub(in crate::shell) model: Entity<Desktop>,
    pub(in crate::shell) key: WindowKey,
    /// The model: what every screen here still draws from (s9 deletes
    /// this view with it), and the slices that say what it shows.
    _desktop: Observed<Desktop>,
    _overlays: Observed<Overlays>,
    _spotlight: Observed<Slice<Spotlight>>,
    screen: Observed<Slice<Screen>>,
    /// The native text fields drawn in this window, by their element id.
    pub(in crate::shell) inputs: HashMap<&'static str, text_field::NativeInput>,
    /// ⌘K's field took focus when it opened; it is not taken again while
    /// Spotlight stays open.
    pub(in crate::shell) spotlight_focused: bool,
    /// ⌘K's list: ↑↓ scroll the picked row into it.
    pub(in crate::shell) spotlight_rows: gpui_kit::ScrollHandle,
    /// Settings' page: the row holding the keys scrolls into view.
    pub(in crate::shell) settings_rows: settings::Page,
    /// The one Tab stop of each tab list and radio group drawn here, by
    /// the composite's id: its active item tracks it (`a11y::roving`).
    pub(in crate::shell) stops: HashMap<gpui_kit::SharedString, gpui_kit::FocusHandle>,
    /// The launcher's figure, written at its draw until s9's
    /// `LauncherLayer` writes it from its observers.
    pub(in crate::shell) launcher_spin: Entity<spin::Spin>,
    /// What was open over the desk when it was last drawn.
    pub(in crate::shell) covered: Option<Overlay>,
    /// What had the keys when something opened over the desk: they go back
    /// to it when it closes, so typing carries on where it was. Not when
    /// they left a menu, which closed it: they stay where they went.
    pub(in crate::shell) refocus: Option<gpui_kit::FocusHandle>,
    /// A dialog on a scrim (Spotlight, Settings, Approve): the keys go into
    /// it when it opens, and Tab and Shift+Tab stay in it.
    pub(in crate::shell) modal: gpui_kit::FocusHandle,
    /// A menu hanging from the bar (the window's handle, `WindowRoot`): the
    /// keys go into it when it opens.
    menu: gpui_kit::FocusHandle,
}

impl Screens {
    pub(in crate::shell) fn new(
        model: Entity<Desktop>,
        key: WindowKey,
        own: &WindowEntities,
        menu: gpui_kit::FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let screen = model.read(cx).entities.screen.clone();
        let launcher_spin = cx
            .new(|cx| spin::Spin::new(figure::Figure::Roll, false, gpui_kit::Hsla::default(), cx));
        Self {
            _desktop: Observed::new(&model, cx),
            _overlays: Observed::new(&own.overlays, cx),
            _spotlight: Observed::new(&own.spotlight, cx),
            screen: Observed::new(&screen, cx),
            model,
            key,
            inputs: HashMap::new(),
            spotlight_focused: false,
            spotlight_rows: Default::default(),
            settings_rows: Default::default(),
            stops: HashMap::new(),
            launcher_spin,
            covered: None,
            refocus: None,
            modal: cx.focus_handle(),
            menu,
        }
    }

    /// The one Tab stop of the tab list or radio group `id`
    /// (`a11y::roving`), made the first time it is drawn.
    pub(in crate::shell) fn stop(&mut self, id: &str, cx: &gpui_kit::App) -> gpui_kit::FocusHandle {
        self.stops
            .entry(id.to_owned().into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// The keys left the open menu (Tab past its end, a click elsewhere;
    /// `layers::Chrome` through `WindowRoot`): the keys stay where they
    /// went (owner, 2026-09-28) rather than going back to what had them
    /// before it opened.
    pub(in crate::shell) fn menu_left_by_keys(&mut self) {
        self.refocus = None;
    }

    /// The dialog open over the desk, if one is: Spotlight, Approve or
    /// Settings (the menus are the bar's). Whatever opens takes the keys
    /// (its backdrop holds `modal`, or a menu's `menu`), unless something
    /// in it already did, as Spotlight's field does; one giving way to the
    /// next hands them on. The last to close gives them back to what had
    /// them before the first opened, unless they left a menu and closed it.
    fn dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let state = self.model.read(cx).state.facts();
        let open = state.overlay;
        let dialog = match open {
            Some(Overlay::Spotlight) => Some(self.spotlight(&state, window, cx)),
            Some(Overlay::Approve) => Some(self.approve(&state, window, cx)),
            Some(Overlay::Settings) => Some(self.settings(&state, window, cx)),
            Some(Overlay::Network | Overlay::Menu(_)) | None => None,
        };
        if open != Some(Overlay::Spotlight) {
            self.spotlight_focused = false;
        }
        if open != self.covered {
            match (self.covered, open) {
                (None, Some(_)) => self.refocus = window.focused(cx),
                (Some(_), None) => {
                    // read when it runs, not now: this view draws at prepaint,
                    // after the pane layer's draw asked for its own handoff,
                    // and a pane that came to the front meanwhile keeps the
                    // keys (`PaneLayer::keys_move` clears `refocus`)
                    let this = cx.weak_entity();
                    window.defer(cx, move |window, cx| {
                        let refocus = this.update(cx, |this, _| this.refocus.take());
                        if let Ok(Some(handle)) = refocus {
                            handle.focus(window, cx);
                        }
                    });
                }
                _ => {}
            }
            if let Some(open) = open {
                let modal = match open {
                    Overlay::Network | Overlay::Menu(_) => self.menu.clone(),
                    _ => self.modal.clone(),
                };
                window.defer(cx, move |window, cx| {
                    if !modal.contains_focused(window, cx) {
                        modal.focus(window, cx);
                        window.focus_next(cx);
                    }
                });
            }
        }
        self.covered = open;
        dialog
    }

    /// A dialog open over the desk, below the bar: a dimmed backdrop that
    /// closes it on a click, and on it the card the canvas dresses its
    /// dialogs in, `border: 1.5px solid ink` and a soft shadow. `dress`
    /// places and fills the card. Escape closes it through
    /// `keys::CloseOverlay` (bound under the `overlay` context). The
    /// backdrop holds the handle the keys enter it by (`dialog`): it is
    /// modal, on `modal`, so Tab and Shift+Tab go round its controls, never
    /// out to the bar. (The bar's menus are `layers::Chrome`'s.)
    pub(in crate::shell) fn overlay(
        &self,
        id: &'static str,
        role: gpui_kit::Role,
        name: &'static str,
        closes: Overlay,
        ink: &super::super::ink::Ink,
        dress: impl FnOnce(gpui_kit::Stateful<gpui_kit::Div>) -> gpui_kit::AnyElement,
    ) -> gpui_kit::AnyElement {
        use crate::a11y::Control as _;
        use gpui_kit::component::FocusTrapElement as _;
        use gpui_kit::*;
        let model = self.model.clone();
        let backdrop = div()
            .id(SharedString::from(format!("{id}-backdrop")))
            .absolute()
            .top(px(BAR))
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            // a dialog keeps its margin from the window's sides, as
            // `dialog_fit` keeps it from the bottom
            .bg(ink.bg.opacity(0.6))
            .flex()
            .justify_center()
            .px(px(DIALOG_EDGE))
            .on_click(move |_, _, cx| {
                model.update(cx, |model, cx| {
                    model.dispatch(crate::AppMessage::CloseOverlay(closes), cx)
                });
            });
        let card = div()
            .id(id)
            .control(role, name)
            // a press inside the card stays inside: occluded, the backdrop
            // is not under the pointer and its click never fires. No
            // `on_click` here: that would offer a press on the dialog.
            .occlude()
            .flex()
            .flex_col()
            .bg(ink.bg)
            .text_color(ink.ink)
            .border(px(1.5))
            .border_color(ink.ink)
            .shadow_lg();
        // modal to assistive technology as to the keyboard: what is
        // behind the scrim is not reachable
        backdrop
            .child(dress(crate::a11y::modal(card)))
            .focus_trap(SharedString::from(format!("{id}-backdrop")), &self.modal)
            .into_any_element()
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
            return div().absolute().inset_0().children(self.dialog(window, cx));
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

/// The least a dialog keeps from the window's edges, at any size.
const DIALOG_EDGE: f32 = 12.;

/// A dialog `tall` high in a window `high` high, hung `top` below the bar
/// when the window has room for it, higher (not under 12px) when it hasn't:
/// where its top goes, and the height it may take so its bottom stays 12px
/// inside the window.
pub(in crate::shell) fn dialog_fit(high: f32, tall: f32, top: f32) -> (f32, f32) {
    let room = high - BAR;
    let top = (room - tall - DIALOG_EDGE).clamp(DIALOG_EDGE, top);
    (top, (room - top - DIALOG_EDGE).max(0.))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_rises_then_shrinks_to_stay_inside_a_short_window() {
        // room for it: where the design hangs it, whole
        assert_eq!(dialog_fit(800., 680., 74.), (72., 680.));
        assert_eq!(dialog_fit(1000., 680., 74.), (74., 878.));
        // the smallest window: as high as it goes, and no taller than what is left
        let (top, tall) = dialog_fit(480., 680., 74.);
        assert_eq!(top, 12.);
        assert_eq!(BAR + top + tall + 12., 480.);
    }
}
