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
    /// a switch off, the segmented row's frame, a window's frame on the desk
    pub(super) strong: Hsla,
    /// a text field's border: 3:1 against the page and a chosen row
    /// (AX-121; owner, 2026-09-28)
    pub(super) field: Hsla,
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
    /// The colour a control's focus ring and border take: ink, and on a
    /// control filled with ink the page's colour, so the ring stays seen.
    pub(super) fn ring(&self, filled: bool) -> Hsla {
        match filled {
            true => self.bg,
            false => self.ink,
        }
    }

    pub(super) fn of(dark: bool) -> Self {
        match dark {
            false => Self {
                bg: rgb(0xFFFFFF),
                ink: rgb(0x111111),
                muted: rgb(0x6B6B6B),
                line: rgb(0xE6E6E6),
                strong: rgb(0xCFCFCF),
                field: rgb(0x8E8E8E),
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
                field: rgb(0x666666),
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

/// `<label>` over its control, `gap: 8px`, and a line under it; `id` is
/// the label's.
pub(super) fn field(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    control: AnyElement,
    below: Option<AnyElement>,
    ink: &Ink,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(label(id, text, ink))
        .child(control)
        .children(below)
}

/// Whether a button takes a press: `Off` reads at 0.3, and `Busy` is off
/// because the work it starts is in flight, which it says to assistive
/// technology too. A `bool` is whether it is off.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Press {
    Ready,
    Off,
    Busy,
}

impl Press {
    pub(super) fn busy(busy: bool) -> Self {
        match busy {
            true => Self::Busy,
            false => Self::Ready,
        }
    }
}

impl From<bool> for Press {
    fn from(off: bool) -> Self {
        match off {
            true => Self::Off,
            false => Self::Ready,
        }
    }
}

/// The canvas's button kinds: a filled ink block, an ink outline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Primary,
    Secondary,
    /// the outline at `height: 34px; padding: 0 12px; font-size: 14px`
    Small,
}

/// `<button>`: `height 44px; padding 0 18px; border 1.5px solid ink;
/// font 500 15px`, filled for [`Kind::Primary`]. Off or busy, it reads at
/// 0.3 ([`Press`]). Pressed, it dispatches `message` through `model`.
#[allow(clippy::too_many_arguments, reason = "one button, seven facts")]
pub(super) fn button(
    model: &Entity<Desktop>,
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    kind: Kind,
    message: fn() -> Message,
    press: impl Into<Press>,
    ink: &Ink,
) -> AnyElement {
    let press = press.into();
    let disabled = press != Press::Ready;
    let text = text.into();
    let model = model.clone();
    let (bg, fg) = match kind {
        Kind::Primary => (ink.ink, ink.bg),
        Kind::Secondary | Kind::Small => (gpui_kit::transparent_black(), ink.ink),
    };
    let filled = matches!(kind, Kind::Primary);
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
    let button = crate::a11y::keyboard(button, ink.ring(filled)).aria_disabled(disabled);
    match press {
        Press::Busy => crate::a11y::Patch::default().busy().on(button),
        _ => button,
    }
    .into_any_element()
}

/// `<span role=alert>`: `400 13px/1.5` in danger, read out at once as it
/// shows.
pub(super) fn alert(id: &'static str, said: String, ink: &Ink) -> AnyElement {
    let alert = sans(400, 13.)
        .line_height(px(13. * 1.5))
        .id(id)
        .role(Role::Alert)
        .text_color(ink.danger);
    crate::a11y::live(alert, accesskit::Live::Assertive, said.clone())
        .child(said)
        .into_any_element()
}

/// `<a>`: `400 15px`, underlined, in ink; `small` is the back link's
/// `13px` muted. Pressed, it runs `run`.
pub(super) fn link_running(
    id: &'static str,
    text: impl Into<SharedString>,
    run: impl Fn(&mut App) + 'static,
    small: bool,
    ink: &Ink,
) -> AnyElement {
    let text = text.into();
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
                run(cx)
            })
            .child(text),
        ink.ink,
    )
    .into_any_element()
}

/// `<input>`'s box around a bare field: `height 44px; padding 0 12px;
/// border 1px solid field; font 400 15px`. `border` recolors it (an
/// error's danger). While the field is `focused` the box
/// wears its ring, and the ring's ink hides that colour: the note under the
/// field still says it.
pub(super) fn field_box(
    field: AnyElement,
    focused: bool,
    border: Hsla,
    height: f32,
    ink: &Ink,
) -> Div {
    let field_box = sans(400, 15.)
        .h(px(tall(height)))
        .px(px(12.))
        .flex()
        .items_center()
        .border_1()
        .border_color(border)
        .bg(ink.bg)
        .text_color(ink.ink)
        .child(div().flex_1().min_w_0().child(field));
    crate::a11y::around_field(field_box, focused, ink.ring(false))
}

#[cfg(test)]
mod tests {
    use super::Ink;
    use gpui_kit::{Hsla, IntoElement as _, Rgba, Styled as _};

