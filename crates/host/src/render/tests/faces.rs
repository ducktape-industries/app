//! The faces a view's text is drawn in, through the Linux text system the
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
            transform: None,
        })),
        style: crate::render::test_style(div().flex().font_family(family).style().clone()),
    }
}

/// A view names the design crate's faces, which the app does not bundle:
/// a chat code line asks for its mono. Whatever kind of node asks, the app's
/// own mono draws it, the face the paragraph's runs are drawn in.
#[test]
fn a_view_naming_the_design_crates_mono_is_drawn_in_the_apps() {
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
        crate::render::draw_frame(window, cx);
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

/// A text whose style sets `spacing`, in a box as wide as its words.
fn spaced(key: &str, spacing: Option<f32>) -> wire::Node {
    let mut style = div();
    if let Some(spacing) = spacing {
        style = style.letter_spacing(px(spacing));
    }
    wire::Node::Text(view_wire::TextNode {
        id: Some(named_id(key)),
        style: crate::render::test_style(style.style().clone()),
        content: "abcdef".into(),
        transform: None,
    })
}

#[test]
fn a_views_letter_spacing_is_drawn_as_none() {
    let mut cx = HeadlessAppContext::new(Arc::new(crate::fonts::font_fallback::text_system()));
    cx.update(gpui_kit::init);
    let mut rich = paragraph("rich", None);
    let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut rich else {
        unreachable!()
    };
    *style = crate::render::test_style(div().letter_spacing(px(2.)).style().clone());
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
            spaced("plain", None),
            spaced("wide", Some(2.)),
            spaced("tight", Some(-0.5)),
            spaced("max", Some(f32::MAX)),
            spaced("min", Some(f32::MIN)),
            paragraph("rich-plain", None),
            rich,
        ],
    );
    let window = cx
        .open_window(size(px(400.), px(200.)), |_, cx| {
            cx.new(|_| ViewTree::new(root))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        crate::render::draw_frame(window, cx);
        let width = |key: &'static str| window.find(key).bounds().size.width;
        for key in ["wide", "tight", "max", "min"] {
            assert_eq!(width(key), width("plain"), "{key}");
        }
        assert_eq!(width("rich"), width("rich-plain"));
    })
    .unwrap();
}
