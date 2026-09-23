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

/// `font: <weight> <size>px 'Instrument Sans'`.
pub(super) fn sans(weight: u16, size: f32) -> Div {
    div()
        .font_family(super::theme::FAMILY_UI)
        .font_weight(FontWeight(weight as f32))
        .text_size(px(size))
}

/// `font: <weight> <size>px 'IBM Plex Mono'`.
pub(super) fn mono(weight: u16, size: f32) -> Div {
    div()
        .font_family(super::theme::FAMILY_MONO)
        .font_weight(FontWeight(weight as f32))
        .text_size(px(size))
}

/// `<h1>`: `400 32px/1.18`.
pub(super) fn h1(text: impl Into<SharedString>, ink: &Ink) -> Div {
    sans(400, 32.)
        .line_height(px(32. * 1.18))
        .text_color(ink.ink)
        .child(text.into())
}

/// The lead under a headline: `400 16px/1.65`, in ink.
pub(super) fn lead(text: impl Into<SharedString>, ink: &Ink) -> Div {
    sans(400, 16.)
        .line_height(px(16. * 1.65))
        .text_color(ink.ink)
        .child(text.into())
}

/// A small muted note: `400 13px/1.55`.
pub(super) fn note(text: impl Into<SharedString>, color: Hsla) -> Div {
    sans(400, 13.)
        .line_height(px(13. * 1.55))
        .text_color(color)
        .child(text.into())
}

/// A step label or caption: `400 12px MONO`, muted.
pub(super) fn tag(text: impl Into<SharedString>, ink: &Ink) -> Div {
    mono(400, 12.).text_color(ink.muted).child(text.into())
}

/// A field's `<label>`: `500 14px`.
pub(super) fn label(text: impl Into<SharedString>, ink: &Ink) -> Div {
    sans(500, 14.).text_color(ink.ink).child(text.into())
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
            .h(px(height))
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
        crate::a11y::disabled(crate::a11y::keyboard(button), disabled).into_any_element()
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

    /// `<span role=alert>`: `400 13px/1.5` in danger, read out as it shows.
    pub(super) fn alert(&self, id: &'static str, said: String, ink: &Ink) -> AnyElement {
        sans(400, 13.)
            .line_height(px(13. * 1.5))
            .id(id)
            .role(Role::Alert)
            .aria_label(said.clone())
            .text_color(ink.danger)
            .child(said)
            .into_any_element()
    }
}

/// `<input>`'s box around a bare field: `height 44px; padding 0 12px;
/// border 1px solid strong; font 400 15px`. `border` recolors it (an
/// error's danger, a match's green).
pub(super) fn field_box(field: AnyElement, border: Hsla, height: f32, ink: &Ink) -> Div {
    sans(400, 15.)
        .h(px(height))
        .px(px(12.))
        .flex()
        .items_center()
        .border_1()
        .border_color(border)
        .bg(ink.bg)
        .text_color(ink.ink)
        .child(div().flex_1().min_w_0().child(field))
}
