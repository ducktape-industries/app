//! The ask under touch: the phone's confirm sheet (mobile
//! `src/kit/Confirm.tsx`, board 96), its content: the ask's title as an
//! eyebrow, "<verb>: <target>", the op's facts, what it takes away, Cancel
//! and the verb (in danger for one that takes something away), and the
//! caption on who signs; every size the phone's own. Each shell docks it
//! in its own sheet (the phone's, the desktop's under touch).

use super::Words;
use crate::a11y::Control as _;
use crate::fonts::{FAMILY_MONO, FAMILY_UI};
use crate::theme::hsla as color;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _, Role, Stateful,
    StatefulInteractiveElement as _, Styled as _, Text, div, px,
};

/// The phone's spacing ladder (the design crate's, which the Expo kit's
/// `gap` is), and the 20 its confirm card insets its lines by.
const XS: f32 = design::spacing::XS as f32;
const SM: f32 = design::spacing::SM as f32;
const MD: f32 = design::spacing::MD as f32;
const LG: f32 = design::spacing::LG as f32;
const BLOCK: f32 = design::spacing::BLOCK as f32;
const INSET: f32 = 20.;

/// The sheet's content for `asked`, scrolling when it outgrows the sheet:
/// Cancel answers no (`answer(false)`), the verb `approved`.
pub fn sheet(
    asked: Words,
    dark: bool,
    approved: impl Fn(&mut App) + 'static,
    answer: impl Fn(bool, &mut App) + 'static,
) -> Stateful<Div> {
    let palette = design::palette(dark);
    let card = asked.card;
    let face = |size: f32, line: f32, weight: u16, mono: bool| {
        div()
            .font_family(match mono {
                true => FAMILY_MONO,
                false => FAMILY_UI,
            })
            .font_weight(FontWeight(f32::from(weight)))
            .text_size(px(size))
            .line_height(px(line))
    };
    // words assistive technology reads: a `Label` whose value is the text
    let words = |id: String, text: String| Text::new(id.into(), text.into());
    // the kit's `Text` roles: title 17/21, small 14/19, body 15/21,
    // secondary 13/18, caption 12/16, eyebrow mono 11/14 uppercase
    let head = div()
        .flex()
        .flex_col()
        .gap(px(XS))
        .px(px(INSET))
        .pb(px(MD))
        .child(
            // drawn in capitals, read as written, as a view's text with a
            // transform is: one Label node, the drawn words in no node
            face(11., 14., 400, true)
                .id("consent-title")
                .role(Role::Label)
                .aria_value(asked.title)
                .text_color(color(palette.muted))
                .child(asked.title.to_uppercase()),
        )
        .child(
            face(17., 21., 500, false)
                .text_color(color(palette.foreground))
                .child(words("consent-card-title".into(), card.title)),
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
        .child(words("consent-warning".into(), card.warning));
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
            "consent-caption".into(),
            "This phone's key signs it. The node runs the program's rules; \
             a refusal comes back in its words."
                .into(),
        ));
    div()
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
        .child(caption)
}
