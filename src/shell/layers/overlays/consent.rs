//! A view's op waiting on the person's yes (`runtime::consent`): the
//! front of the queue, in the dialog "Add a device…" asks its yes with.

use super::super::super::ink::{self, *};
use super::{OverlayLayer, scrim};
use crate::runtime::consent;
use crate::shell::entities::Overlay;
use gpui_kit::*;

impl OverlayLayer {
    /// The card for ask `ask`: what the view asks, the thing it names on a
    /// line of its own (a key's fingerprint, an agent's number), Approve
    /// and Cancel: the pieces of `approve`'s second screen, with these
    /// words. Approve lets the op go on to be signed; Cancel, or the dialog
    /// closed any other way (`Windows`), refuses it. The card is the ask's
    /// own (its id is in the dialog's): it shows no other ask's words, and
    /// a press that began on another card is not one on it.
    pub(super) fn consent(&self, ask: u64, cx: &App) -> AnyElement {
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let Some((_, words)) = consent::front().filter(|(front, _)| *front == ask) else {
            // answered or withdrawn already; the layer redraws without it
            return div().into_any_element();
        };
        let overlays = self.overlays.entity().clone();
        let answer = move |yes: bool, cx: &mut App| {
            consent::answer(ask, yes);
            // the card closes; another ask waiting opens as a new one
            let front = consent::front().map(|(id, _)| id);
            overlays.update(cx, |overlays, cx| overlays.follow_consent(front, cx));
        };
        let approve = {
            let answer = answer.clone();
            button(
                "consent-approve",
                "Approve",
                Kind::Primary,
                move |cx| answer(true, cx),
                false,
                &ink,
            )
        };
        let cancel = link_running(
            "consent-cancel",
            "Cancel",
            move |cx| answer(false, cx),
            false,
            &ink,
        );
        let shown = words.shown.map(|shown| {
            crate::a11y::whole(
                mono(400, 28.)
                    .id("consent-shown")
                    .role(Role::Label)
                    .aria_label(shown.clone())
                    .child(shown),
            )
        });
        scrim(
            format!("consent-{ask}"),
            Role::Dialog,
            "Confirm a change to your account",
            Overlay::Consent(ask),
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
                    .child(tag(
                        "consent-title",
                        "Confirm a change to your account",
                        &ink,
                    ))
                    .child(ink::note("consent-instruction", words.said, ink.muted))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .children(shown)
                            .child(div().flex().child(approve)),
                    )
                    .child(cancel)
                    .into_any_element()
            },
        )
    }
}
