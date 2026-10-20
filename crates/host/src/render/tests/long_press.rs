//! A finger held on a node that hears aux clicks: gpui-pre recognizes the
//! long press, the host hands it to the node as its aux click.
use super::*;

/// `node` hearing clicks on `click` and aux clicks on `aux`.
fn hearing(mut node: wire::Node, click: Option<u32>, aux: Option<u32>) -> wire::Node {
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.on_click = click;
        interactivity.on_aux_click = aux;
    }
    node
}

/// A finger down at `at`, held past gpui's long press, then lifted.
fn hold(native: &mut gpui_kit::VisualTestContext, at: gpui_kit::Point<gpui_kit::Pixels>) {
    let touch = |phase| gpui_kit::TouchEvent {
        id: gpui_kit::TouchId(1),
        phase,
        position: at,
        predicted_position: None,
        force: None,
    };
    native.update(|window, cx| {
        let started = touch(gpui_kit::TouchPhase::Started).to_platform_input();
        let _ = window.dispatch_event(started, cx);
    });
    let held = gpui_kit::GestureTuning::default().long_press_duration;
    native
        .executor()
        .advance_clock(held + std::time::Duration::from_millis(1));
    native.run_until_parked();
    native.update(|window, cx| {
        let ended = touch(gpui_kit::TouchPhase::Ended).to_platform_input();
        let _ = window.dispatch_event(ended, cx);
    });
    native.run_until_parked();
}

/// What the view heard: `aux N` for a long press's aux click, `click N`
/// for a click, nothing else.
fn heard(events: &RefCell<Vec<wire::Event>>) -> Vec<String> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::AuxClick {
                handler,
                event:
                    wire::click::Click::Touch {
                        long_press: true, ..
                    },
            } => Some(format!("aux {handler}")),
            wire::Event::AuxClick { handler, .. } => Some(format!("other aux {handler}")),
            wire::Event::Click { handler, .. } => Some(format!("click {handler}")),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn a_long_press_is_the_nodes_aux_click_and_no_click(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let row = container_with_style("row", div().size_full().style().clone(), []);
    let root = hearing(row, Some(71), Some(72));
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    hold(&mut native, point(px(60.), px(40.)));
    assert_eq!(
        heard(&events),
        ["aux 72"],
        "the hold is the aux click alone"
    );
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_activation().is_some(), "a hold is a gesture");
    });
}

#[gpui_kit::test]
fn a_long_press_on_a_node_inside_is_that_nodes_alone(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let chip_style = sized_style(Some(fixed(40.)), Some(fixed(20.)));
    let chip = hearing(container_with_style("chip", chip_style, []), None, Some(73));
    let row = container_with_style("row", div().size_full().style().clone(), [chip]);
    let root = hearing(row, None, Some(72));
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    hold(&mut native, point(px(10.), px(10.)));
    assert_eq!(heard(&events), ["aux 73"], "the chip takes it, not its row");
    events.borrow_mut().clear();
    hold(&mut native, point(px(100.), px(60.)));
    assert_eq!(heard(&events), ["aux 72"], "beside the chip, the row's");
}

#[gpui_kit::test]
fn a_long_press_on_a_node_that_hears_only_clicks_is_its_click(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let button = container_with_style("button", div().size_full().style().clone(), []);
    let root = hearing(button, Some(71), None);
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    hold(&mut native, point(px(60.), px(40.)));
    assert_eq!(
        heard(&events),
        ["click 71"],
        "unclaimed, gpui's hold ends as a tap"
    );
}

fn following(mut node: wire::Node, moves: Option<u32>, ups: Option<u32>) -> wire::Node {
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.on_mouse_move = moves;
        interactivity.on_mouse_up = ups;
    }
    node
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
    });
    native.run_until_parked();
}

fn held_past_long_press(
    native: &mut gpui_kit::VisualTestContext,
    at: gpui_kit::Point<gpui_kit::Pixels>,
) {
    finger(native, gpui_kit::TouchPhase::Started, at);
    let held = gpui_kit::GestureTuning::default().long_press_duration;
    native
        .executor()
        .advance_clock(held + std::time::Duration::from_millis(1));
    native.run_until_parked();
}

fn pointer(
    events: &RefCell<Vec<wire::Event>>,
) -> Vec<(&'static str, u32, gpui_kit::Point<gpui_kit::Pixels>)> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::AuxClick {
                handler,
                event: wire::click::Click::Touch { position, .. },
            } => Some(("aux", *handler, *position)),
            wire::Event::MouseMove { handler, event, .. } => {
                Some(("move", *handler, event.position))
            }
            wire::Event::MouseUp { handler, event, .. } => Some(("up", *handler, event.position)),
            wire::Event::Click { handler, .. } => Some(("click", *handler, point(px(0.), px(0.)))),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn a_held_finger_moves_and_lifts_on_the_node_it_long_pressed(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let list_style = sized_style(Some(fixed(120.)), Some(fixed(40.)));
    let list = hearing(container_with_style("list", list_style, []), None, Some(72));
    let root = container_with_style(
        "root",
        div().size_full().style().clone(),
        [following(list, Some(74), Some(75))],
    );
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    held_past_long_press(&mut native, point(px(20.), px(10.)));
    finger(
        &mut native,
        gpui_kit::TouchPhase::Moved,
        point(px(30.), px(30.)),
    );
    finger(
        &mut native,
        gpui_kit::TouchPhase::Moved,
        point(px(40.), px(70.)),
    );
    finger(
        &mut native,
        gpui_kit::TouchPhase::Ended,
        point(px(40.), px(70.)),
    );
    assert_eq!(
        pointer(&events),
        [
            ("aux", 72, point(px(20.), px(10.))),
            ("move", 74, point(px(30.), px(30.))),
            ("move", 74, point(px(40.), px(70.))),
            ("up", 75, point(px(40.), px(70.))),
        ],
        "past its own bounds too, the finger stays the pressed node's"
    );
    events.borrow_mut().clear();
    finger(
        &mut native,
        gpui_kit::TouchPhase::Moved,
        point(px(50.), px(20.)),
    );
    assert_eq!(pointer(&events), [], "a lifted finger is nobody's");
}

#[gpui_kit::test]
fn a_cancelled_long_press_lets_go_where_it_began(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let row = hearing(
        container_with_style("row", div().size_full().style().clone(), []),
        None,
        Some(72),
    );
    let root = following(row, Some(74), Some(75));
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    held_past_long_press(&mut native, point(px(60.), px(40.)));
    finger(
        &mut native,
        gpui_kit::TouchPhase::Moved,
        point(px(70.), px(50.)),
    );
    finger(
        &mut native,
        gpui_kit::TouchPhase::Cancelled,
        point(px(70.), px(50.)),
    );
    assert_eq!(
        pointer(&events),
        [
            ("aux", 72, point(px(60.), px(40.))),
            ("move", 74, point(px(70.), px(50.))),
            ("up", 75, point(px(60.), px(40.))),
        ]
    );
}

#[gpui_kit::test]
fn a_long_pressed_node_with_no_move_or_up_route_hears_only_its_aux(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let row = container_with_style("row", div().size_full().style().clone(), []);
    let root = hearing(row, None, Some(72));
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    held_past_long_press(&mut native, point(px(60.), px(40.)));
    finger(
        &mut native,
        gpui_kit::TouchPhase::Moved,
        point(px(70.), px(50.)),
    );
    finger(
        &mut native,
        gpui_kit::TouchPhase::Ended,
        point(px(70.), px(50.)),
    );
    assert_eq!(pointer(&events), [("aux", 72, point(px(60.), px(40.)))]);
}
