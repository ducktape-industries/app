use super::*;

/// A form: a heading, a button the view hears on 31 and a field it hears
/// on 1, the field submitting on 41.
fn form() -> wire::Node {
    let mut send = button("send", "Send");
    if let wire::Node::Container(view_wire::ContainerNode {
        interactivity: Some(interactivity),
        ..
    }) = &mut send
    {
        interactivity.on_click = Some(31);
    }
    let mut field = input("Message", false, false);
    if let wire::Node::Field { on_submit, .. } = &mut field {
        *on_submit = Some(41);
    }
    container("form", [text("title", "New message"), field, send])
}

fn mounted(
    root: wire::Node,
    cx: &mut gpui_kit::TestAppContext,
) -> (Entity<ViewTree>, gpui_kit::VisualTestContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    (tree, native)
}

/// The inspector reads the view's own tree: every node by its authored
/// path, what it is called and shows, and what pressing it can send.
#[gpui_kit::test]
fn the_inspector_reads_the_views_tree_by_path(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(form(), cx);
    let nodes = native.update(|window, cx| tree.read(cx).inspect(window, cx));
    let at = |path: &str| {
        nodes
            .iter()
            .find(|node| node.path == path)
            .unwrap_or_else(|| panic!("no {path} in {nodes:#?}"))
    };
    assert_eq!(at("form").kind, "container");
    assert_eq!(at("form/title").text.as_deref(), Some("New message"));
    let send = at("form/send");
    assert_eq!(send.role.as_deref(), Some("Button"));
    assert_eq!(send.label.as_deref(), Some("Send"));
    assert_eq!(send.presses, ["click"]);
    let field = at("form/i");
    assert_eq!(field.kind, "field");
    assert_eq!(field.label.as_deref(), Some("Message"));
    assert_eq!(field.value.as_deref(), Some("hunter2"));
    assert_eq!(field.presses, ["type", "submit"]);
}

/// A secure field's text never leaves through the inspector.
#[gpui_kit::test]
fn a_secure_fields_text_is_not_read(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(container("form", [input("PIN", true, false)]), cx);
    let nodes = native.update(|window, cx| tree.read(cx).inspect(window, cx));
    let field = nodes.iter().find(|node| node.path == "form/i").unwrap();
    assert_eq!(field.value, None);
}

/// A press sends the node's own handler event and records the view's
/// activation, with no pointer: a node something covers is pressed all
/// the same. A press the node does not take is refused in words.
#[gpui_kit::test]
fn a_press_sends_the_nodes_own_event(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(form(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            tree.inspect_press("form/send", "click", cx).unwrap();
            tree.inspect_press("form/i", "submit", cx).unwrap();
            let refused = tree.inspect_press("form/title", "click", cx).unwrap_err();
            assert!(refused.contains("takes no click"), "{refused}");
            let missing = tree.inspect_press("form/gone", "click", cx).unwrap_err();
            assert!(missing.contains("no node"), "{missing}");
        })
    });
    let heard: Vec<String> = events
        .borrow()
        .iter()
        .map(|event| match event {
            wire::Event::Click { handler, .. } => format!("click {handler}"),
            wire::Event::Message(route) => format!("message {route}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(heard, ["click 31", "message 41"]);
    assert!(tree.read_with(&native, |tree, _| tree.take_activation().is_some()));
}

/// Typing goes into the engine at its caret, which tells the view as
/// typing on a keyboard does.
#[gpui_kit::test]
fn typing_reaches_the_view_as_its_text(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(form(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.inspect_type("form/i", "!", window, cx).unwrap();
            let refused = tree.inspect_type("form/send", "x", window, cx).unwrap_err();
            assert!(refused.contains("not a field"), "{refused}");
        });
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
    assert!(
        events.borrow().iter().any(|event| matches!(
            event,
            wire::Event::Text { handler: 1, change } if change.text == "hunter2!"
        )),
        "{:?}",
        events.borrow()
    );
}

#[cfg(feature = "ax-door")]
#[gpui_kit::test]
fn the_listeners_at_a_point_are_the_nodes_drawn_there_innermost_first(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::ax::natives::serve(cx);
    });
    let mut send = button("send", "Send");
    if let wire::Node::Container(view_wire::ContainerNode {
        interactivity: Some(interactivity),
        ..
    }) = &mut send
    {
        interactivity.on_click = Some(32);
        interactivity.on_mouse_down = Some(33);
    }
    let mut page = container_with_style(
        "page",
        div().w(px(300.)).h(px(200.)).style().clone(),
        [send],
    );
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut page {
        interactivity.get_or_insert_default().on_click = Some(31);
    }
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(page));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    native.update(crate::render::draw_frame);
    let trees = [("t:v".to_owned(), tree)];
    let mut at = |x: f32, y: f32| {
        native.update(|window, cx| {
            crate::ax::inspect::listening_at(&trees, &[], point(px(x), px(y)), window, cx)
        })
    };
    let ids = |answer: &serde_json::Value| -> Vec<String> {
        answer["listeners"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let on_send = at(10., 10.);
    assert_eq!(
        on_send["scope"],
        "host-registered listeners; kit internals not listed"
    );
    assert_eq!(ids(&on_send), ["t:v/page/send", "t:v/page"]);
    assert_eq!(
        on_send["listeners"][0]["listeners"],
        serde_json::json!([
            { "on": "on_click", "handler": 32 },
            { "on": "on_mouse_down", "handler": 33 },
        ])
    );
    assert_eq!(ids(&at(250., 150.)), ["t:v/page"]);
    assert!(ids(&at(350., 250.)).is_empty(), "outside the page");
}
