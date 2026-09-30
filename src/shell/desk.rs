//! The desk frame of a window: the menu bar (`layers::Chrome`, console
//! only), the pane area (`layers::PaneLayer`), the dialog open over it, and
//! the footer. Here the keys go where an overlay opening or closing sends
//! them. Also the pieces every dialog is built from: `overlay` (scrim and
//! card), `dialog_fit`.

use super::layers::cached_unless_a11y;
use super::*;
use crate::Overlay;

/// The menu bar's height.
pub(super) const BAR: f32 = 36.;

impl DesktopWindow {
    /// The desk, in the console and in a pop-out alike: the bar (console
    /// only), the panes, the open overlay, the footer. Reached with an
    /// unlocked key, or by choosing to read without one.
    pub(super) fn desk_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let state = self.model.read(cx).state.facts();
        // tell the notification centre which window is in front and which
        // view is focused in it: that view's banners stay away (unless asked for)
        let layout = self.layout(cx);
        let focused = layout
            .panes
            .get(layout.focused)
            .map_or(layout::EMPTY, |pane| pane.module);
        state
            .center
            .lock()
            .set_front(self.key, window.is_window_active(), focused);
        let console = self.kind == crate::shell::WindowKind::Console;
        // the bar: a cached view of its own, its menus hanging from it
        let bar = self.chrome.clone().map(|chrome| {
            cached_unless_a11y(
                chrome.into(),
                StyleRefinement::default()
                    .w_full()
                    .h(px(BAR))
                    .flex_shrink_0(),
                window,
            )
        });
        let overlay = match state.overlay.filter(|_| console) {
            Some(Overlay::Spotlight) => Some(self.spotlight(&state, window, cx)),
            Some(Overlay::Approve) => Some(self.approve(&state, window, cx)),
            Some(Overlay::Settings) => Some(self.settings(&state, window, cx)),
            // the menus are the bar's
            Some(Overlay::Network | Overlay::Menu(_)) | None => None,
        };
        if state.overlay != Some(Overlay::Spotlight) {
            self.spotlight_focused = false;
        }
        // Whatever opens takes the keys (its backdrop holds `modal`, or a
        // menu's `menu`), unless something in it already did, as
        // Spotlight's field does; one giving way to the next hands them on.
        // The last to close gives them back to what had them before the
        // first opened, unless they left a menu and closed it.
        let open = state.overlay.filter(|_| console);
        if open != self.covered {
            match (self.covered, open) {
                (None, Some(_)) => self.refocus = window.focused(cx),
                (Some(_), None) => {
                    if let Some(handle) = self.refocus.take() {
                        window.defer(cx, move |window, cx| handle.focus(window, cx));
                    }
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
        div()
            .id("console")
            .size_full()
            .flex()
            .flex_col()
            .children(bar)
            // the panes: a layer of their own, measuring the desk they sit on
            .child(
                div()
                    .id("seat")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.panes.clone()),
            )
            .children(overlay)
            // over an open menu, which is deferred too, as it paints today
            .children(
                self.footer(cx)
                    .map(|footer| deferred(footer).with_priority(2)),
            )
            .into_any_element()
    }

    /// A dialog open over the desk, below the bar: a dimmed backdrop that
    /// closes it on a click, and on it the card the canvas dresses its
    /// dialogs in, `border: 1.5px solid ink` and a soft shadow. `dress`
    /// places and fills the card. Escape closes it through
    /// `keys::CloseOverlay` (bound under the `overlay` context). The
    /// backdrop holds the handle the keys enter it by (`desk_view`): it is
    /// modal, on `modal`, so Tab and Shift+Tab go round its controls, never
    /// out to the bar. (The bar's menus are `layers::Chrome`'s.)
    pub(super) fn overlay(
        &self,
        id: &'static str,
        role: gpui_kit::Role,
        name: &'static str,
        closes: crate::Overlay,
        ink: &super::ink::Ink,
        dress: impl FnOnce(gpui_kit::Stateful<gpui_kit::Div>) -> gpui_kit::AnyElement,
    ) -> gpui_kit::AnyElement {
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
                    model.dispatch(Message::CloseOverlay(closes), cx)
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

/// The least a dialog keeps from the window's edges, at any size.
const DIALOG_EDGE: f32 = 12.;

/// A dialog `tall` high in a window `high` high, hung `top` below the bar
/// when the window has room for it, higher (not under 12px) when it hasn't:
/// where its top goes, and the height it may take so its bottom stays 12px
/// inside the window.
pub(super) fn dialog_fit(high: f32, tall: f32, top: f32) -> (f32, f32) {
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
