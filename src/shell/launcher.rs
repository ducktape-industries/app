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
use facts::Facts;
use figure::Figure;

/// The launcher window's size; it does not change.
pub(super) const LAUNCHER_SIZE: (f32, f32) = (960., 640.);

/// The narrowest the reading column goes beside the drawing: its 40px sides
/// around a 360px field, the address and its button on one line. Narrower,
/// the drawing steps aside and the column takes the window.
const COLUMN_MIN: f32 = 440.;

/// The drawing's panel, `width: 380px`.
const FIGURE_W: f32 = 380.;

// the launcher's own window keeps its drawing
const _: () = assert!(LAUNCHER_SIZE.0 >= FIGURE_W + COLUMN_MIN);

/// A quiet link above a screen: its element id, its words, what it does.
pub(super) type Back = (&'static str, &'static str, fn() -> Message);

/// What a launcher screen puts in the frame (`DesktopWindow::launcher`).
pub(super) struct LauncherScreen {
    /// The reading column's element id: the screen's name to the AX tree.
    pub(super) id: &'static str,
    /// A tighter column (`padding-top: 24px; gap: 16px`) for a long body
    /// (the phrase's 24 words).
    pub(super) tight: bool,
    /// The drawing on the left.
    pub(super) figure: Figure,
    /// The mono line under the drawing.
    pub(super) caption: String,
    /// The small link above the column, if the screen has a way back.
    pub(super) back: Option<Back>,
    /// The step tag over the headline: `[02 / 03] Key`.
    pub(super) label: String,
    /// The `<h1>`.
    pub(super) headline: String,
    /// The paragraph under it.
    pub(super) lead: Option<String>,
    /// The screen's own controls, after the lead.
    pub(super) body: Vec<gpui_kit::AnyElement>,
}

impl DesktopWindow {
    /// One launcher screen in the canvas's frame: on the left a 380px panel,
    /// `padding: 20px; gap: 12px`, the drawing on `surface` and its mono
    /// caption; on the right the reading column, `padding: 32px 40px 0;
    /// gap: 22px`.
    pub(super) fn launcher(
        &self,
        screen: LauncherScreen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let LauncherScreen {
            id,
            tight,
            figure,
            caption,
            back,
            label,
            headline,
            lead,
            body,
        } = screen;
        use gpui_kit::*;
        let state = self.model.read(cx).state.facts();
        let ink = Ink::of(state.dark);
        // removed in s9: `LauncherLayer` writes its figure from its observers
        self.launcher_spin.update(cx, |spin, cx| {
            spin.set(figure, state.motion, ink.figure, cx)
        });
        // The canvas draws a title bar. macOS lends the window's own
        // (transparent, the traffic lights in it); elsewhere the system's
        // title bar is that bar. It is the desk's menu bar height: one
        // window serves both, and its traffic lights sit where they were
        // put when it opened.
        let titlebar = theme::traffic_lights(window).map(|lights| {
            div()
                .id("launcher-titlebar")
                .h(px(super::desk::BAR))
                .flex_shrink_0()
                .flex()
                .items_center()
                .pl(px(lights))
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
            .w(px(FIGURE_W))
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
                    .child(super::spin::drawing(&self.launcher_spin)),
            )
            .child(tag("caption", caption, &ink));
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
                        .child(tag("step", label, &ink))
                        .child(h1("headline", headline, &ink))
                        .children(lead.map(|text| ink::lead("lead", text, &ink))),
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
                    .when(
                        f32::from(window.viewport_size().width) >= FIGURE_W + COLUMN_MIN,
                        |row| row.child(picture),
                    )
                    .child(reading),
            )
            .children(self.footer(cx))
            .into_any_element()
    }

    /// `<label>` over its control, `gap: 8px`, and a line under it; `id`
    /// is the label's.
    pub(super) fn field(
        &self,
        id: impl Into<gpui_kit::ElementId>,
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
            .child(label(id, text, ink))
            .child(control)
            .children(below)
    }
}

/// The canvas's button row: `display: flex; gap: 12px; margin-top: 4px`.
pub(super) fn buttons(children: impl IntoIterator<Item = gpui_kit::AnyElement>) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .flex_wrap()
        .gap(px(12.))
        .mt(px(4.))
        .children(children)
}

/// The canvas's closing links: `gap: 10px; padding-top: 20px;
/// border-top: 1px solid line`.
pub(super) fn closing(
    children: impl IntoIterator<Item = gpui_kit::AnyElement>,
    ink: &Ink,
) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(10.))
        .pt(px(20.))
        .border_t_1()
        .border_color(ink.line)
        .children(children)
}

/// The node reached, as the drawing's caption: "testkit · 127.0.0.1:8844".
pub(super) fn node_caption(state: &Facts) -> String {
    let host = crate::backend::host_of(&state.connected_rpc);
    format!("{} · {host}", state.network)
}
