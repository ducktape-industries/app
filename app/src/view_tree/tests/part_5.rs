use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

fn uniform_node(id: &str, count: usize, rows: Range<usize>) -> wire::Node {
    let wire_id = named_id(id);
    let mut indices = rows.map(|index| index as u32).collect::<Vec<_>>();
    if count > 0 && !indices.contains(&0) {
        indices.insert(0, 0);
    }
    let children = indices
        .iter()
        .map(|index| {
            sized(
                &format!("{id}/row:{index}"),
                text(&format!("{id}/label:{index}"), format!("Row {index}")),
                Some(fill()),
                Some(fixed(24.)),
            )
        })
        .collect();
    wire::Node::UniformList {
        id: wire_id.clone(),
        path: vec![wire_id],
        route: 1,
        style: sized_style(Some(fill()), Some(fixed(96.))),
        interactivity: Default::default(),
        count,
        measure_index: 0,
        sizing: wire::list::UniformListSizing::Auto,
        horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
        y_flipped: false,
        scroll_request: None,
        indices,
        children,
    }
}

#[gpui_kit::test]
fn uniform_list_measures_row_zero_and_emits_bounded_viewport_ranges(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let node = uniform_node("uniform", 2_000, 0..1);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::UniformListRange { .. } = event {
                received.borrow_mut().push(event.clone());
            }
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();

    let events = events.borrow();
    assert!(
        events.iter().any(|event| matches!(
            event,
            wire::Event::UniformListRange {
                start: 0,
                end,
                ..
            } if *end > 1 && (*end as usize) <= wire::MAX_UNIFORM_LIST_ROWS
        )),
        "initial viewport event: {events:?}"
    );
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert_eq!(state.rows.len(), 1, "the first frame stores only row zero");
        assert!(state.rows.contains_key(&0));
        assert!(state.requested.is_some());
    });
}

#[gpui_kit::test]
fn uniform_list_scroll_requests_far_rows_without_guest_layout_or_unbounded_state(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let node = uniform_node("uniform", 2_000, 0..1);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::UniformListRange { .. } = event {
                received.borrow_mut().push(event.clone());
            }
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    events.borrow_mut().clear();

    tree.update(&mut native, |tree, _| {
        tree.uniform_lists
            .get(&vec![named_id("uniform")])
            .unwrap()
            .scroll
            .scroll_to_item(1_500, gpui_kit::ScrollStrategy::Top);
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();

    let events = events.borrow();
    let far = events.iter().find_map(|event| match event {
        wire::Event::UniformListRange { start, end, .. } if *start >= 1_500 => Some((*start, *end)),
        _ => None,
    });
    let (start, end) = far.expect("far scroll asks guest for a visible range");
    assert!(end > start);
    assert!((end - start) as usize <= wire::MAX_UNIFORM_LIST_ROWS);
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert!(state.rows.len() <= wire::MAX_UNIFORM_LIST_ROWS + 1);
        assert!(state.rows.contains_key(&0));
    });
}

#[gpui_kit::test]
fn uniform_list_row_cache_contains_only_the_current_frame(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let first = uniform_node("uniform", 2_000, 0..4);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(first));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();

    tree.update(&mut native, |tree, cx| {
        tree.replace(uniform_node("uniform", 2_000, 1_000..1_020), cx);
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert!(
            state.rows.contains_key(&0),
            "measurement row survives patch"
        );
        assert!(state.rows.contains_key(&1_000));
        assert_eq!(state.rows.len(), 21);
        assert!(
            state
                .rows
                .keys()
                .all(|index| *index == 0 || *index >= 1_000)
        );
        assert!(
            !state.rows.contains_key(&1),
            "an old row callback cannot survive into the new frame"
        );
    });
}

#[gpui_kit::test]
fn uniform_list_click_uses_native_identity_and_records_user_activation(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let mut node = uniform_node("uniform-click", 4, 0..4);
    let wire::Node::UniformList { interactivity, .. } = &mut node else {
        unreachable!()
    };
    interactivity.on_click = Some(42);
    interactivity.role = Some(gpui_kit::Role::List);
    interactivity.aria.label = Some("Rows".into());
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            received.borrow_mut().push(event.clone());
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| {
        window
            .within("uniform-click")
            .click("uniform-click/row:0", cx)
    });

    assert!(events.borrow().iter().any(|event| matches!(
        event,
        wire::Event::Click {
            handler: 42,
            event: wire::click::Click::Mouse { .. }
        }
    )));
    assert_eq!(
        tree.read_with(&native, |tree, _| tree.user_activation.get()),
        Some(42)
    );
}
