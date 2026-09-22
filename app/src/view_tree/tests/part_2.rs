#[gpui_kit::test]
fn typed_input_state_is_scoped_by_its_authored_parent(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let field = |handler| {
        let mut node = input("Message", false, false);
        let wire::Node::Input {
            id,
            value,
            on_input,
            on_submit,
            ..
        } = &mut node
        else {
            unreachable!()
        };
        *id = wire::ElementIdWire::Name("field".into());
        value.clear();
        *on_input = handler;
        *on_submit = Some(handler + 10);
        node
    };
    let root = container(
        "form",
        [
            container("left", [field(1)]),
            container("right", [field(2)]),
        ],
    );
    let window = cx.open_window(size(px(500.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));

    let (left, right) = tree.read_with(&native, |tree, _| {
        assert_eq!(tree.fields.len(), 2);
        let left = vec![named_id("form"), named_id("left"), named_id("field")];
        let right = vec![named_id("form"), named_id("right"), named_id("field")];
        (
            tree.fields[&left].state.clone(),
            tree.fields[&right].state.clone(),
        )
    });
    assert_ne!(left.entity_id(), right.entity_id());

    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone())
        })
    });
    native.update(|window, cx| left.update(cx, |state, cx| state.focus(window, cx)));
    native.simulate_input("hello");
    native.run_until_parked();
    assert!(events.borrow().iter().any(
        |event| matches!(event, wire::Event::Input { handler: 1, text } if text == "hello")
    ));
    assert_eq!(right.read_with(&native, |state, _| state.value().to_string()), "");
}

#[gpui_kit::test]
fn combo_search_reset_and_routes_use_fresh_native_state(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let combo = |reset, handler| wire::Node::ComboBox {
        key: "combo".into(),
        label: None,
        state_key: "choices".into(),
        reset,
        options: vec!["Alpha".into(), "Beta".into()],
        selected: None,
        placeholder: "Choose".into(),
        on_select: handler,
        width: Some(wire::Length::Fixed(200.)),
        settings: Box::new(wire::ComboOptions {
            input: Some(44),
            ..Default::default()
        }),
    };
    let window = cx.open_window(size(px(400.), px(200.)), |_, _| ViewTree::new(combo(0, 7)));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone())
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    let old = tree.read_with(&native, |tree, _| tree.pickers["combo"].state.clone());
    native.update(|window, cx| window.within("combo").click("input", cx));
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_input("Be");
    native.run_until_parked();
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, wire::Event::Input { handler: 44, text } if text == "Be")),
        "native search routes its query: {:?}",
        events.borrow()
    );
    let mut authoritative = combo(0, 7);
    if let wire::Node::ComboBox { selected, .. } = &mut authoritative {
        *selected = Some(0);
    }
    tree.update(&mut native, |tree, cx| tree.replace(authoritative, cx));
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        old.read_with(&native, |state, _| state.selected_value().copied()),
        Some(0),
        "authoritative Alpha is not filtered menu row zero (Beta)"
    );
    assert!(
        events.borrow().iter().any(|event| matches!(event,
        wire::Event::Input { handler: 44, text } if text.is_empty())),
        "native authoritative projection clears its search query"
    );
    native.update(|window, cx| {
        let mut pending = combo(0, 7);
        if let wire::Node::ComboBox { selected, .. } = &mut pending {
            *selected = Some(1);
        }
        tree.update(cx, |tree, cx| tree.replace(pending, cx));
        window.render_frame(cx);
        tree.update(cx, |tree, cx| tree.replace(combo(1, 99), cx));
        window.render_frame(cx);
    });
    assert_eq!(
        old.read_with(&native, |state, _| state.selected_value().copied()),
        Some(0),
        "a deferred projection cannot mutate a retired native picker"
    );
    let fresh = tree.read_with(&native, |tree, _| {
        assert_eq!(tree.measured_bounds("combo").unwrap().size.width, px(200.));
        tree.pickers["combo"].state.clone()
    });
    assert_ne!(old.entity_id(), fresh.entity_id());
    events.borrow_mut().clear();
    old.update(&mut native, |_, cx| cx.emit(SelectEvent::Confirm(Some(0))));
    assert!(
        events.borrow().is_empty(),
        "retired picker cannot emit a fresh-frame route"
    );
    fresh.update(&mut native, |_, cx| cx.emit(SelectEvent::Confirm(Some(1))));
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        wire::Event::Select {
            handler: 99,
            index: 1
        }
    )));
}

