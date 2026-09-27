//! The desk frame of a window: the menu bar (console only), the pane area,
//! whatever is open over it, and the footer. Here the bar is measured for
//! folding, the desk's size is reported to the model, and the keys go back
//! where they were when an overlay closes. Also the pieces every overlay is
//! built from: `overlay` (backdrop and card), `hanging` (a menu under its
//! bar button), `menu_row`, `dialog_fit`.

use super::*;
use crate::{Overlay, Popover};

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
        let rail = crate::runtime::rail();
        if rail.iter().any(|row| row.note == Some("Loading")) {
            window.request_animation_frame();
        }
        // Folding. The tabs show their full labels until they overflow their
        // strip; then every tab folds to its icon or initial, so none is cut.
        // `bar_needs` is the narrowest window the full labels are known to
        // need: the width the bar was last drawn unfolded at, plus how far
        // the strip overflowed there (`self.rail.max_offset()`). Layout
        // reports that overflow one frame late, so a bar drawn unfolded at a
        // new width asks for one more frame to be measured in. `bar_needs`
        // only grows while the bar shows the same words. New words (the
        // network, the tab list, the sign-in state: `bar_made` hashes them)
        // reset it to measure again, and the overflow the old words left is
        // skipped on that frame. Badge counts stay out of the hash: they
        // tick while folded, and a re-measure draws the bar whole for a frame.
        let made_of = {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            state.network.hash(&mut hasher);
            for row in &rail {
                (row.module, &row.label, row.note, row.empty).hash(&mut hasher);
            }
            (state.signer_key.is_empty(), &state.account).hash(&mut hasher);
            hasher.finish()
        };
        let remade = self.bar_made != made_of;
        if remade {
            self.bar_made = made_of;
            self.bar_needs = 0.;
        }
        let width = f32::from(window.viewport_size().width);
        // last frame's overflow: the old words', on the frame that remade the bar
        let over = f32::from(self.rail.max_offset().x);
        if let Some(drawn) = self.bar_drawn
            && !remade
            && over > 0.
            && drawn + over > self.bar_needs
        {
            self.bar_needs = drawn + over;
        }
        let narrow = width < self.bar_needs;
        let drawn = (!narrow).then_some(width);
        // drawn whole at a new width or of new words: the next frame measures
        // it, so ask for one (nothing else may draw one soon)
        if drawn.is_some() && (remade || drawn != self.bar_drawn) {
            window.request_animation_frame();
        }
        self.bar_drawn = drawn;
        // report the desk's size; on the console's first draw also `seed`, a
        // program a link opened before the desk existed (else it starts empty)
        let desk = self.desk(window);
        let layout = self.layout(cx);
        let seed = (self.kind == crate::shell::WindowKind::Console && !layout.initialized)
            .then_some(state.active)
            .flatten();
        if layout.desk != Some(desk) || seed.is_some() {
            let window = self.key;
            self.model.update(cx, |model, cx| {
                model.dispatch(Message::DeskShown { window, desk, seed }, cx)
            });
        }
        // tell the notification centre which window is in front and which
        // view is focused in it: that view's banners stay away (unless asked for)
        let layout = self.layout(cx);
        let focused = layout
            .panes
            .get(layout.focused)
            .map_or(layout::EMPTY, |pane| pane.module);
        crate::runtime::notify::center().set_front(self.key, window.is_window_active(), focused);
        let console = self.kind == crate::shell::WindowKind::Console;
        let bar = console.then(|| self.menubar(&state, &rail, narrow, window, cx));
        let seat = self.pane_stage(window, cx);
        let overlay = match state.overlay.filter(|_| console) {
            None => None,
            Some(Overlay::Spotlight) => Some(self.spotlight(&state, window, cx)),
            Some(Overlay::Approve) => Some(self.approve(&state, window, cx)),
            Some(Overlay::Settings) => Some(self.settings(&state, window)),
            Some(Overlay::Network) => Some(self.network_menu(&state, narrow, window)),
            Some(Overlay::Menu(Popover::Node)) => Some(self.node_menu(&state, window, cx)),
            Some(Overlay::Menu(Popover::Account)) => Some(self.account_menu(&state, window, cx)),
            Some(Overlay::Menu(Popover::Notifications)) => {
                Some(self.notifications(&state, window, cx))
            }
        };
        if state.overlay != Some(Overlay::Spotlight) {
            self.spotlight_focused = false;
        }
        match (self.covered, state.overlay.filter(|_| console)) {
            (None, Some(open)) => {
                let before = window.focused(cx);
                self.refocus = before.clone();
                // a dialog on a scrim takes the keys, unless its own field
                // already did, and keeps Tab (`overlay`)
                if matches!(
                    open,
                    Overlay::Spotlight | Overlay::Approve | Overlay::Settings
                ) {
                    let modal = self.modal.clone();
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx) == before {
                            modal.focus(window, cx);
                            window.focus_next(cx);
                        }
                    });
                }
            }
            (Some(_), None) => {
                if let Some(handle) = self.refocus.take() {
                    window.defer(cx, move |window, cx| handle.focus(window, cx));
                }
            }
            _ => {}
        }
        self.covered = state.overlay.filter(|_| console);
        div()
            .id("console")
            .size_full()
            .flex()
            .flex_col()
            .children(bar)
            .child(div().id("seat").flex_1().min_h_0().w_full().child(seat))
            .children(overlay)
            .children(self.footer(cx))
            .into_any_element()
    }

    /// Something open over the desk, below the bar (its items stay live,
    /// and one menu gives way to the next in one click): a backdrop that
    /// closes it on a click, dimmed when `scrim`, and on it the card the
    /// canvas dresses its menus and dialogs in, `border: 1.5px solid ink`
    /// and a soft shadow. `dress` places and fills the card. Escape closes
    /// it through `keys::CloseOverlay` (bound under the `overlay` context).
    /// Dimmed, it is modal: Tab and Shift+Tab go round its controls, never
    /// out to the bar.
    #[allow(clippy::too_many_arguments, reason = "one frame, five overlays")]
    pub(super) fn overlay(
        &self,
        id: &'static str,
        role: gpui_kit::Role,
        name: &'static str,
        closes: crate::Overlay,
        scrim: bool,
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
            .when(scrim, |backdrop| {
                backdrop
                    .bg(ink.bg.opacity(0.6))
                    .flex()
                    .justify_center()
                    .px(px(DIALOG_EDGE))
            })
            .on_click(move |_, _, cx| {
                model.update(cx, |model, cx| {
                    model.dispatch(Message::CloseOverlay(closes), cx)
                });
            });
        let card = div()
            .id(id)
            .control(role, name)
            .occlude()
            .flex()
            .flex_col()
            .bg(ink.bg)
            .text_color(ink.ink)
            .border(px(1.5))
            .border_color(ink.ink)
            .shadow_lg()
            // a press inside the card stays inside: the backdrop's click
            // (`on_click` needs the down it never sees) does not close it.
            // Not an `on_click` here: that would offer a press on the dialog.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match scrim {
            // modal to assistive technology as to the keyboard: what is
            // behind the scrim is not reachable
            true => backdrop
                .child(dress(crate::a11y::modal(card)))
                .focus_trap(SharedString::from(format!("{id}-backdrop")), &self.modal)
                .into_any_element(),
            false => backdrop.child(dress(card)).into_any_element(),
        }
    }

    /// A menu hanging below the bar (the canvas's menus: `top: 40px`), its
    /// right edge under its button's.
    pub(super) fn hanging(
        &self,
        which: crate::Popover,
        width: f32,
        body: impl IntoElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        let (id, name) = match which {
            crate::Popover::Node => ("node-status", "Node status"),
            crate::Popover::Account => ("account-menu", "Account"),
            crate::Popover::Notifications => ("notifications", "Notifications"),
        };
        let overlay = Overlay::Menu(which);
        let at = self.under_button(overlay, Anchor::TopRight, window);
        // a short window scrolls the menu rather than cutting it off
        // (the card's 1.5px border above and below it)
        let room = f32::from(window.viewport_size().height) - BAR - 8. - 3.;
        let body = div()
            .id(SharedString::from(format!("{id}-body")))
            .max_h(px(room.max(0.)))
            .overflow_y_scroll()
            .child(body);
        self.overlay(id, Role::Dialog, name, overlay, false, &ink, |card| {
            at.child(card.w(px(width)).child(body)).into_any_element()
        })
    }

    /// Where `overlay`'s menu hangs: `anchor` (a top corner) at the same
    /// corner of its bar button, 4px below the bar, shifted back inside the
    /// window when it would run off it. Before the button was ever painted,
    /// the window's edge on that side.
    pub(super) fn under_button(
        &self,
        overlay: Overlay,
        anchor: gpui_kit::Anchor,
        window: &Window,
    ) -> gpui_kit::Anchored {
        use gpui_kit::*;
        let left = anchor == Anchor::TopLeft;
        let x = match self.bar_buttons.get(&overlay) {
            Some(button) if left => button.left(),
            Some(button) => button.right(),
            None if left => px(8.),
            None => window.viewport_size().width - px(8.),
        };
        anchored()
            .position(point(x, px(BAR + 4.)))
            .anchor(anchor)
            .snap_to_window_with_margin(px(8.))
    }

    /// A menu item: `height: 36px; padding: 0 16px; font: 400 14px`.
    pub(super) fn menu_row(
        &self,
        id: &'static str,
        label: &'static str,
        run: impl Fn(&mut gpui_kit::App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use super::ink::{Ink, sans};
        use gpui_kit::*;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let surface = ink.surface;
        crate::a11y::keyboard(
            sans(400, 14.)
                .id(id)
                .control(Role::MenuItem, label)
                .h(px(36.))
                .px(px(16.))
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(move |style| style.bg(surface))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    run(cx)
                })
                .child(label),
        )
    }

    pub(super) fn dispatching(
        &self,
        message: fn() -> Message,
    ) -> impl Fn(&mut gpui_kit::App) + 'static {
        let model = self.model.clone();
        move |cx| model.update(cx, |model, cx| model.dispatch(message(), cx))
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
