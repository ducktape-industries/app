//! The launcher: everything before the desk, in a smaller window of its
//! own size — reaching a node, this device's key, its recovery phrase, and
//! the account on the network. A drawing on the left, one column to read
//! on the right, the way a game client signs in before its main window.
//!
//! Two different things happen here, one after the other. The KEY is this
//! device's: kept by the system and opened on its own; it never leaves the
//! device. The ACCOUNT is the network's: created for that key, or one this
//! key joins (another device, a passkey or a recovery key consents).
//!
//! Every screen is ported from its board on the design canvas, element by
//! element, out of `ink`'s pieces.

use super::ink::{self, *};
use super::*;
use figure::Figure;

/// The launcher window's size; it does not change.
pub(super) const LAUNCHER_SIZE: (f32, f32) = (960., 640.);

/// A quiet link above a screen: its element id, its words, what it does.
pub(super) type Back = (&'static str, &'static str, fn() -> Message);

impl DesktopWindow {
    /// One launcher screen, the canvas's frame: on the left a 380px panel,
    /// `padding: 20px; gap: 12px`, the drawing on `surface` and its mono
    /// `caption`; on the right the reading column, `padding: 32px 40px 0;
    /// gap: 22px` — a small back link, then `[step]`, the `<h1>` and the
    /// lead (`gap: 18px`), then the screen's own `body`.
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
        let ink = Ink::of(state.dark);
        // The canvas draws a title bar. macOS lends the window's own
        // (transparent, the traffic lights in it); elsewhere the system's
        // title bar is that bar. It is the desk's menu bar height: one
        // window serves both, and its traffic lights sit where they were
        // put when it opened.
        let titlebar = (cfg!(target_os = "macos") && !window.is_fullscreen()).then(|| {
            div()
                .id("launcher-titlebar")
                .h(px(super::desk::BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .pl(px(78.))
                .border_b_1()
                .border_color(ink.line)
                .on_mouse_down(MouseButton::Left, |event, window, _| {
                    match event.click_count {
                        2 => window.titlebar_double_click(),
                        _ => window.start_window_move(),
                    }
                })
                .child(sans(500, 13.).text_color(ink.ink).child("Ducktape"))
        });
        let picture = div()
            .id("launcher-figure")
            .w(px(380.))
            .flex_shrink_0()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .border_r_1()
            .border_color(ink.line)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(ink.surface)
                    .child(super::spin::drawing(
                        "launcher-figure-drawing",
                        figure,
                        state.motion,
                        ink.figure,
                        window,
                        cx,
                    )),
            )
            .child(tag(caption, &ink));
        // the phrase's 24 words need the room: its column sits tighter
        let tight = id == "recovery";
        let reading =
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .overflow_y_scroll()
                .pt(px(if tight { 24. } else { 32. }))
                .px(px(40.))
                .pb(px(32.))
                .flex()
                .flex_col()
                .gap(px(if tight { 16. } else { 22. }))
                .children(back.map(|(key, text, message)| {
                    div().child(self.link(key, text, message, true, &ink))
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child(tag(label, &ink))
                        .child(h1(headline, &ink))
                        .children(lead.map(|text| ink::lead(text, &ink))),
                )
                .children(body);
        div()
            .id("launcher")
            .size_full()
            .flex()
            .flex_col()
            .bg(ink.bg)
            .text_color(ink.ink)
            .children(titlebar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(picture)
                    .child(reading),
            )
            .children(self.footer(cx))
            .into_any_element()
    }

    /// `<label>` over its control, `gap: 8px`, and a line under it.
    pub(super) fn field(
        &self,
        text: impl Into<gpui_kit::SharedString>,
        control: gpui_kit::AnyElement,
        below: Option<gpui_kit::AnyElement>,
        ink: &Ink,
    ) -> gpui_kit::Div {
        use gpui_kit::*;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(label(text, ink))
            .child(control)
            .children(below)
    }
}
