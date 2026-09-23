//! The launcher: everything before the desk, in a smaller window of its
//! own size — reaching a node, this device's key, its recovery phrase, and
//! the account on the network. A drawing on the left, one column to read
//! on the right, the way a game client signs in before its main window.
//!
//! Two different things happen here, one after the other. The KEY is this
//! device's: made (and its 24 words written down), restored, or unlocked
//! with its password; it never leaves the device. The ACCOUNT is the
//! network's: created for that key, or an existing one this key joins
//! (a passkey consents). The key comes first; the account step only ever
//! asks about a key already unlocked.

use super::*;
use figure::Figure;
use gpui_kit::component::button::{Button, ButtonRounded};

/// The launcher window's size; it does not change.
pub(super) const LAUNCHER_SIZE: (f32, f32) = (960., 640.);

/// A quiet link above a screen: its element id, its words, what it does.
pub(super) type Back = (&'static str, &'static str, fn() -> Message);

impl DesktopWindow {
    /// One launcher screen: `figure` and its `caption` on the left, then a
    /// small step `label`, a `headline`, a `lead`, and the screen's own
    /// `body`. `back` is a quiet link above the label.
    #[allow(
        clippy::too_many_arguments,
        reason = "one frame, every screen fills it"
    )]
    pub(super) fn launcher(
        &self,
        id: &'static str,
        figure: Figure,
        caption: String,
        back: Option<Back>,
        label: String,
        headline: String,
        lead: Option<String>,
        body: Vec<gpui_kit::AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let palette = design::palette(state.dark);
        let muted = hsla_of(palette.muted);
        // macOS draws the traffic lights over the window's top; that strip
        // is the only handle, so it moves the window.
        let titlebar = cfg!(target_os = "macos") && !window.is_fullscreen();
        let picture = div()
            .id("launcher-figure")
            .w(px(380.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .when(titlebar, |panel| panel.pt(px(40.)))
            .border_r_1()
            .border_color(hsla_of(palette.border))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hsla_of(palette.surface))
                    .child(drawing(figure, state.motion, palette)),
            )
            .child(mono(caption, muted));
        let reading =
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_y_scroll()
                .px(px(40.))
                .pt(px(if titlebar { 44. } else { 32. }))
                .pb(px(32.))
                .flex()
                .flex_col()
                .gap(px(20.))
                .children(back.map(|(key, text, message)| {
                    div().child(self.quiet_link(key, text, message, cx))
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(mono(label, muted))
                        .child(
                            div()
                                .text_size(px(28.))
                                .line_height(px(34.))
                                .child(headline),
                        )
                        .children(lead.map(|lead| {
                            div()
                                .text_size(px(14.))
                                .line_height(px(22.))
                                .text_color(muted)
                                .child(lead)
                        })),
                )
                .children(body);
        div()
            .id("launcher")
            .size_full()
            .flex()
            .when(titlebar, |frame| {
                frame.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(28.))
                        .on_mouse_down(MouseButton::Left, |event, window, _| {
                            match event.click_count {
                                2 => window.titlebar_double_click(),
                                _ => window.start_window_move(),
                            }
                        }),
                )
            })
            .child(picture)
            .child(reading)
            .children(self.footer(cx))
            .into_any_element()
    }

    /// Underlined words that do one thing: "Restore from recovery phrase".
    pub(super) fn quiet_link(
        &self,
        key: &'static str,
        text: &'static str,
        message: fn() -> Message,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::*;
        let model = self.model.clone();
        let muted = hsla_of(design::palette(self.model.read(cx).state.dark()).muted);
        crate::a11y::keyboard(
            div()
                .id(key)
                .control(Role::Button, text)
                .cursor_pointer()
                .text_size(px(13.5))
                .underline()
                .hover(move |style| style.text_color(muted))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    model.update(cx, |model, cx| model.dispatch(message(), cx))
                })
                .child(text),
        )
    }

    /// A labelled field: the words above it, the field, a hint below.
    pub(super) fn field(
        &self,
        label: impl Into<gpui_kit::SharedString>,
        field: gpui_kit::AnyElement,
    ) -> gpui_kit::Div {
        use gpui_kit::*;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.into()),
            )
            .child(field)
    }

    /// A sentence in the danger tone, read out as it appears.
    pub(super) fn alert(
        &self,
        id: &'static str,
        said: String,
        cx: &gpui_kit::App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::*;
        let danger = hsla_of(design::palette(self.model.read(cx).state.dark()).danger);
        div()
            .id(id)
            .role(Role::Alert)
            .aria_label(said.clone())
            .text_size(px(13.))
            .text_color(danger)
            .child(said)
    }
}

/// The launcher's and the desk's buttons: square, a little taller than the
/// kit's.
pub(super) fn square(button: Button) -> Button {
    use gpui_kit::Styled as _;
    button.rounded(ButtonRounded::None).h(gpui_kit::px(40.))
}

/// Small mono text: step labels, captions, counts.
pub(super) fn mono(
    text: impl Into<gpui_kit::SharedString>,
    color: gpui_kit::Hsla,
) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .font_family(design::fonts::FAMILY_MONO)
        .text_size(px(12.))
        .text_color(color)
        .child(text.into())
}

/// The drawing, one line per row, turning while `moving`. Hidden from a
/// reader: it is the screen's mood, not its content.
pub(super) fn drawing(
    figure: Figure,
    moving: bool,
    palette: &design::Palette,
) -> gpui_kit::AnyElement {
    use gpui_kit::*;
    let ink = hsla_of(palette.muted);
    let lines = move |elapsed: u64| {
        div()
            .flex()
            .flex_col()
            .w(px(figure::COLS as f32 * figure::ADVANCE))
            .font_family(design::fonts::FAMILY_MONO)
            // "===" is one glyph in the mono face; a drawing wants three
            .font_features(FontFeatures::disable_ligatures())
            .text_size(px(figure::SIZE))
            .line_height(px(figure::SIZE))
            .text_color(ink)
            .children(figure.frame(elapsed).iter().map(|line| {
                div()
                    .h(px(figure::SIZE))
                    .whitespace_nowrap()
                    .child(line.clone())
            }))
    };
    match moving {
        false => lines(0).into_any_element(),
        true => div()
            .with_animation(
                SharedString::from(format!("figure/{figure:?}")),
                Animation::new(std::time::Duration::from_millis(figure::LOOP_MS))
                    .repeat()
                    .with_max_fps(4.),
                move |frame, delta| frame.child(lines((delta * figure::LOOP_MS as f32) as u64)),
            )
            .into_any_element(),
    }
}
