//! The faces a guest's text is drawn in, through the Linux text system the
//! app ships with its bundled faces and none of the machine's.
use super::*;
use gpui_kit::HeadlessAppContext;

const WORDS: &str = "iiiiiiii";

/// A paragraph whose own style names `family`, in a box as wide as its words.
fn paragraph(key: &str, family: Option<&'static str>) -> wire::Node {
    let mut style = div();
    if let Some(family) = family {
        style = style.font_family(family);
    }
    container(
        key,
        [wire::Node::RichText {
            id: None,
            style: crate::render::test_style(style.style().clone()),
            text: WORDS.into(),
            runs: wire::RichTextRuns::Highlights(Vec::new()),
            font_family_overrides: Vec::new(),
            clickable_ranges: Vec::new(),
            on_click: None,
            on_hover: None,
            tooltip: None,
        }],
    )
}

/// A sensor whose style names `family`, around a text that names none.
fn sensed(key: &str, family: &'static str) -> wire::Node {
    wire::Node::Sensor {
        id: named_id("sensor"),
        on_bounds: None,
        child: Box::new(wire::Node::Text(view_wire::TextNode {
            id: Some(named_id(key)),
            style: crate::render::plain_style(),
            content: WORDS.into(),
        })),
        style: crate::render::test_style(div().flex().font_family(family).style().clone()),
    }
}

/// A guest names the design crate's faces, which the app does not bundle:
/// a chat code line asks for its mono. Whatever kind of node asks, the app's
/// own mono draws it, the face the paragraph's runs are drawn in.
#[test]
fn a_guest_naming_the_design_crates_mono_is_drawn_in_the_apps() {
    let mut cx = HeadlessAppContext::new(Arc::new(crate::fonts::font_fallback::text_system()));
    cx.update(gpui_kit::init);
    let root = container_with_style(
        "page",
        div()
            .flex()
            .flex_col()
            .items_start()
            .size_full()
            .style()
            .clone(),
        [
            paragraph("design-mono", Some(design::fonts::FAMILY_MONO)),
            paragraph("app-mono", Some(crate::fonts::FAMILY_MONO)),
            paragraph("sans", None),
            sensed("sensed-design-mono", design::fonts::FAMILY_MONO),
        ],
    );
    let window = cx
        .open_window(size(px(400.), px(200.)), |_, cx| {
            cx.new(|_| ViewTree::new(root))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let width = |key: &'static str| window.find(key).bounds().size.width;
        assert_ne!(
            width("app-mono"),
            width("sans"),
            "the words must tell the mono from the sans"
        );
        assert_eq!(width("design-mono"), width("app-mono"), "a paragraph");
        assert_eq!(width("sensed-design-mono"), width("app-mono"), "a sensor");
    })
    .unwrap();
}
