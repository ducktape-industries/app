//! "Add a device…": approving another device onto the account.

use super::super::super::entities::Account;
use super::super::super::ink::{self, *};
use super::super::fields::TextField;
use super::{OverlayLayer, scrim};
use crate::shell::entities::Overlay;
use gpui_kit::*;

impl OverlayLayer {
    /// "Add a device…": the code a new device shows, then its fingerprint
    /// to compare, then this device's yes — consent it signs for the
    /// account and hands over the relay. Dressed as the canvas's dialogs:
    /// `border: 1.5px solid ink`, a soft shadow, on a scrim below the bar.
    /// What it asks is one `Account` call, the code from its field.
    pub(super) fn approve(&self, window: &mut Window, cx: &App) -> AnyElement {
        let entity = self.account.entity().clone();
        let account = self.account.read(cx).get();
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let failed = (!account.error.is_empty()).then(|| account.error.clone());
        let error = failed
            .clone()
            .map(|error| alert("approve-error", error, &ink));
        let (said, fields) = match &account.approve {
            None => {
                let code = self.approve_code.input(
                    "approve-code",
                    TextField {
                        label: "Code".into(),
                        private: false,
                        error: failed,
                        size: 15.,
                    },
                    cx,
                );
                let focused = self.approve_code.focused(window, cx);
                (
                    "On the new device, choose \"Add this device from another device\". Type the code it shows.",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(field(
                            "approve-code-label",
                            "Code",
                            field_box(code, focused, ink.field, 44., &ink)
                                .font_family(super::super::super::theme::FAMILY_MONO)
                                .into_any_element(),
                            error,
                            &ink,
                        ))
                        .child(div().flex().child(button(
                            "approve-find",
                            "Next",
                            Kind::Primary,
                            {
                                let code = self.approve_code.state.clone();
                                move |cx| {
                                    let code = code.read(cx).value().to_string();
                                    entity.update(cx, |account, cx| account.approve_find(code, cx))
                                }
                            },
                            account.busy,
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
                    .child(div().flex().child(button(
                        "approve-confirm",
                        "Approve",
                        Kind::Primary,
                        move |cx| entity.update(cx, Account::approve_confirm),
                        account.busy,
                        &ink,
                    ))),
            ),
        };
        let cancel = {
            let overlays = self.overlays.entity().clone();
            link_running(
                "approve-cancel",
                "Cancel",
                move |cx| overlays.update(cx, |overlays, cx| overlays.close(Overlay::Approve, cx)),
                false,
                &ink,
            )
        };
        scrim(
            "approve",
            Role::Dialog,
            "Add a device",
            Overlay::Approve,
            self.overlays.entity(),
            &self.modal,
            &ink,
            |card| {
                card.mt(px(84.))
                    .w(px(440.))
                    .max_w_full()
                    .self_start()
                    .gap(px(18.))
                    .p(px(24.))
                    .child(tag("approve-title", "Add a device", &ink))
                    .child(ink::note("approve-instruction", said, ink.muted))
                    .child(fields)
                    .child(cancel)
                    .into_any_element()
            },
        )
    }
}