#[gpui_kit::test]
fn sensor_visibility_uses_current_routes_and_removal_does_not_replay_old_ids(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::test::TestWindowExt as _;
    let sensor = |y, hide| {
        let mut host = linear(
            "host",
            wire::Axis::Row,
            [wire::Node::Pin {
                key: "position".into(),
                x: 0.,
                y,
                width: Some(wire::Length::Fill),
                height: Some(wire::Length::Fill),
                content: Box::new(wire::Node::Sensor {
                    key: "watched".into(),
                    reset: None,
                    on_show: None,
                    on_resize: None,
                    on_hide: Some(hide),
                    anticipate: None,
                    delay: None,
                    child: Box::new(wire::Node::Space {
                        width: Some(wire::Length::Fixed(20.)),
                        height: Some(wire::Length::Fixed(20.)),
                    }),
                }),
            }],
        );
        if let wire::Node::Linear { height, .. } = &mut host {
            *height = Some(wire::Length::Fill);
        }
        host
    };
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(100.), px(100.)), |_, _| {
        ViewTree::new(sensor(0., 11))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone())
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(tree.sensors["watched"].size.is_some())
    });
    tree.update(&mut native, |tree, cx| {
        tree.replace(sensor(200., 23), cx);
        assert_eq!(
            tree.sensors["watched"].on_hide,
            Some(23),
            "routes refresh before native draw or delayed measurement"
        );
    });
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        &*events.borrow(),
        &[wire::Event::Message(23)],
        "mounted viewport exit uses the new frame route"
    );
    tree.update(&mut native, |tree, cx| tree.replace(sensor(0., 11), cx));
    native.update(|window, cx| window.render_frame(cx));
    events.borrow_mut().clear();
    tree.update(&mut native, |tree, cx| {
        tree.replace(
            wire::Node::Button {
                key: "watched".into(),
                role: None,
                selected: None,
                content: wire::ButtonContent::Label("New action".into()),
                label: None,
                checked: None,
                expanded: None,
                description: None,
                on_press: Some(11),
                width: None,
                height: None,
                padding: None,
                style: Default::default(),
            },
            cx,
        );
        assert!(
            tree.sensors.is_empty(),
            "reusing a key for another widget does not retain its sensor"
        );
    });
    native.update(|window, cx| window.render_frame(cx));
    assert!(
        events.borrow().is_empty(),
        "old hide ID 11 must not invoke the replacement action 11"
    );
    tree.update(&mut native, |tree, _| {
        assert!(
            tree.take_user_activation(&wire::Event::Message(11))
                .is_none(),
            "a generated event cannot grant device authority"
        );
    });
    native.update(|window, cx| window.click("watched", cx));
    tree.update(&mut native, |tree, _| {
        assert!(
            tree.take_user_activation(&wire::Event::Message(11))
                .is_some()
        );
        assert!(
            tree.take_user_activation(&wire::Event::Message(11))
                .is_none(),
            "one native click authorizes at most one session"
        );
    });
    assert_eq!(
        &*events.borrow(),
        &[wire::Event::Message(11)],
        "only a real new-frame click activates the replacement action"
    );
}

