use super::*;

/// A 1px divider between two panes, the right one clickable.
fn split() -> wire::Node {
    let mut row = div().flex().flex_row().size_full();
    let row = row.style().clone();
    let mut right = container_with_style("right", div().flex_1().h_full().style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut right {
        interactivity.on_click = Some(9);
    }
    container_with_style(
        "split",
        row,
        [
            container_with_style("left", div().w(px(100.)).h_full().style().clone(), []),
            wire::Node::ResizeHandle {
                id: named_id("divider"),
                style: Default::default(),
                on_press: None,
                on_release: None,
                on_drag: Some(5),
                cursor: Some(wire::mouse::Cursor::ResizingHorizontally),
                content: Box::new(container_with_style(
                    "line",
                    div().w(px(1.)).h_full().style().clone(),
                    [],
                )),
            },
            right,
        ],
    )
}

/// Presses at `x`, drags 30px right and lets go: how far the divider moved,
/// and whether the pane under the press heard a click.
fn drag_from(x: f32, cx: &mut gpui_kit::TestAppContext) -> (f64, bool) {
    let window = cx.open_window(size(px(300.), px(80.)), |_, _| ViewTree::new(split()));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone());
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    let at = point(px(x), px(40.));
    native.simulate_mouse_move(at, None, Default::default());
    native.simulate_mouse_down(at, MouseButton::Left, Default::default());
    native.simulate_mouse_up(at, MouseButton::Left, Default::default());
    native.simulate_mouse_down(at, MouseButton::Left, Default::default());
    let to = point(px(x + 30.), px(40.));
    native.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
    native.simulate_mouse_up(to, MouseButton::Left, Default::default());
    let events = events.borrow();
    let moved = events
        .iter()
        .filter_map(|event| match event {
            wire::Event::Drag { handler: 5, dx, .. } => Some(*dx),
            _ => None,
        })
        .sum();
    let clicked = events
        .iter()
        .any(|event| matches!(event, wire::Event::Click { handler: 9, .. }));
    (moved, clicked)
}

#[gpui_kit::test]
fn a_divider_grips_a_near_miss_on_either_side(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    assert_eq!(drag_from(100.5, cx), (30., false), "on the line");
    assert_eq!(drag_from(96., cx), (30., false), "just left of it");
    assert_eq!(
        drag_from(105., cx),
        (30., false),
        "just right, over the pane: the press is the divider's"
    );
    assert_eq!(drag_from(108., cx), (0., true), "past the grip: the pane's");
}

#[test]
fn a_grip_reaches_along_the_axis_it_sizes() {
    let grab = crate::shell::GRAB;
    assert_eq!(
        super::sensors::grip_reach(CursorStyle::ResizeLeftRight),
        (grab, 0.)
    );
    assert_eq!(
        super::sensors::grip_reach(CursorStyle::ResizeUpDown),
        (0., grab)
    );
    assert_eq!(super::sensors::grip_reach(CursorStyle::Arrow), (grab, grab));
}
