use super::*;
use crate::send_trace::tests::capture;

const KEY: crate::perf::Key = crate::perf::Key::View {
    program: "test-view",
    instance: 7,
};

fn row(opacity: Option<f32>) -> wire::Node {
    let style = gpui_kit::StyleRefinement {
        opacity,
        ..Default::default()
    };
    container_with_style("row", style, [text("words", "private message text")])
}

#[gpui_kit::test]
fn trace_records_painted_revisions_and_opacity_without_message_text(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let ((), log) = capture(|| {
        let window = cx.open_window(size(px(200.), px(120.)), |_, _| {
            let mut tree = ViewTree::new(row(Some(0.5))).with_perf_key(KEY);
            tree.send_trace = true;
            tree.set_trace_frame(7);
            tree
        });
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
        tree.update(&mut native, |tree, cx| {
            tree.set_trace_frame(8);
            tree.replace(row(None), &[], cx);
        });
        native.update(crate::render::draw_frame);
    });
    assert!(log.lines().any(|line| {
        line.contains("node_painted")
            && line.contains("row")
            && line.contains("revision=7")
            && line.contains("style_opacity=Some(0.5)")
    }));
    assert!(log.lines().any(|line| {
        line.contains("node_painted")
            && line.contains("row")
            && line.contains("revision=8")
            && line.contains("style_opacity=None")
    }));
    assert!(!log.contains("private message text"));
}

#[gpui_kit::test]
fn a_tree_with_capture_disabled_emits_no_paint_trace(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let ((), log) = capture(|| {
        let window = cx.open_window(size(px(200.), px(120.)), |_, _| {
            let mut tree = ViewTree::new(row(None)).with_perf_key(KEY);
            tree.send_trace = false;
            tree
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
    });
    assert!(!log.contains("node_painted"));
}

#[gpui_kit::test]
fn nodes_outside_the_paint_clip_are_not_reported_as_shown(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let outside = div()
        .absolute()
        .top(px(240.))
        .w(px(20.))
        .h(px(20.))
        .style()
        .clone();
    let clip = div().size_full().overflow_hidden().style().clone();
    let root = container_with_style(
        "root",
        clip,
        [container_with_style("offscreen", outside, [])],
    );
    let ((), log) = capture(|| {
        let window = cx.open_window(size(px(200.), px(120.)), |_, _| {
            let mut tree = ViewTree::new(root).with_perf_key(KEY);
            tree.send_trace = true;
            tree
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(crate::render::draw_frame);
    });
    assert!(log.contains("node_painted"));
    assert!(!log.contains("offscreen"));
}