#[gpui_kit::test]
fn editor_obeys_authored_size_and_height_limits(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for (height, minimum, maximum, expected) in [
        (Some(wire::Length::Fixed(60.)), None, None, 60.),
        (Some(wire::Length::Fixed(60.)), Some(100.), None, 100.),
        (Some(wire::Length::Fixed(180.)), None, Some(100.), 100.),
        (None, None, None, 300.),
    ] {
        let root = wire::Node::Editor {
            key: "document".into(),
            label: None,
            options: Box::default(),
            placeholder: String::new(),
            document: wire::editor_document::EditorDocumentRef {
                document: "sizing".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: Default::default(),
                byte_len: 0,
            },
            on_document: 0,
            editable: false,
            width: Some(240.),
            height,
            min_height: minimum,
            max_height: maximum,
        };
        let store = crate::editor::wire::EditorStore::new(91);
        store.replace(&root).unwrap();
        let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
            let mut tree = ViewTree::new(root);
            tree.set_editor_store(store, cx);
            tree
        });
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        let bounds = tree
            .read_with(&native, |tree, _| tree.measured_bounds("document"))
            .unwrap();
        assert_eq!(bounds.size, size(px(240.), px(expected)));
    }
}

/// Put a document's text into a store the honest way: the store asks for
/// every document it has no text for, so answer the request it just made.
fn seed_editor_text(store: &crate::editor::wire::EditorStore, text: &str) {
    use wire::editor_document::{EditorDocumentMessage as Message, EditorTransfer};
    let asked = store.drain().into_iter().find_map(|event| match event {
        wire::Event::EditorDocument {
            message: Message::Request { id, target },
            ..
        } => Some((id, target)),
        _ => None,
    });
    let (id, target) = asked.expect("the store asks for a document it has no text for");
    store
        .frame(&wire::Frame {
            editor_documents: vec![
                Message::Transfer(EditorTransfer::Begin {
                    id: id.clone(),
                    target,
                }),
                Message::Transfer(EditorTransfer::Chunk {
                    id: id.clone(),
                    index: 0,
                    bytes: text.as_bytes().to_vec(),
                }),
                Message::Transfer(EditorTransfer::Complete { id }),
            ],
            ..Default::default()
        })
        .expect("the answer to the store's own request");
}

/// An editor asked to lay out to its own content is as tall as the words in
/// it — every line of them.
///
/// A card that grows with what you type is the whole reason a view asks for
/// `Shrink`, and a shrunk editor that reported one line's height made every
/// such card hide what had just been written in it.
#[gpui_kit::test]
fn a_shrunk_editor_is_as_tall_as_all_of_its_lines(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let words = "one\ntwo\nthree\nfour\nfive\nsix";
    let leading = 20.;
    let root = wire::Node::Editor {
        key: "document".into(),
        label: None,
        options: Box::new(wire::EditorOptions {
            size: Some(14.),
            line_height: Some(wire::LineHeight::Absolute(leading)),
            padding: Some(0.),
            ..Default::default()
        }),
        placeholder: String::new(),
        document: wire::editor_document::EditorDocumentRef {
            document: "sizing".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: words.len() as u32,
        },
        on_document: 0,
        editable: true,
        width: Some(240.),
        height: Some(wire::Length::Shrink),
        min_height: None,
        max_height: None,
    };
    // In a box with room to spare, which is the only place shrinking means
    // anything: the editor is the root of nothing in a real view, it sits
    // inside the card's own layout.
    let root = sized(
        "card",
        root,
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    let store = crate::editor::wire::EditorStore::new(91);
    store.replace(&root).unwrap();
    seed_editor_text(&store, words);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        let mut tree = ViewTree::new(root);
        tree.set_editor_store(store, cx);
        tree
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.render_frame(cx));
    let bounds = tree
        .read_with(&native, |tree, _| tree.measured_bounds("document"))
        .unwrap();
    let lines = f32::from(bounds.size.height) / leading;
    assert!(
        (lines - 6.).abs() < 0.5,
        "six lines were laid out {lines} lines tall ({:?})",
        bounds.size
    );
}

