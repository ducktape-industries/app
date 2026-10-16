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
    })
}

/// A view's letter spacing crosses in its text's style and the host draws
/// it: each of the six letters gains the spacing after it, a negative one
/// tightening the line, and a rich paragraph inherits it from its box as
/// a plain text does.
#[test]
fn a_texts_letter_spacing_widens_or_tightens_its_line() {
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
        window.render_frame(cx);
        let width = |key: &'static str| window.find(key).bounds().size.width;
        assert_eq!(width("wide") - width("plain"), px(12.), "6 x 2 px");
        assert_eq!(width("plain") - width("tight"), px(3.), "6 x 0.5 px");
        // `paragraph`'s words are eight letters
        assert_eq!(width("rich") - width("rich-plain"), px(16.), "8 x 2 px");
    })
    .unwrap();
}

/// A spacing at either end of `f32` (what the host's sanitizer leaves of
/// an infinite one) puts the line's glyphs past any finite place after two
/// letters. A text, a wrapping one, a truncated one and a rich paragraph
/// at such a spacing lay out and paint, frame after frame.
#[test]
fn a_letter_spacing_at_the_end_of_f32_lays_out_and_paints() {
    let mut cx = HeadlessAppContext::new(Arc::new(crate::fonts::font_fallback::text_system()));
    cx.update(gpui_kit::init);
    let text = |key: &str, mut style: gpui_kit::Div| {
        wire::Node::Text(view_wire::TextNode {
            id: Some(named_id(key)),
            style: crate::render::test_style(style.style().clone()),
            content: "a line long enough to wrap in its narrow box".into(),
        })
    };
    let mut children = Vec::new();
    for (name, spacing) in [("max", f32::MAX), ("min", f32::MIN)] {
        let spaced = || div().letter_spacing(px(spacing));
        children.push(text(&format!("{name}-line"), spaced()));
        children.push(text(&format!("{name}-wrapped"), spaced().w(px(60.))));
        children.push(text(
            &format!("{name}-truncated"),
            spaced().w(px(60.)).truncate(),
        ));
        let mut rich = paragraph(&format!("{name}-rich"), None);
        let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut rich else {
            unreachable!()
        };
        *style = crate::render::test_style(spaced().style().clone());
        children.push(rich);
    }
    let root = container_with_style(
        "page",
        div()
            .flex()
            .flex_col()
            .items_start()
            .size_full()
            .style()
            .clone(),
        children,
    );
    let window = cx
        .open_window(size(px(400.), px(400.)), |_, cx| {
            cx.new(|_| ViewTree::new(root))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        for _ in 0..2 {
            window.refresh();
            window.render_frame(cx);
        }
        // laid out: each has a place, and the page a finite one for it
        for name in ["max", "min"] {
            for kind in ["line", "wrapped", "truncated", "rich"] {
                let bounds = window.find(format!("{name}-{kind}")).bounds();
                let place = [bounds.origin.x, bounds.origin.y];
                let size = [bounds.size.width, bounds.size.height];
                assert!(
                    place
                        .into_iter()
                        .chain(size)
                        .all(|v| f32::from(v).is_finite()),
                    "{name}-{kind}: {bounds:?}"
                );
            }
        }
    })
    .unwrap();
}
