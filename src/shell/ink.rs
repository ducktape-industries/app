//! The design canvas's vocabulary, one to one: its colors, and the handful
//! of elements every screen is built from, each with the canvas's own CSS
//! values (`font: 500 15px`, `height: 44px`, `border: 1.5px solid`). A
//! screen is ported from its board by reading the DOM and writing these.

use super::*;
use gpui_kit::*;

/// The canvas's colors for one appearance.
#[derive(Clone, Copy)]
pub(super) struct Ink {
    pub(super) bg: Hsla,
    /// text, the primary fill, the chosen pane's frame
    pub(super) ink: Hsla,
    pub(super) muted: Hsla,
    /// hairlines
    pub(super) line: Hsla,
    /// a field's border
    pub(super) strong: Hsla,
    /// the figure's panel, a chosen row
    pub(super) surface: Hsla,
    /// the drawing in characters
    pub(super) figure: Hsla,
    pub(super) danger: Hsla,
    pub(super) ok: Hsla,
    pub(super) ok_soft: Hsla,
}

fn rgb(value: u32) -> Hsla {
    gpui_kit::rgb(value).into()
}

impl Ink {
    pub(super) fn of(dark: bool) -> Self {
        match dark {
            false => Self {
                bg: rgb(0xFFFFFF),
                ink: rgb(0x111111),
                muted: rgb(0x6B6B6B),
                line: rgb(0xE6E6E6),
                strong: rgb(0xCFCFCF),
                surface: rgb(0xF5F5F3),
                figure: rgb(0x3A3A3A),
                danger: rgb(0xB42318),
                ok: rgb(0x2E7D32),
                ok_soft: rgb(0x8BC79A),
            },
            true => Self {
                bg: rgb(0x111111),
                ink: rgb(0xEDEDED),
                muted: rgb(0x8F8F8F),
                line: rgb(0x2A2A2A),
                strong: rgb(0x3A3A3A),
                surface: rgb(0x1A1A1A),
                figure: rgb(0xBDBDBD),
                danger: rgb(0xF97066),
                ok: rgb(0x6FCF97),
                ok_soft: rgb(0x2F6B47),
            },
        }
    }
}

/// The canvas is a board to present: its type reads large in a desktop
/// window. Every size in these ports is the canvas's own, put through this:
/// up to 13px it stands, above it the step is roughly halved (32 → 23,
/// 16 → 14.7, 15 → 14.1). The one knob for the whole shell's scale.
pub(super) fn fit(size: f32) -> f32 {
    match size <= 13. {
        true => size,
        false => 13. + (size - 13.) * 0.55,
    }
}

/// A control's height, shrunk with its type (44 → 36, 34 → 28, 56 → 46).
pub(super) fn tall(height: f32) -> f32 {
    (height * 0.82).round()
}

/// `font: <weight> <size>px 'Instrument Sans'`.
pub(super) fn sans(weight: u16, size: f32) -> Div {
    div()
        .font_family(super::theme::FAMILY_UI)
        .font_weight(FontWeight(weight as f32))
        .text_size(px(fit(size)))
}

/// `font: <weight> <size>px 'IBM Plex Mono'`.
pub(super) fn mono(weight: u16, size: f32) -> Div {
    div()
        .font_family(super::theme::FAMILY_MONO)
        .font_weight(FontWeight(weight as f32))
        .text_size(px(fit(size)))
}

