//! A view's op waiting on the person's yes (`runtime::consent`): the
//! front of the queue, in the dialog "Add a device…" asks its yes with.

use super::super::super::ink::*;
use super::{OverlayLayer, dialog_fit, scrim};
use crate::runtime::consent;
use crate::shell::entities::Overlay;
use gpui_kit::*;

/// How long Approve takes no press after the card opens: a second press
/// the view asked for ("press twice") lands on the card before anyone
/// read it.
pub(in crate::shell) const SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// The card's usual height, a few lines of description in it: where it
/// hangs in a short window (`dialog_fit`).
const TALL: f32 = 360.;

impl OverlayLayer {
    /// The card for ask `ask`: its title (a change to the person's account
    /// or to the network), what the view asks, the thing it names on a
    /// line of its own (a key's fingerprint, an agent's number) or the
    /// target program's description of the op in a box, Approve and
    /// Cancel: the pieces of `approve`'s second screen, with these
    /// words. Approve lets the op go on to be signed; Cancel, or the dialog
    /// closed any other way (`Windows`), refuses it. The card is the ask's
    /// own (its id is in the dialog's): it shows no other ask's words, and
    /// a press that began on another card is not one on it, nor one in the
    /// first [`SETTLE`] after it opened. In a short window the description
    /// scrolls and Approve stays on screen.
    pub(super) fn consent(&self, ask: u64, window: &Window, cx: &App) -> AnyElement {
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
        // Approve's press, once the card's first moment has passed
        let approved = {
            let answer = answer.clone();
            let overlays = self.overlays.entity().clone();
            move |cx: &mut App| {
                let opened = overlays.read(cx).opened();
                if cx
                    .background_executor()
                    .now()
                    .saturating_duration_since(opened)
                    >= SETTLE
                {
                    answer(true, cx);
                }
            }
        };
        if ducktape_host::contract::os().facts().touch_first {
            return self.consent_sheet(ask, words, approved, answer, window, cx);
        }
        let approve = button(
            "consent-approve",
            "Approve",
            Kind::Primary,
            approved,
            false,
            &ink,
        );
        let cancel = link_running(
            "consent-cancel",
            "Cancel",
            move |cx| answer(false, cx),
            false,
            &ink,
        );
        // the sentence, the asking program's id in mono: one Label, its
        // words its value, as `ink::note`'s text is
        let said = SharedString::from(words.said);
        let id = words.id;
        let instruction = sans(400, 13.)
            .id("consent-instruction")
            .role(Role::Label)
            .aria_value(said.clone())
            .line_height(px(13. * 1.55))
            .text_color(ink.muted)
            .child(
                StyledText::new(said)
                    .with_highlights(id.clone().map(|id| (id, HighlightStyle::default())))
                    .with_font_family_overrides(
                        id.map(|id| (id, SharedString::from(crate::shell::theme::FAMILY_MONO))),
                    ),
            );
        let described = words.described;
        let shown = words.shown.map(|shown| {
            let label = mono(400, if described { 12. } else { 28. })
                .id("consent-shown")
                .role(Role::Label)
                .aria_label(shown.clone());
            // a description: lines in a box, as the design's `.shown`
            let label = match described {
                true => label
                    .line_height(px(12. * 1.5))
                    .py(px(8.))
                    .px(px(10.))
                    .bg(ink.surface)
                    .border_1()
                    .border_color(ink.line),
                false => label,
            };
            // the description scrolls before the card outgrows the window
            div()
                .id("consent-shown-scroll")
                .min_h_0()
                .overflow_y_scroll()
                .child(crate::a11y::whole(label.child(shown)))
        });
        let title = words.title;
        let (top, tall) = dialog_fit(
            f32::from(window.viewport_size().height),
            self.inset(window, cx),
            TALL,
            84.,
        );
        scrim(
            format!("consent-{ask}"),
            Role::Dialog,
            title,
            Overlay::Consent(ask),
            self.overlays.entity(),
            &self.modal,
            self.inset(window, cx),
            &ink,
            |card| {
                card.mt(px(top))
                    .max_h(px(tall))
                    .w(px(440.))
                    .max_w_full()
                    .self_start()
                    .gap(px(18.))
                    .p(px(24.))
                    .child(tag("consent-title", title, &ink))
                    .child(instruction)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .gap(px(16.))
                            .children(shown)
                            .child(div().flex().child(approve)),
                    )
                    .child(cancel)
                    .into_any_element()
            },
        )
    }

    /// The card under touch: the phone's confirm sheet (mobile
    /// `src/kit/Confirm.tsx`, board 96), docked to the bottom over the
    /// Expo scrim with the Expo sheet's handle: the ask's title as an
    /// eyebrow, "<verb>: <target>", the op's facts, what it takes away,
    /// Cancel and the verb (in danger for one that takes something away),
    /// and the caption on who signs; every size the phone's own. A press on
    /// the scrim closes it, as Cancel does. Approve keeps the desktop's
    /// first moment ([`SETTLE`]).
    fn consent_sheet(
        &self,
        ask: u64,
        asked: consent::Words,
        approved: impl Fn(&mut App) + 'static,
        answer: impl Fn(bool, &mut App) + 'static,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        use crate::a11y::Control as _;
        use gpui_kit::component::FocusTrapElement as _;
        use gpui_kit::prelude::FluentBuilder as _;
        let dark = self.prefs.read(cx).get().dark();
        let palette = design::palette(dark);
        let color = ducktape_host::theme::hsla;
        let card = asked.card;
        let face = |size: f32, line: f32, weight: u16, mono: bool| {
            div()
                .font_family(match mono {
                    true => crate::shell::theme::FAMILY_MONO,
                    false => crate::shell::theme::FAMILY_UI,
                })
                .font_weight(FontWeight(f32::from(weight)))
                .text_size(px(size))
                .line_height(px(line))
        };
        // the kit's `Text` roles: title 17/21, small 14/19, body 15/21,
        // secondary 13/18, caption 12/16, eyebrow mono 11/14 uppercase
        let head = div()
            .flex()
            .flex_col()
            .gap(px(XS))
            .px(px(INSET))
            .pb(px(MD))
            .child(
                face(11., 14., 400, true)
                    .text_color(color(palette.muted))
                    .child(words("consent-title", asked.title.to_uppercase())),
            )
            .child(
                face(17., 21., 500, false)
                    .text_color(color(palette.foreground))
                    .child(words("consent-card-title", card.title)),
            );
        // the kit's `KeyValue`: a 96 wide label, the value beside it
        let facts = card.facts.into_iter().enumerate().map(|(at, fact)| {
            div()
                .min_h(px(44.))
                .flex()
                .items_start()
                .gap(px(LG))
                .py(px(MD))
                .px(px(BLOCK))
                .border_b_1()
                .border_color(color(palette.border))
                .child(
                    face(14., 19., 400, false)
                        .w(px(96.))
                        .flex_none()
                        .text_color(color(palette.muted))
                        .child(words(format!("consent-fact-{at}"), fact.label)),
                )
                .child(
                    face(14., 19., 400, fact.mono)
                        .flex_1()
                        .min_w_0()
                        .text_color(color(palette.foreground))
                        .child(words(format!("consent-fact-{at}-value"), fact.value)),
                )
        });
        let warning = face(13., 18., 400, false)
            .px(px(INSET))
            .pt(px(MD))
            .text_color(color(palette.warning))
            .child(words("consent-warning", card.warning));
        // the kit's `Button`: Cancel quiet at the primary's 48, the verb
        // twice its width, 48 as a primary and 44 in danger, as the kit's
        // own heights put them (`size.primary`, `size.control`)
        let pressable = |id: &'static str,
                         label: &'static str,
                         ground: Option<design::Color>,
                         edge: Option<design::Color>,
                         tone: design::Color,
                         weight: u16,
                         tall: f32,
                         run: Box<dyn Fn(&mut App)>| {
            let filled = ground.is_some();
            let button = face(15., 21., weight, false)
                .id(id)
                .control(Role::Button, label)
                .h(px(tall))
                .flex_basis(px(0.))
                .min_w_0()
                .px(px(BLOCK))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(edge.map_or(gpui_kit::transparent_black(), color))
                .when_some(ground, |button, ground| button.bg(color(ground)))
                .text_color(color(tone))
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    run(cx)
                })
                .child(div().truncate().child(label));
            let ring = match filled {
                true => color(palette.background),
                false => color(palette.foreground),
            };
            crate::a11y::keyboard(button, ring)
        };
        let mut cancel = pressable(
            "consent-cancel",
            "Cancel",
            None,
            None,
            palette.muted,
            400,
            48.,
            Box::new(move |cx| answer(false, cx)),
        );
        cancel.style().flex_grow = Some(1.);
        let (ground, edge, tone, tall) = match card.danger {
            true => (None, palette.danger, palette.danger, 44.),
            false => (
                Some(palette.primary),
                palette.primary,
                palette.primary_foreground,
                48.,
            ),
        };
        let mut verb = pressable(
            "consent-approve",
            card.verb,
            ground,
            Some(edge),
            tone,
            500,
            tall,
            Box::new(approved),
        );
        verb.style().flex_grow = Some(2.);
        let buttons = div()
            .flex()
            .items_start()
            .gap(px(SM))
            .px(px(INSET))
            .pt(px(BLOCK))
            .child(cancel)
            .child(verb);
        let caption = face(12., 16., 400, false)
            .px(px(INSET))
            .pt(px(MD))
            .text_color(color(palette.muted))
            .child(words(
                "consent-caption",
                "This phone's key signs it. The node runs the program's rules; \
                 a refusal comes back in its words.",
            ));
        let body = div()
            .id("confirm")
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pt(px(XS))
            .pb(px(LG))
            .child(head)
            .children(facts)
            .child(warning)
            .child(buttons)
            .child(caption);
        // the Expo sheet's handle, in its band
        let handle = div()
            .flex_none()
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(32.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(color(palette.border_strong)),
            );
        let inset = self.inset(window, cx);
        let room = f32::from(window.viewport_size().height) - inset.top;
        let panel = div()
            .id(SharedString::from(format!("consent-{ask}")))
            .control(Role::Dialog, asked.title)
            .occlude()
            .flex()
            .flex_col()
            .w_full()
            .max_h(px(room))
            .bg(color(palette.background))
            .text_color(color(palette.foreground))
            .border_t_1()
            .border_color(color(palette.border_strong))
            .child(handle)
            .child(body);
        let backdrop_id = SharedString::from(format!("consent-{ask}-backdrop"));
        let overlays = self.overlays.entity().clone();
        div()
            .id(backdrop_id.clone())
            .absolute()
            .top(px(inset.top))
            .left(px(inset.left))
            .right_0()
            .bottom_0()
            .occlude()
            // the Expo palette's `scrim`
            .bg(gpui_kit::black().opacity(if dark { 0.55 } else { 0.4 }))
            .flex()
            .flex_col()
            .justify_end()
            .on_click(move |_, _, cx| {
                overlays.update(cx, |overlays, cx| overlays.close(Overlay::Consent(ask), cx));
            })
            .child(crate::a11y::modal(panel))
            .focus_trap(backdrop_id, &self.modal)
            .into_any_element()
    }
}

/// The phone's spacing ladder (the design crate's, which the Expo kit's
/// `gap` is), and the 20 its confirm card insets its lines by.
const XS: f32 = design::spacing::XS as f32;
const SM: f32 = design::spacing::SM as f32;
const MD: f32 = design::spacing::MD as f32;
const LG: f32 = design::spacing::LG as f32;
const BLOCK: f32 = design::spacing::BLOCK as f32;
const INSET: f32 = 20.;
