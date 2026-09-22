fn variable_list_node(
    count: usize,
    alignment: wire::ListAlignment,
    range_start: usize,
    rows: Vec<wire::Node>,
) -> wire::Node {
    let mut style = gpui_kit::StyleRefinement::default();
    style.size.width = Some(relative(1.).into());
    style.size.height = Some(relative(1.).into());
    wire::Node::List {
        state: 17,
        path: vec![wire::ElementIdWire::Name("room".into())],
        item_count: count,
        alignment,
        overdraw: 32.,
        sizing: wire::ListSizingBehavior::Auto,
        following_tail: alignment == wire::ListAlignment::Bottom,
        revision: 0,
        commands: Vec::new(),
        request_handler: 41,
        scroll_handler: Some(42),
        range_start,
        style,
        children: rows,
    }
}

fn fixed_row(id: u64, height: f32, color: u32) -> wire::Node {
    let mut style = gpui_kit::StyleRefinement::default().h(px(height)).w_full();
    style.background = Some(rgb(color).into());
    wire::Node::Container {
        id: Some(wire::ElementIdWire::Integer(id)),
        style,
        interactivity: Default::default(),
        children: vec![],
    }
}

#[gpui_kit::test]
fn native_variable_list_measures_different_heights_and_keeps_slot_clip(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let root = variable_list_node(
        3,
        wire::ListAlignment::Top,
        0,
        vec![
            fixed_row(10, 20., 0xff0000),
            fixed_row(11, 60., 0x00ff00),
            fixed_row(12, 100., 0x0000ff),
        ],
    );
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        let list = tree
            .variable_lists
            .values()
            .next()
            .expect("native list state");
        assert_eq!(list.state.bounds_for_item(0).unwrap().size.height, px(20.));
        assert_eq!(list.state.bounds_for_item(1).unwrap().size.height, px(60.));
        assert_eq!(list.state.bounds_for_item(2).unwrap().size.height, px(100.));
    });
    native.update(|window, _| {
        let bound = gpui_kit::ScaledPixels(200. * window.scale_factor());
        for quad in window.painted_quads() {
            assert!(quad.content_mask.bounds.right() <= bound);
        }
    });
}

#[gpui_kit::test]
fn missing_far_rows_emit_one_bounded_request_and_bottom_anchor_uses_tail_rows(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let root = variable_list_node(
        2_000,
        wire::ListAlignment::Bottom,
        1_998,
        vec![
            fixed_row(1998, 48., 0x111111),
            fixed_row(1999, 96., 0x222222),
        ],
    );
    let window = cx.open_window(size(px(240.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone())
        })
    });
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    native.run_until_parked();
    let requests = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::ListRequest { request, .. } => Some(*request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !requests.is_empty(),
        "leading overdraw requests its missing row"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.end - request.start <= wire::MAX_LIST_ROWS)
    );
    tree.read_with(&native, |tree, _| {
        let list = tree.variable_lists.values().next().unwrap();
        assert!(list.rows.contains_key(&1_998) && list.rows.contains_key(&1_999));
        assert!(matches!(
            &list.rows[&1_998],
            wire::Node::Container {
                id: Some(wire::ElementIdWire::Integer(1998)),
                ..
            }
        ));
        assert!(list.state.logical_scroll_top().item_ix >= 1_998);
    });
}
