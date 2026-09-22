#[gpui_kit::test]
fn native_gpui_click_grants_one_user_activation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut root = container_with_style("action", div().size_full().style().clone(), []);
    if let wire::Node::Container { interactivity, .. } = &mut root {
        interactivity.on_click = Some(71);
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
    let events = events.borrow();
    let event = events
        .iter()
        .find(|event| matches!(event, wire::Event::Click { handler: 71, .. }))
        .expect("native click delivers the actual click payload");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_user_activation(event).is_some());
        assert!(tree.take_user_activation(event).is_none());
    });
}