/// Words assistive technology reads: a `Label` whose value is `text`, as
/// gpui's own `Text` has it. `id` is explicit: the `text!` macro derives
/// one from its call site, and text mapped over a list would share it.
pub(super) fn words(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Text {
    Text::new(id.into(), text.into())
}

/// `<h1>`: `400 32px/1.18`; the screen's one `Heading`, level 1, its words
/// its name and its value, as the presenter's headings carry them. One
/// node: a `Text` inside would be a second one reading the same words.
pub(super) fn h1(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    ink: &Ink,
) -> Stateful<Div> {
    let text = text.into();
    sans(400, 32.)
        .id(id)
        .role(Role::Heading)
        .aria_label(text.clone())
        .aria_value(text.clone())
        .aria_level(1)
        .line_height(px(fit(32.) * 1.18))
        .text_color(ink.ink)
        .child(text)
}

/// The lead under a headline: `400 16px/1.65`, in ink.
pub(super) fn lead(id: impl Into<ElementId>, text: impl Into<SharedString>, ink: &Ink) -> Div {
    sans(400, 16.)
        .line_height(px(fit(16.) * 1.65))
        .text_color(ink.ink)
        .child(words(id, text))
}

/// A small muted note: `400 13px/1.55`.
pub(super) fn note(id: impl Into<ElementId>, text: impl Into<SharedString>, color: Hsla) -> Div {
    sans(400, 13.)
        .line_height(px(13. * 1.55))
        .text_color(color)
        .child(words(id, text))
}

/// A step label or caption: `400 12px MONO`, muted.
pub(super) fn tag(id: impl Into<ElementId>, text: impl Into<SharedString>, ink: &Ink) -> Div {
    mono(400, 12.).text_color(ink.muted).child(words(id, text))
}

/// A field's `<label>`: `500 14px`.
pub(super) fn label(id: impl Into<ElementId>, text: impl Into<SharedString>, ink: &Ink) -> Div {
    sans(500, 14.).text_color(ink.ink).child(words(id, text))
}

/// The canvas's button kinds: a filled ink block, an ink outline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Primary,
    Secondary,
    /// the outline at `height: 34px; padding: 0 12px; font-size: 14px`
    Small,
}

impl DesktopWindow {
    /// `<button>`: `height 44px; padding 0 18px; border 1.5px solid ink;
    /// font 500 15px`, filled for [`Kind::Primary`]. Disabled reads at 0.3.
    pub(super) fn button(
        &self,
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        kind: Kind,
        message: fn() -> Message,
        disabled: bool,
        ink: &Ink,
    ) -> AnyElement {
        let text = text.into();
        let model = self.model.clone();
        let (bg, fg) = match kind {
            Kind::Primary => (ink.ink, ink.bg),
            Kind::Secondary | Kind::Small => (gpui_kit::transparent_black(), ink.ink),
        };
        let (height, pad, size) = match kind {
            Kind::Small => (34., 12., 14.),
            _ => (44., 18., 15.),
        };
        let button = sans(500, size)
            .id(id)
            .control(Role::Button, text.clone())
            .h(px(tall(height)))
            .px(px(pad))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border(px(1.5))
            .border_color(ink.ink)
            .bg(bg)
            .text_color(fg)
            .when(disabled, |button| button.opacity(0.3))
            .when(!disabled, |button| {
                button.cursor_pointer().on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    model.update(cx, |model, cx| model.dispatch(message(), cx))
                })
            })
            .child(text);
        crate::a11y::keyboard(button)
            .aria_disabled(disabled)
            .into_any_element()
    }

    /// `<a>`: `400 15px`, underlined, in ink; `small` is the back link's
    /// `13px` muted.
    pub(super) fn link(
        &self,
        id: &'static str,
        text: impl Into<SharedString>,
        message: fn() -> Message,
        small: bool,
        ink: &Ink,
    ) -> AnyElement {
        let text = text.into();
        let model = self.model.clone();
        let (size, color) = match small {
            true => (13., ink.muted),
            false => (15., ink.ink),
        };
        let hover = ink.muted;
        crate::a11y::keyboard(
            sans(400, size)
                .id(id)
                .control(Role::Button, text.clone())
                .text_color(color)
                .underline()
                .cursor_pointer()
                .hover(move |style| style.text_color(hover))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    model.update(cx, |model, cx| model.dispatch(message(), cx))
                })
                .child(text),
        )
        .into_any_element()
    }

    /// `<span role=alert>`: `400 13px/1.5` in danger, read out at once as
    /// it shows.
    pub(super) fn alert(&self, id: &'static str, said: String, ink: &Ink) -> AnyElement {
        let alert = sans(400, 13.)
            .line_height(px(13. * 1.5))
            .id(id)
            .role(Role::Alert)
            .text_color(ink.danger);
        crate::a11y::live(alert, accesskit::Live::Assertive, said.clone())
            .child(said)
            .into_any_element()
    }
}

