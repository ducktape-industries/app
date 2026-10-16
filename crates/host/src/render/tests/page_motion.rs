//! A view's page change under a finger: a deeper page slides in over the
//! one it was opened from, a shallower one is uncovered, and both land
//! at the still layout; a page at the same depth, Reduce Motion and a
//! pointer window slide nothing.
use super::*;
use std::time::Duration;

const WIDTH: f32 = 390.;

/// A view showing the page `key`, `depth` over its stack's root.
fn page(key: &str, depth: u32) -> wire::Node {
    let page = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: crate::render::test_style(sized_style(Some(fill()), Some(fill()))),
        interactivity: Some(Box::new(wire::Interactivity {
            page: Some(depth),
            ..Default::default()
        })),
        children: vec![text(&format!("{key}-words"), key)],
    });
    sized("view", page, Some(fill()), Some(fill()))
}

fn open(
    cx: &mut gpui_kit::TestAppContext,
    first: wire::Node,
) -> (Entity<ViewTree>, gpui_kit::VisualTestContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(WIDTH), px(844.)), |_, _| ViewTree::new(first));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    (tree, native)
}

fn show(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext, root: wire::Node) {
    tree.update(native, |tree, cx| tree.replace(root, &[], cx));
    native.update(|window, cx| window.render_frame(cx));
}

/// `by` later, drawn.
fn later(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext, by: Duration) {
    native.executor().advance_clock(by);
    tree.update(native, |_, cx| cx.notify());
    native.update(|window, cx| window.render_frame(cx));
}

fn left(native: &mut gpui_kit::VisualTestContext, key: &str) -> f32 {
    native.update(|window, _| {
        f32::from(
            window
                .find(named_id(key).to_gpui().unwrap())
                .bounds()
                .left(),
        )
    })
}

fn drawn(native: &mut gpui_kit::VisualTestContext, key: &str) -> bool {
    native.update(|window, _| window.try_find(named_id(key).to_gpui().unwrap()).is_some())
}

fn moving(tree: &Entity<ViewTree>, native: &gpui_kit::VisualTestContext) -> bool {
    tree.read_with(native, |tree, _| tree.page_moving())
}

/// A push slides the new page in from the right edge over the old, which
/// moves left; a pop slides the top page off to the right over the one
/// under it, which comes back from the left. Each ends at the still
/// layout, with the old page gone.
#[gpui_kit::test]
fn a_push_and_a_pop_slide_and_land_still(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native) = open(cx, page("rooms", 0));
    show(&tree, &mut native, page("room", 1));
    assert!(moving(&tree, &native));
    assert_eq!(
        left(&mut native, "room"),
        WIDTH,
        "the new page starts off the right edge"
    );
    assert_eq!(left(&mut native, "rooms"), 0.);
    later(&tree, &mut native, Duration::from_millis(100));
    let (over, under) = (left(&mut native, "room"), left(&mut native, "rooms"));
    assert!(0. < over && over < WIDTH, "{over}");
    assert!((-WIDTH * 0.3..0.).contains(&under), "{under}");
    later(&tree, &mut native, crate::motion::PUSH.duration);
    assert!(!moving(&tree, &native));
    assert_eq!(left(&mut native, "room"), 0.);
    assert!(!drawn(&mut native, "rooms"), "the old page is gone");

    show(&tree, &mut native, page("rooms", 0));
    assert!(moving(&tree, &native));
    assert_eq!(
        left(&mut native, "room"),
        0.,
        "the popped page starts in place"
    );
    later(&tree, &mut native, Duration::from_millis(100));
    let (over, under) = (left(&mut native, "room"), left(&mut native, "rooms"));
    assert!(0. < over && over < WIDTH, "{over}");
    assert!(-WIDTH * 0.3 < under && under < 0., "{under}");
    later(&tree, &mut native, crate::motion::POP.duration);
    assert!(!moving(&tree, &native));
    assert_eq!(left(&mut native, "rooms"), 0.);
    assert!(!drawn(&mut native, "room"));
    crate::contract::testing::touch_first_for_test(false);
}

/// Under Reduce Motion nothing slides: the old page fades over the new
/// one in place, then is gone.
#[gpui_kit::test]
fn reduce_motion_slides_nothing(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    cx.update(|cx| cx.set_reduce_motion(true));
    let (tree, mut native) = open(cx, page("rooms", 0));
    show(&tree, &mut native, page("room", 1));
    later(&tree, &mut native, Duration::from_millis(50));
    assert!(moving(&tree, &native));
    assert_eq!(left(&mut native, "room"), 0.);
    assert_eq!(left(&mut native, "rooms"), 0.);
    later(&tree, &mut native, crate::motion::FADE.duration);
    assert!(!moving(&tree, &native));
    assert!(!drawn(&mut native, "rooms"));
    crate::contract::testing::touch_first_for_test(false);
}

/// A page at the same depth (a tab of the root, a page put in place of
/// another) is put in place at once, as the phone switches a tab.
#[gpui_kit::test]
fn a_page_at_the_same_depth_does_not_slide(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native) = open(cx, page("overview", 0));
    show(&tree, &mut native, page("blocks", 0));
    assert!(!moving(&tree, &native));
    assert_eq!(left(&mut native, "blocks"), 0.);
    assert!(!drawn(&mut native, "overview"));
    crate::contract::testing::touch_first_for_test(false);
}

/// A pointer window never moves a page.
#[gpui_kit::test]
fn a_pointer_window_does_not_slide(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = open(cx, page("rooms", 0));
    show(&tree, &mut native, page("room", 1));
    assert!(!moving(&tree, &native));
    assert_eq!(left(&mut native, "room"), 0.);
    assert!(!drawn(&mut native, "rooms"));
}
