//! "Add a device…": approving another device onto the account.

use super::*;
use screens::Facts;

impl DesktopWindow {
    /// "Add a device…": the code a new device shows, then its fingerprint
    /// to compare, then this device's yes — consent it signs for the
    /// account and hands over the relay. Dressed as the canvas's dialogs:
    /// `border: 1.5px solid ink`, a soft shadow, on a scrim below the bar.
    pub(super) fn approve(
        &mut self,
        state: &Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::{self, *};
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let error = (!state.unlock_error.is_empty())
            .then(|| self.alert("approve-error", state.unlock_error.clone(), &ink));
        let (said, fields) = match &state.approve_fingerprint {
            None => {
                let code = self.input(
                    "approve-code",
                    "XXXX-XXXX",
                    false,
                    |state| &state.sign_in.approve_code,
                    Message::ApproveCodeTyped,
                    || Message::ApproveFind,
                    Some("Code".into()),
                    false,
                    15.,
                    window,
                    cx,
                );
                (
                    "On the new device, choose \"Add this device from another device\". Type the code it shows.",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(
                            self.field(
                                "Code",
                                field_box(code, ink.strong, 44., &ink)
                                    .font_family(super::theme::FAMILY_MONO)
                                    .into_any_element(),
                                error,
                                &ink,
                            ),
                        )
                        .child(div().flex().child(self.button(
                            "approve-find",
                            "Next",
                            Kind::Primary,
                            || Message::ApproveFind,
                            state.unlock_busy,
                            &ink,
                        ))),
                )
            }
            Some(fingerprint) => (
                "Approve only if the new device shows these same characters. It can then write as your account.",
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(crate::a11y::whole(
                        mono(400, 28.)
                            .id("approve-fingerprint")
                            .role(Role::Label)
                            .aria_label(fingerprint.clone())
                            .child(fingerprint.clone()),
                    ))
                    .children(error)
                    .child(div().flex().child(self.button(
                        "approve-confirm",
                        "Approve",
                        Kind::Primary,
                        || Message::ApproveConfirm,
                        state.unlock_busy,
                        &ink,
                    ))),
            ),
        };
        let cancel = self.link(
            "approve-cancel",
            "Cancel",
            || Message::CloseOverlay(crate::Overlay::Approve),
            false,
            &ink,
        );
        self.overlay(
            "approve",
            Role::Dialog,
            "Add a device",
            crate::Overlay::Approve,
            true,
            &ink,
            |card| {
                card.mt(px(84.))
                    .w(px(440.))
                    .max_w_full()
                    .self_start()
                    .gap(px(18.))
                    .p(px(24.))
                    .child(tag("Add a device", &ink))
                    .child(ink::note(said, ink.muted))
                    .child(fields)
                    .child(cancel)
                    .into_any_element()
            },
        )
    }
}
