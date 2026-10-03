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

/// What a node consumes stops at it, as gpui's `cx.stop_propagation()` in
/// its listener would: a link's click does not reach the card it is drawn
/// on, and the Enter and Space it takes for its keyboard click do not
/// reach the composite around it, which hears every other key.
#[gpui_kit::test]
fn a_consumed_press_stops_at_the_node_that_consumes_it(cx: &mut gpui_kit::TestAppContext) {
    const CARD: u32 = 81;
    const LINK: u32 = 82;
    const KEYS: u32 = 83;
    cx.update(gpui_kit::init);
    let mut link = button("link", "block 12");
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut link {
        interactivity.on_click = Some(LINK);
        interactivity.consumes_click = true;
        interactivity.consumes_keys = vec!["enter".into(), "space".into()];
    }
    let mut card = container_with_style("card", div().size_full().style().clone(), [link]);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut card {
        interactivity.on_click = Some(CARD);
        interactivity.on_key_down = Some(KEYS);
    }
    let window = cx.open_window(size(px(120.), px(80.)), |_, _| ViewTree::new(card));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.click("link", cx));
    native.update(|window, cx| {
        window.focus_next(cx);
        window.render_frame(cx);
    });
    for key in ["space", "enter", "x"] {
        native.update(|window, cx| {
            let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
            window.dispatch_keystroke(keystroke.clone(), cx);
            window.dispatch_event(
                gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }),
                cx,
            );
        });
    }
    let heard: Vec<String> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Click { handler, .. } => Some(format!("click {handler}")),
            wire::Event::KeyDown {
                handler: KEYS,
                event,
                ..
            } => Some(event.clone().into_gpui().keystroke.key),
            _ => None,
        })
        .collect();
    assert_eq!(
        heard,
        ["click 82", "click 82", "click 82", "x"],
        "the link took its presses, the card and the composite none"
    );
}
