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
    native.update(|window, cx| window.render_frame(cx));
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
    native.update(|window, cx| window.render_frame(cx));
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
    native.update(|window, cx| window.render_frame(cx));
    hold(&mut native, point(px(60.), px(40.)));
    assert_eq!(
        heard(&events),
        ["click 71"],
        "unclaimed, gpui's hold ends as a tap"
    );
}
