//! A view's page change under a finger: a deeper page slides in over the
//! one it was opened from, a shallower one is uncovered, and both land
//! at the still layout; a page at the same depth, Reduce Motion and a
//! pointer window slide nothing. The page going keeps its lists where they
//! were left and takes no press.
use super::*;
use std::time::Duration;

const WIDTH: f32 = 390.;

/// A view showing the page `key`, `depth` over its stack's root.
fn page(key: &str, depth: u32) -> wire::Node {
    page_of(key, depth, vec![text(&format!("{key}-words"), key)])
}

/// A view showing the page `key`, `depth` over its stack's root, made of
/// `children`.
fn page_of(key: &str, depth: u32, children: Vec<wire::Node>) -> wire::Node {
    let page = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: crate::render::test_style(sized_style(Some(fill()), Some(fill()))),
        interactivity: Some(Box::new(wire::Interactivity {
            page: Some(depth),
            ..Default::default()
        })),
        children,
    });
    sized("view", page, Some(fill()), Some(fill()))
}

/// A list of three rows on the page `key`, filed under its path.
fn rows(key: &str) -> wire::Node {
    let row = |id: u64| {
        container_with_style(
            &format!("row-{id}"),
            div().h(px(600.)).w_full().style().clone(),
            [],
        )
    };
    wire::Node::List {
        id: named_id("rows"),
        path: vec![named_id("view"), named_id(key), named_id("rows")],
        item_count: 3,
        alignment: wire::ListAlignment::Top,
        overdraw: 32.,
        sizing: wire::ListSizingBehavior::Auto,
        following_tail: false,
        revision: 0,
        commands: Vec::new(),
        request_handler: 41,
        scroll_handler: None,
        range_start: 0,
        style: crate::render::test_style(sized_style(Some(fill()), Some(fill()))),
        interactivity: Default::default(),
        children: (0..3).map(row).collect(),
    }
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
    later(&tree, &mut native, crate::motion::idiom().push.duration);
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
    later(&tree, &mut native, crate::motion::idiom().push.duration);
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

/// A list the page going was scrolled in moves away where it was left,
/// not back at its top.
#[gpui_kit::test]
fn the_page_going_keeps_its_list_where_it_was_left(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    let (tree, mut native) = open(cx, page_of("rooms", 0, vec![rows("rooms")]));
    let scrolled = gpui_kit::ListOffset {
        item_ix: 1,
        offset_in_item: px(30.),
    };
    tree.update(&mut native, |tree, _| {
        let list = tree
            .variable_lists
            .values()
            .next()
            .expect("the list is drawn");
        list.state.scroll_to(scrolled);
    });
    show(&tree, &mut native, page("room", 1));
    let leaving = tree
        .read_with(&native, |tree, _| tree.leaving_page())
        .expect("the push moves");
    let top = leaving.read_with(&native, |tree, _| {
        let list = tree
            .variable_lists
            .values()
            .next()
            .expect("the list went with it");
        list.state.logical_scroll_top()
    });
    assert_eq!((top.item_ix, top.offset_in_item), (1, px(30.)));
    assert!(
        tree.read_with(&native, |tree, _| tree.variable_lists.is_empty()),
        "the page coming has no list of its own"
    );
    crate::contract::testing::touch_first_for_test(false);
}

/// Over a pop the page going is on top: a press on it reaches nothing,
/// not the control under it on the page coming back; once it has gone the
/// control is pressed.
#[gpui_kit::test]
fn a_press_on_the_page_going_reaches_nothing(cx: &mut gpui_kit::TestAppContext) {
    crate::contract::testing::touch_first_for_test(true);
    // a row the width of the page: at a pop's start the page coming back
    // sits 30 % to the left, and the row still spans the press there
    let mut control = container_with_style("open", div().w_full().h(px(24.)).style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut control {
        interactivity.get_or_insert_default().on_click = Some(9);
    }
    let (tree, mut native) = open(cx, page("room", 1));
    let (events, _subscription) = emitted(&tree, &mut native);
    let press = |native: &mut gpui_kit::VisualTestContext| {
        let at = point(px(WIDTH / 2.), px(10.));
        native.simulate_mouse_move(at, None, Default::default());
        native.simulate_mouse_down(at, MouseButton::Left, Default::default());
        native.simulate_mouse_up(at, MouseButton::Left, Default::default());
        native.update(|window, cx| window.render_frame(cx));
    };
    let clicks = |events: &RefCell<Vec<wire::Event>>| {
        events
            .borrow()
            .iter()
            .filter(|event| matches!(event, wire::Event::Click { handler: 9, .. }))
            .count()
    };
    show(&tree, &mut native, page_of("rooms", 0, vec![control]));
    assert!(moving(&tree, &native));
    press(&mut native);
    assert_eq!(clicks(&events), 0, "the press went through the page going");
    later(&tree, &mut native, crate::motion::idiom().push.duration);
    assert!(!moving(&tree, &native));
    press(&mut native);
    assert_eq!(
        clicks(&events),
        1,
        "the control under it is where the press was"
    );
    crate::contract::testing::touch_first_for_test(false);
}
