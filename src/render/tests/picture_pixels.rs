//! Pictures as a real renderer draws them: wgpu's headless renderer (on a
//! box with no GPU, Mesa's software Vulkan) draws the scene the window
//! painted, and the test reads its pixels back.
use super::*;
use gpui_kit::{HeadlessAppContext, PlatformHeadlessRenderer};

/// A square filling its whole box.
const SQUARE: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path d="M0 0h24v24H0z"/></svg>"#;
const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// An app whose windows draw through wgpu's headless renderer, so its
/// atlas is a GPU's and `capture_screenshot` reads back what it drew.
pub(in crate::render) fn rendering_app() -> HeadlessAppContext {
    let text = Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
        "fallback",
    ));
    let mut cx = HeadlessAppContext::with_platform(text, Arc::new(()), || {
        let renderer: Box<dyn PlatformHeadlessRenderer> =
            Box::new(gpui_wgpu::WgpuHeadlessRenderer::new()?);
        Ok(Some(renderer))
    });
    cx.update(gpui_kit::init);
    cx
}

/// A 30px guest Svg of [`SQUARE`] with `style` on top.
fn glyph(style: gpui_kit::Div) -> wire::Node {
    wire::Node::Svg {
        id: Some(named_id("glyph")),
        source: wire::SvgSource::Data {
            hash: 1,
            bytes: Some(SQUARE.to_vec()),
        },
        transformation: wire::SvgTransformation {
            scale: [1.0, 1.0],
            translate: [0.0, 0.0],
            rotate: 0.0,
        },
        label: None,
        style: style.size(px(30.)).style().clone(),
        interactivity: Default::default(),
    }
}

/// `child` at the top left of a white 60px page, read back as the
/// renderer drew it: the pixel at the middle of the glyph's box, and one
/// outside it.
fn painted(page: gpui_kit::Div, child: wire::Node) -> ([u8; 4], [u8; 4]) {
    let mut cx = rendering_app();
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("page")),
        style: page.size_full().bg(rgb(0xffffff)).style().clone(),
        interactivity: Default::default(),
        children: vec![child],
    });
    let window = cx
        .open_window(size(px(60.), px(60.)), |_, cx| {
            cx.new(|cx| Seat(cx.new(|_| ViewTree::new(root))))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let image = cx.capture_screenshot(window.into()).unwrap();
    let scale = image.width() / 60;
    let at = |x: u32, y: u32| image.get_pixel(x * scale, y * scale).0;
    (at(15, 15), at(45, 45))
}

fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 3),
        "{what}: {actual:?}, expected {expected:?}"
    );
}

/// A guest Svg with its own colour draws in it.
#[test]
fn a_guest_svg_paints_in_its_own_colour() {
    let (glyph, outside) = painted(div(), glyph(div().text_color(rgb(0xff0000))));
    assert_near(outside, WHITE, "the page around the glyph");
    assert_near(glyph, RED, "the glyph's middle");
}

/// A guest Svg with no colour of its own draws in the colour it inherits
/// (CSS `currentColor`), as text does, not nothing.
#[test]
fn a_guest_svg_with_no_colour_paints_in_the_one_it_inherits() {
    let (glyph, outside) = painted(div().text_color(rgb(0x0000ff)), glyph(div()));
    assert_near(outside, WHITE, "the page around the glyph");
    assert_near(glyph, BLUE, "the glyph's middle");
}

/// A guest's gradient background crosses the host's sanitizer and paints
/// as gpui draws one: red at its left, blue at its right, where the
/// sanitizer once dropped it and left the page white.
#[test]
fn a_guest_gradient_background_paints() {
    let gradient = gpui_kit::linear_gradient(
        90.,
        gpui_kit::linear_color_stop(rgb(0xff0000), 0.),
        gpui_kit::linear_color_stop(rgb(0x0000ff), 1.),
    );
    let mut frame = wire::Frame {
        root: Some(wire::Node::Container(view_wire::ContainerNode {
            id: Some(named_id("gradient")),
            style: div().size_full().bg(gradient).style().clone(),
            interactivity: Default::default(),
            children: vec![],
        })),
        ..wire::Frame::default()
    };
    wire::sanitize(&mut frame).unwrap();
    let (left, right) = painted(div(), frame.root.unwrap());
    assert!(
        left[0] > left[2] && right[2] > right[0],
        "red to blue, left to right: {left:?} then {right:?}"
    );
}
