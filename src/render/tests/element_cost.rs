//! A host element costs what its node sets (briefs/perf-root, H2): the
//! canvas that brings a scroller to a node is drawn only on a node that
//! claims.
use super::*;

/// A scroller of ten rows, each with an id, the first `first` px tall;
/// `claim` is the row that claims.
fn rows(claim: Option<usize>, first: f32) -> wire::Node {
    let rows = (0..10).map(|n| {
        let height = if n == 0 { first } else { 40. };
        let mut row = container_with_style(
            &format!("row-{n}"),
            div().h(px(height)).flex_shrink_0().style().clone(),
            [text(&format!("label-{n}"), "row")],
        );
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut row {
            let interactivity = interactivity.get_or_insert_default();
            interactivity.role = Some(gpui_kit::Role::ListBoxOption);
            interactivity.aria.active_descendant = claim == Some(n);
        }
        row
    });
    let mut style = div()
        .flex()
        .flex_col()
        .w(px(200.))
        .h(px(100.))
        .style()
        .clone();
    style.overflow.y = Some(gpui_kit::Overflow::Scroll);
    container_with_style("list", style, rows)
}

/// A node that does not claim has no canvas over it: nothing measures it,
/// so a frame that moves every row is drawn once. The canvas is on the row
/// that claims, and its one move of the scroller asks for the one render
/// that draws the row there; a claim that stays asks for nothing.
#[gpui_kit::test]
fn only_a_claim_asks_for_a_second_render(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(rows(None, 40.))))
    });
    let seat = window.root(cx).unwrap();
    let tree = seat.read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the renders `frame` costs, over five draws of the desk, and where
    // they leave the scroller
    let mut renders = |frame: Option<wire::Node>| {
        let before = tree.read_with(&native, |tree, _| tree.renders);
        if let Some(frame) = frame {
            tree.update(&mut native, |tree, cx| tree.replace(frame, &[], cx));
        }
        for _ in 0..5 {
            native.update(|window, cx| {
                // the desk draws its seats again: `render_frame` would refresh
                seat.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            });
            native.run_until_parked();
        }
        tree.read_with(&native, |tree, _| {
            let offset = tree.scrolls[&vec![named_id("list")]].offset().y;
            (tree.renders - before, f32::from(offset))
        })
    };
    assert_eq!(
        renders(None),
        (0, 0.),
        "at rest the open tree is not drawn again"
    );
    assert_eq!(
        renders(Some(rows(None, 60.))),
        (1, 0.),
        "every row below the first moved, and no row claims: one render"
    );
    assert_eq!(
        renders(Some(rows(Some(7), 60.))),
        (2, -240.),
        "row 7 (300..340) claims below the fold: its frame, and the one scrolled to it"
    );
    assert_eq!(
        renders(Some(rows(Some(7), 40.))),
        (1, -240.),
        "the claim stays where the rows move: one render, and the offset is left alone"
    );
}
