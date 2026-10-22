//! A finger's pan over a block that scrolls sideways, inside a list that
//! scrolls down, on a touch-first device.
use super::*;

const WIDE: u32 = 0x00ff00;

fn wide_in_a_list() -> wire::Node {
    let mut wide = div().w(px(600.)).h(px(40.)).flex_none();
    wide.style().background = Some(rgb(WIDE).into());
    let mut table = div().w_full().h(px(40.)).flex_none().flex();
    table.style().overflow.x = Some(gpui_kit::Overflow::Scroll);
    let mut list = div().w_full().h(px(100.)).flex().flex_col();
    list.style().overflow.y = Some(gpui_kit::Overflow::Scroll);
    let row = |at: usize| {
        let style = div().w_full().h(px(40.)).flex_none().style().clone();
        container_with_style(&format!("row-{at}"), style, [])
    };
    let block = container_with_style(
        "table",
        table.style().clone(),
        [container_with_style("wide", wide.style().clone(), [])],
    );
    container_with_style(
        "list",
        list.style().clone(),
        std::iter::once(row(0))
            .chain([block])
            .chain((1..6).map(row)),
    )
}

fn wide_at(native: &mut gpui_kit::VisualTestContext) -> (f32, f32) {
    native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        let quad = window
            .painted_quads()
            .into_iter()
            .find(|quad| quad.background.as_solid() == Some(rgb(WIDE).into()))
            .expect("the wide block paints");
        (quad.bounds.origin.x.0, quad.bounds.origin.y.0)
    })
}

fn finger(
    native: &mut gpui_kit::VisualTestContext,
    phase: gpui_kit::TouchPhase,
    at: gpui_kit::Point<gpui_kit::Pixels>,
) {
    let touch = gpui_kit::TouchEvent {
        id: gpui_kit::TouchId(1),
        phase,
        position: at,
        predicted_position: None,
        force: None,
    };
    native.update(|window, cx| {
        let _ = window.dispatch_event(touch.to_platform_input(), cx);
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
}

fn panned(path: &[(f32, f32)], cx: &mut gpui_kit::TestAppContext) -> ((f32, f32), (f32, f32)) {
    crate::contract::testing::touch_first_for_test(true);
    cx.update(gpui_kit::init);
    let root = wide_in_a_list();
    let window = cx.open_window(size(px(200.), px(100.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let before = wide_at(&mut native);
    let at = |(x, y): (f32, f32)| point(px(x), px(y));
    let (first, rest) = path.split_first().expect("a path");
    finger(&mut native, gpui_kit::TouchPhase::Started, at(*first));
    for step in rest {
        finger(&mut native, gpui_kit::TouchPhase::Moved, at(*step));
    }
    let last = *path.last().expect("a path");
    finger(&mut native, gpui_kit::TouchPhase::Ended, at(last));
    let after = wide_at(&mut native);
    crate::contract::testing::touch_first_for_test(false);
    (before, after)
}

#[gpui_kit::test]
fn a_mostly_upright_pan_over_a_wide_block_moves_the_list_and_not_the_block(
    cx: &mut gpui_kit::TestAppContext,
) {
    let path = [(60., 78.), (63., 63.), (66., 48.), (69., 33.)];
    let (before, after) = panned(&path, cx);
    assert!(
        after.1 < before.1,
        "the list moved up: {before:?} -> {after:?}"
    );
    assert_eq!(after.0, before.0, "the block kept its place sideways");
}

#[gpui_kit::test]
fn a_mostly_sideways_pan_over_a_wide_block_moves_the_block_and_not_the_list(
    cx: &mut gpui_kit::TestAppContext,
) {
    let path = [(150., 60.), (130., 63.), (110., 66.), (90., 69.)];
    let (before, after) = panned(&path, cx);
    assert!(
        after.0 < before.0,
        "the block moved sideways: {before:?} -> {after:?}"
    );
    assert_eq!(after.1, before.1, "the list kept its place");
}
