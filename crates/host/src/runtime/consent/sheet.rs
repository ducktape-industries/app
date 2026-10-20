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
    App, ClickEvent, Div, FontWeight, InteractiveElement as _, ParentElement as _, Role, Stateful,
    StatefulInteractiveElement as _, Styled as _, Text, div, px,
};

const XS: f32 = design::spacing::XS as f32;
const SM: f32 = design::spacing::SM as f32;
const MD: f32 = design::spacing::MD as f32;
const LG: f32 = design::spacing::LG as f32;
const BLOCK: f32 = design::spacing::BLOCK as f32;
const INSET: f32 = 20.;

type Run = Box<dyn Fn(&ClickEvent, &mut App)>;

pub fn counts(click: &ClickEvent) -> bool {
    click.click_count() == 1
}

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
    let words = |id: String, text: String| Text::new(id.into(), text.into());
    let head = div()
        .flex()
        .flex_col()
        .gap(px(XS))
        .px(px(INSET))
        .pb(px(MD))
        .child(
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
    let pressable = |id: &'static str,
                     label: &'static str,
                     ground: Option<design::Color>,
                     edge: Option<design::Color>,
                     tone: design::Color,
                     weight: u16,
                     tall: f32,
                     run: Run| {
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
            .on_click(move |click, _, cx| {
                cx.stop_propagation();
                run(click, cx)
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
        Box::new(move |_: &ClickEvent, cx: &mut App| answer(false, cx)),
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
        Box::new(move |click: &ClickEvent, cx: &mut App| {
            if counts(click) {
                approved(cx)
            }
        }),
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