#[test]
fn geometry_and_pixels_keep_the_wire_meaning() {
    let mut path = String::new();
    let end = append_arc_to(&mut path, [0.0, 0.0], [10.0, 0.0], [10.0, 10.0], 2.0);
    assert!((end[0] - 10.0).abs() < 0.001 && (end[1] - 2.0).abs() < 0.001);
    assert!(path.contains("A2 2"));
    path.clear();
    append_arc(
        &mut path,
        [10.0, 10.0],
        [5.0, 5.0],
        0.0,
        0.0,
        std::f32::consts::TAU,
    );
    assert_eq!(path.as_str().matches('A').count(), 2);
    let pixels = wire::ImageData::Rgba {
        width: 1,
        height: 1,
        pixels: vec![255, 10, 20, 255],
    };
    let image = decode_image(&pixels).expect("one valid pixel");
    assert_eq!(image.as_bytes(0), Some([20, 10, 255, 255].as_slice()));
    assert!(decode_image(&wire::ImageData::Encoded(vec![1, 2, 3])).is_none());
}

fn button(content: wire::ButtonContent, label: Option<&str>, on_press: Option<u32>) -> wire::Node {
    wire::Node::Button {
        key: "b".into(),
        role: None,
        selected: None,
        content,
        label: label.map(str::to_owned),
        checked: None,
        expanded: None,
        description: None,
        on_press,
        width: None,
        height: None,
        padding: None,
        style: Default::default(),
    }
}

fn input(label: &str, secure: bool, disabled: bool) -> wire::Node {
    wire::Node::Input {
        options: wire::InputOptions {
            label: label.into(),
            description: Some("Shown to members".into()),
            disabled,
            ..Default::default()
        },
        id: wire::ElementIdWire::Name("i".into()),
        placeholder: "Type here".into(),
        value: "hunter2".into(),
        on_input: 1,
        on_submit: None,
        secure,
        style: Default::default(),
    }
}

fn picture(label: Option<&str>) -> [wire::Node; 3] {
    let label = label.map(str::to_owned);
    [
        wire::Node::Image {
            id: Some(wire::ElementIdWire::Name("img".into())),
            hash: 1,
            data: None,
            label: label.clone(),
            image_style: wire::ImageStyle { grayscale: false, object_fit: wire::ImageObjectFit::Contain },
            loading: false,
            fallback: false,
            state_children: vec![],
            style: Default::default(),
            interactivity: Default::default(),
        },
        wire::Node::ImageViewer {
            key: "viewer".into(),
            hash: 1,
            data: None,
            label: label.clone(),
            fit: None,
            width: None,
            height: None,
            options: Default::default(),
        },
        wire::Node::Svg {
            id: Some(wire::ElementIdWire::Name("svg".into())),
            source: wire::SvgSource::Data { hash: 1, bytes: None },
            transformation: wire::SvgTransformation { scale: [1., 1.], translate: [0., 0.], rotate: 0. },
            label,
            style: Default::default(),
            interactivity: Default::default(),
        },
    ]
}

#[test]
fn text_is_a_label_its_content_names() {
    let text = |content: &str| text("t", content);
    assert_eq!(
        accessible(&text("Members")),
        Accessible {
            role: Some(gpui_kit::Role::Label),
            name: Some("Members".into()),
            ..Default::default()
        }
    );
    assert_eq!(accessible(&text("")).name, None);
}

#[test]
fn a_button_is_named_by_its_label_then_its_text_and_disabled_without_a_handler() {
    use wire::ButtonContent::{Child, Label};
    let glyph = || {
        Child(Box::new(wire::Node::Space {
            width: None,
            height: None,
        }))
    };
    let plain = accessible(&button(Label("Send".into()), None, Some(1)));
    assert_eq!(
        plain,
        Accessible {
            role: Some(gpui_kit::Role::Button),
            name: Some("Send".into()),
            ..Default::default()
        }
    );
    let labelled = accessible(&button(Label("×".into()), Some("Close"), Some(1)));
    assert_eq!(labelled.name.as_deref(), Some("Close"));
    let named_icon = accessible(&button(glyph(), Some("Close"), Some(1)));
    assert_eq!(named_icon.name.as_deref(), Some("Close"));
    // an icon the view did not name has no name: the gap stays visible
    assert_eq!(accessible(&button(glyph(), None, Some(1))).name, None);
    assert_eq!(accessible(&button(glyph(), Some(""), Some(1))).name, None);
    assert!(accessible(&button(Label("Send".into()), None, None)).disabled);
    assert!(!plain.disabled);
}
