#[gpui_kit::test]
fn native_gpui_click_grants_one_user_activation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut root = container_with_style("action", div().size_full().style().clone(), []);
    if let wire::Node::Container { interactivity, .. } = &mut root {
        interactivity.on_click = Some(71);
        interactivity.on_aux_click = Some(72);
    }
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
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
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_user_activation(&wire::Event::Message(71))
                .is_none()
        );
    });
    native.update(|window, cx| window.click("action", cx));
    let observed_events = events.borrow();
    let event = observed_events
        .iter()
        .find(|event| matches!(event, wire::Event::Click { handler: 71, .. }))
        .expect("native click delivers the actual click payload");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_user_activation(event).is_some());
        assert!(tree.take_user_activation(event).is_none());
    });
    drop(observed_events);
    native.simulate_mouse_down(
        point(px(60.), px(40.)),
        gpui_kit::MouseButton::Right,
        Default::default(),
    );
    native.simulate_mouse_up(
        point(px(60.), px(40.)),
        gpui_kit::MouseButton::Right,
        Default::default(),
    );
    let events = events.borrow();
    let event = events
        .iter()
        .find(|event| matches!(event, wire::Event::AuxClick { handler: 72, .. }))
        .expect("native auxiliary click delivers its click payload");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_user_activation(event).is_some());
        assert!(tree.take_user_activation(event).is_none());
    });
}

#[gpui_kit::test]
fn native_rich_text_click_grants_one_user_activation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = wire::Node::RichText {
        id: Some(named_id("link")), style: Default::default(), text: "Open link".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()), font_family_overrides: Vec::new(),
        clickable_ranges: vec![0..9], on_click: Some(72), on_hover: None, tooltip: None,
    };
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| cx.subscribe(&tree, move |_, event: &wire::Event, _| {
        observed.borrow_mut().push(event.clone());
    }));
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| assert!(tree.take_user_activation(&wire::Event::Select { handler: 72, index: 0 }).is_none()));
    native.simulate_mouse_move(point(px(5.), px(5.)), None, Default::default());
    native.simulate_mouse_down(point(px(5.), px(5.)), MouseButton::Left, Default::default());
    native.simulate_mouse_up(point(px(5.), px(5.)), MouseButton::Left, Default::default());
    let events = events.borrow();
    let event = events.iter().find(|event| matches!(event, wire::Event::Select { handler: 72, index: 0 })).expect("native text range click");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_user_activation(event).is_some());
        assert!(tree.take_user_activation(event).is_none());
    });
}
