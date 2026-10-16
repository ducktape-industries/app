//! The window ledger's cost, measured with gpui's own SVG element through
//! the headless wgpu renderer (`picture_pixels::rendering_app`): Linux only,
//! the one host the test renderer builds on. Lives here, not in
//! `svg_limits.rs`, so the host core's source carries no `cfg(target_os)`
//! outside its test modules (`tools/dep-gate.sh`).
#![cfg(target_os = "linux")]

use crate::render::svg_limits::{SvgPaintSource, guarded_svg_paint, svg_admitted_bytes};
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
    px, size, svg,
};
use std::sync::Arc;

const HELD: usize = 4_096;

/// `count` distinct 24px icons drawn by gpui's own SVG element.
struct RealIcons {
    count: usize,
}

impl Render for RealIcons {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .size_full()
            .children((0..self.count).map(|index| {
                let icon = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" id="icon-{index}"><circle cx="12" cy="12" r="10"/></svg>"#
                );
                guarded_svg_paint(
                    SvgPaintSource::Data(Arc::from(icon.as_bytes())),
                    svg()
                        .data(icon.as_bytes())
                        .size(px(24.))
                        .text_color(gpui_kit::black()),
                )
            }))
    }
}

/// The window ledger's cost, measured: one window drawing 4,096
/// distinct 24px icons through gpui's rasteriser into wgpu's atlas (`rendering_app`). With no GPU, Mesa's software
/// Vulkan keeps the atlas in this process, so resident memory counts
/// it. The window is drawn and read back empty first, so the readback
/// target is not counted. Run alone: `cargo test
/// render::svg_limits::tests::a_window_of_4096_icons --
/// --ignored --exact --nocapture`.
#[test]
#[ignore = "measurement: prints resident memory and the ledger, asserts nothing of them"]
fn a_window_of_4096_icons() {
    let mut cx = crate::render::tests::picture_pixels::rendering_app();
    let window = cx
        .open_window(size(px(1536.), px(1536.)), |_, cx| {
            cx.new(|_| RealIcons { count: 0 })
        })
        .unwrap();
    let drawn = |cx: &mut gpui_kit::HeadlessAppContext| {
        let (scale, ledger, refusals) = cx
            .update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                (
                    window.scale_factor(),
                    svg_admitted_bytes(window.window_handle().window_id(), cx),
                    window.painted_quads().len(),
                )
            })
            .unwrap();
        // the renderer uploads the frame's tiles when it draws it
        cx.capture_screenshot(window.into()).unwrap();
        (scale, ledger, refusals)
    };
    drawn(&mut cx);
    let before = crate::render::tests::resident_mib();
    cx.update(|cx| {
        window.update(cx, |icons, _, cx| {
            icons.count = HELD;
            cx.notify();
        })
    })
    .unwrap();
    let (scale, ledger, refusals) = drawn(&mut cx);
    let after = crate::render::tests::resident_mib();
    println!(
        "{HELD} icons at scale {scale}: ledger {ledger} B, {refusals} refusal quads, resident +{:.1} MiB",
        after - before,
    );
}