/// `<input>`'s box around a bare field: `height 44px; padding 0 12px;
/// border 1px solid strong; font 400 15px`. `border` recolors it (an
/// error's danger, a match's green).
pub(super) fn field_box(field: AnyElement, border: Hsla, height: f32, ink: &Ink) -> Div {
    sans(400, 15.)
        .h(px(tall(height)))
        .px(px(12.))
        .flex()
        .items_center()
        .border_1()
        .border_color(border)
        .bg(ink.bg)
        .text_color(ink.ink)
        .child(div().flex_1().min_w_0().child(field))
}

#[cfg(test)]
mod tests {
    use super::Ink;
    use gpui_kit::{Hsla, Rgba};

    /// WCAG 2.2's contrast ratio of two opaque colors.
    fn contrast(one: Hsla, other: Hsla) -> f32 {
        let luminance = |color: Hsla| {
            let Rgba { r, g, b, .. } = color.to_rgb();
            let linear = |v: f32| match v <= 0.04045 {
                true => v / 12.92,
                false => ((v + 0.055) / 1.055).powf(2.4),
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        };
        let (one, other) = (luminance(one), luminance(other));
        (one.max(other) + 0.05) / (one.min(other) + 0.05)
    }

    /// Each pair, on the light theme and the dark, reads at `floor`:1.
    fn reach(floor: f32, pairs: fn(&Ink) -> Vec<(&'static str, Hsla, Hsla)>) {
        for (theme, dark) in [("light", false), ("dark", true)] {
            for (what, fore, back) in pairs(&Ink::of(dark)) {
                let ratio = contrast(fore, back);
                assert!(
                    ratio >= floor,
                    "{what}, {theme}: {ratio:.2}:1, under {floor}:1"
                );
            }
        }
    }

    /// AX-121: the words the shell draws, on each ground it draws them on.
    #[test]
    fn text_reads_at_four_and_a_half_to_one() {
        reach(4.5, |ink| {
            vec![
                ("text", ink.ink, ink.bg),
                ("text on a chosen row", ink.ink, ink.surface),
                ("muted text", ink.muted, ink.bg),
                ("muted text on a chosen row", ink.muted, ink.surface),
                ("a filled button's words", ink.bg, ink.ink),
                ("an error", ink.danger, ink.bg),
                ("an unread notification", ink.figure, ink.bg),
                ("the drawing in characters", ink.figure, ink.surface),
            ]
        });
    }

    /// AX-121: the focus ring over every fill it lands on, and the marks
    /// that say a state.
    #[test]
    fn the_focus_ring_and_state_marks_read_at_three_to_one() {
        reach(3., |ink| {
            let ring = crate::a11y::ring().color;
            vec![
                ("the focus ring", ring, ink.bg),
                ("the focus ring on a chosen row", ring, ink.surface),
                ("the focus ring on a filled button", ring, ink.ink),
                ("a chosen pane's frame, a switch on", ink.ink, ink.bg),
                ("the node's dot, in sync", ink.ok, ink.bg),
                (
                    "the node's dot, not answering; a failed tab",
                    ink.danger,
                    ink.bg,
                ),
            ]
        });
    }

    /// AX-121's control boundary: a field's border, a switch off, the
    /// segmented row's frame.
    #[test]
    #[ignore = "a look change awaiting the owner (docs/ax.md §6 question 1): `strong` reads 1.56:1 light, 1.66:1 dark"]
    fn a_field_border_reads_at_three_to_one() {
        reach(3., |ink| vec![("a field's border", ink.strong, ink.bg)]);
    }
}
