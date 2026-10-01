use super::*;

#[gpui_kit::test]
fn native_gpui_click_grants_one_user_activation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut root = container_with_style("action", div().size_full().style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut root {
        interactivity.on_click = Some(71);
        interactivity.on_aux_click = Some(72);
    }
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_activation().is_none());
    });
    native.update(|window, cx| window.click("action", cx));
    let observed_events = events.borrow();
    let _event = observed_events
        .iter()
        .find(|event| matches!(event, wire::Event::Click { handler: 71, .. }))
        .expect("native click delivers the actual click payload");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_activation().is_some());
        assert!(tree.take_activation().is_none());
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
    let _event = events
        .iter()
        .find(|event| matches!(event, wire::Event::AuxClick { handler: 72, .. }))
        .expect("native auxiliary click delivers its click payload");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_activation().is_some());
        assert!(tree.take_activation().is_none());
    });
}

#[gpui_kit::test]
fn native_rich_text_click_grants_one_user_activation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = wire::Node::RichText {
        id: Some(named_id("link")),
        style: Default::default(),
        text: "Open link".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: std::iter::once(0..9).collect(),
        on_click: Some(72),
        on_hover: None,
        tooltip: None,
    };
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| assert!(tree.take_activation().is_none()));
    native.simulate_mouse_move(point(px(5.), px(5.)), None, Default::default());
    native.simulate_mouse_down(point(px(5.), px(5.)), MouseButton::Left, Default::default());
    native.simulate_mouse_up(point(px(5.), px(5.)), MouseButton::Left, Default::default());
    let events = events.borrow();
    let _event = events
        .iter()
        .find(|event| {
            matches!(
                event,
                wire::Event::Select {
                    handler: 72,
                    index: 0
                }
            )
        })
        .expect("native text range click");
    tree.read_with(&native, |tree, _| {
        assert!(tree.take_activation().is_some());
        assert!(tree.take_activation().is_none());
    });
}

/// A key the person presses in the view activates it; Escape does not
/// (the web's rule: a dismissal grants nothing).
#[gpui_kit::test]
fn a_key_activates_the_view_and_escape_does_not(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut root = container_with_style("keys", div().size_full().style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut root {
        interactivity.on_key_down = Some(5);
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
    }
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.focus_next(cx);
    });
    let press = |native: &mut gpui_kit::VisualTestContext, key: &str| {
        native.update(|window, cx| {
            window.dispatch_keystroke(gpui_kit::Keystroke::parse(key).unwrap(), cx);
        });
    };
    press(&mut native, "escape");
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, wire::Event::KeyDown { handler: 5, .. })),
        "the key reached the handler"
    );
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_none(),
            "Escape activated the view"
        );
    });
    press(&mut native, "a");
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "a key did not activate the view"
        );
    });
}