    /// A focused field's box wears the ring, its border in the ring's ink
    /// over an error's danger; unfocused, the box keeps its border colour.
    #[test]
    fn a_focused_fields_box_wears_the_ring() {
        let ink = Ink::of(false);
        let field = || gpui_kit::div().into_any_element();
        let mut on = super::field_box(field(), true, ink.danger, 44., &ink);
        assert_eq!(
            on.style().box_shadow,
            Some(vec![crate::a11y::ring(ink.ring(false))])
        );
        assert_eq!(on.style().border_color, Some(ink.ring(false)));
        let mut off = super::field_box(field(), false, ink.danger, 44., &ink);
        assert_eq!(off.style().box_shadow, None);
        assert_eq!(off.style().border_color, Some(ink.danger));
    }

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
            let (ring, inverted) = (ink.ring(false), ink.ring(true));
            vec![
                ("the focus ring", ring, ink.bg),
                ("the focus ring on a chosen row", ring, ink.surface),
                ("the focus ring on a switch off", ring, ink.strong),
                ("the focus ring on a filled button", inverted, ink.ink),
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

    /// The ring is the theme's ink, so a view's field and link wear the
    /// colour the shell's buttons do.
    #[gpui_kit::test]
    fn the_ring_is_the_themes_ink(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::component::{Theme, ThemeMode};
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::shell::theme::configure_native_theme(cx);
            for (mode, dark) in [(ThemeMode::Light, false), (ThemeMode::Dark, true)] {
                Theme::change(mode, None, cx);
                assert_eq!(crate::a11y::ink(cx), Ink::of(dark).ink);
            }
        });
    }

    /// What a keyboard-focused shell button (`ink::button`'s own)
    /// shows, and a mouse-moved one does not: a 2px ink ring and border on
    /// the outline button, the page's colour on the ink-filled Primary, and
    /// nothing once the last input was the mouse.
    #[gpui_kit::test]
    fn a_keyboard_focused_button_wears_an_ink_ring_that_inverts_on_ink(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{Desktop, Kind, Message, Press, button};
        use gpui_kit::test::TestWindowExt as _;
        use gpui_kit::{
            BoxShadow, Context, Div, Entity, FocusHandle, InteractiveElement as _, IntoElement,
            Modifiers, Render, Stateful, Styled as _, VisualTestContext, Window, div, point, px,
            size,
        };
        use std::{cell::RefCell, rc::Rc};
        type Seen = Rc<RefCell<Vec<(Vec<BoxShadow>, Option<Hsla>)>>>;
        struct Buttons(Entity<Desktop>, Ink, [FocusHandle; 2], Seen);
        impl Render for Buttons {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let (desktop, ink) = (self.0.clone(), self.1);
                let (focus, seen) = (self.2.clone(), self.3.clone());
                gpui_kit::canvas(
                    move |_, window, cx| {
                        let mut both = Vec::new();
                        for (nth, kind) in [Kind::Secondary, Kind::Primary].into_iter().enumerate()
                        {
                            let message = || Message::DismissToast;
                            let mut button =
                                button(&desktop, nth, "Go", kind, message, Press::Ready, &ink);
                            let button = button
                                .downcast_mut::<Stateful<Div>>()
                                .expect("a shell button is a Stateful<Div>");
                            // the handle a frame would give it
                            let mut button = std::mem::replace(button, div().id("taken"))
                                .track_focus(&focus[nth]);
                            let style =
                                button.interactivity().compute_style(None, None, window, cx);
                            both.push((style.box_shadow, style.border_color));
                        }
                        *seen.borrow_mut() = both;
                    },
                    |_, _, _, _| {},
                )
                .size_full()
            }
        }
        cx.update(gpui_kit::init);
        let (root, _) = crate::shell::screens_tests::open(crate::Ducktape::boot().0, cx);
        let desktop = cx.update(|cx| root.read(cx).model.clone());
        for dark in [false, true] {
            let ink = Ink::of(dark);
            let seen = Seen::default();
            let focus = cx.update(|cx| [cx.focus_handle(), cx.focus_handle()]);
            let window = cx.open_window(size(px(200.), px(200.)), {
                let (desktop, focus, seen) = (desktop.clone(), focus.clone(), seen.clone());
                move |_, _| Buttons(desktop, ink, focus, seen)
            });
            let mut native = VisualTestContext::from_window(window.into(), cx);
            let shown = |native: &mut VisualTestContext| {
                native.update(|window, cx| window.render_frame(cx));
                seen.borrow().clone()
            };
            // both are drawn with an ink border
            let bare = (vec![], Some(ink.ink));
            assert_eq!(shown(&mut native), vec![bare.clone(), bare.clone()]);
            let ring = |color| {
                let ring = BoxShadow {
                    color,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: true,
                };
                (vec![ring], Some(color))
            };
            // Tab lands on the outline button: ink
            native.update(|window, cx| focus[0].focus(window, cx));
            native.simulate_keystrokes("shift");
            assert_eq!(shown(&mut native), vec![ring(ink.ink), bare.clone()]);
            // and on the ink-filled one: the page's colour, over its border too
            native.update(|window, cx| focus[1].focus(window, cx));
            assert_eq!(shown(&mut native), vec![bare.clone(), ring(ink.bg)]);
            // a click moves the mouse: the focus stays, the ring goes
            native.simulate_click(point(px(5.), px(5.)), Modifiers::default());
            assert_eq!(shown(&mut native), vec![bare.clone(), bare]);
        }
    }

    /// AX-121's control boundary: a text field's border, on the page and
    /// on a chosen row.
    #[test]
    fn a_field_border_reads_at_three_to_one() {
        reach(3., |ink| {
            vec![
                ("a field's border", ink.field, ink.bg),
                ("a field's border on a chosen row", ink.field, ink.surface),
            ]
        });
    }
}
