//! The window ledger's cost, measured with the host's SVG glyphs through the headless wgpu renderer
//! (`picture_pixels::rendering_app`): Linux only, the one host the test renderer builds on, so the
//! host core's source carries no `cfg(target_os)` outside its test modules (`tools/dep-gate.sh`).
#![cfg(target_os = "linux")]

use crate::render::svg_limits::{GlyphSource, svg_admitted_bytes, svg_glyph};
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
    px, size,
};
use std::sync::Arc;

const HELD: usize = 4_096;

struct RealIcons {
    count: usize,
}

impl Render for RealIcons {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .size_full()
            .text_color(gpui_kit::black())
            .children((0..self.count).map(|index| {
                let icon = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" id="icon-{index}"><circle cx="12" cy="12" r="10"/></svg>"#
                );
                div().size(px(24.)).child(svg_glyph(
                    GlyphSource::Data(Arc::from(icon.as_bytes())),
                    view_wire::SvgTransformation {
                        scale: [1., 1.],
                        translate: [0., 0.],
                        rotate: 0.,
                    },
                ))
            }))
    }
}

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
