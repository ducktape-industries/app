use super::*;

/// A 1px divider between two panes, the right one clickable.
fn split() -> wire::Node {
    let mut row = div().flex().flex_row().size_full();
    let row = row.style().clone();
    let mut right = container_with_style("right", div().flex_1().h_full().style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut right {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.on_click = Some(9);
    }
    container_with_style(
        "split",
        row,
        [
            container_with_style("left", div().w(px(100.)).h_full().style().clone(), []),
            wire::Node::ResizeHandle {
                id: named_id("divider"),
                style: crate::render::plain_style(),
                interactivity: Default::default(),
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
    let (events, _subscription) = emitted(&tree, &mut native);
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

/// A divider styled by its group, as any node is: the split it sits in is
/// the group, and a pointer over the left pane lights the divider. Its
/// hover, active and group styles go through the one apply every node
/// takes, where the divider's own used to be dropped.
#[gpui_kit::test]
fn a_divider_wears_its_group_hover(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    // eight bits a channel, as a colour crosses the wire
    let lit: gpui_kit::Hsla = gpui_kit::rgb(0x4073bf).into();
    let mut root = split();
    let wire::Node::Container(view_wire::ContainerNode {
        interactivity,
        children,
        ..
    }) = &mut root
    else {
        unreachable!()
    };
    let interactivity = interactivity.get_or_insert_default();
    interactivity.group = Some("split".into());
    let wire::Node::ResizeHandle { interactivity, .. } = &mut children[1] else {
        unreachable!()
    };
    let mut style = div().bg(lit);
    let interactivity = interactivity.get_or_insert_default();
    interactivity.group_hover = Some(wire::GroupRefinement {
        group: "split".into(),
        style: crate::render::test_style(style.style().clone()),
    });
    let window = cx.open_window(size(px(300.), px(80.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let lit_quads = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            window
                .painted_quads()
                .iter()
                .filter(|quad| quad.background == lit.into())
                .count()
        })
    };
    native.simulate_mouse_move(point(px(400.), px(200.)), None, Default::default());
    assert_eq!(
        lit_quads(&mut native),
        0,
        "the pointer is out of the window"
    );
    native.simulate_mouse_move(point(px(50.), px(40.)), None, Default::default());
    assert_eq!(lit_quads(&mut native), 1, "the pointer is in the split");
}

/// A view that asks for the keys on a divider by its path hears the host say
/// no: the divider takes no focus, and the id-less line it draws sits on the
/// divider's path without being the node that path names.
#[gpui_kit::test]
fn a_focus_on_a_divider_is_refused(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut root = split();
    if let wire::Node::Container(view_wire::ContainerNode { children, .. }) = &mut root
        && let wire::Node::ResizeHandle { content, .. } = &mut children[1]
        && let wire::Node::Container(view_wire::ContainerNode { id, .. }) = &mut **content
    {
        *id = None;
    }
    let window = cx.open_window(size(px(300.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let answer = native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: vec![named_id("split"), named_id("divider")],
                },
                window,
                cx,
            )
        })
    });
    assert!(answer.is_err(), "{answer:?}");
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
    let grab = crate::render::GRAB;
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
