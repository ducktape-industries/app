use super::*;

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
        *on_input = Some(handler);
        *on_submit = Some(handler + 10);
        node
    };
    let branch = |id, child| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(id),
            style: gpui_kit::StyleRefinement::default(),
            interactivity: Default::default(),
            children: vec![child],
        })
    };
    let root = container(
        "form",
        [
            branch(wire::ElementIdWire::Integer(1), field(1)),
            branch(named_id("1"), field(2)),
        ],
    );
    let window = cx.open_window(size(px(500.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));

    let (left, right) = tree.read_with(&native, |tree, _| {
        assert_eq!(tree.fields.len(), 2);
        let left = vec![
            named_id("form"),
            wire::ElementIdWire::Integer(1),
            named_id("field"),
        ];
        let right = vec![named_id("form"), named_id("1"), named_id("field")];
        (
            tree.fields[&left].state.clone(),
            tree.fields[&right].state.clone(),
        )
    });
    assert_ne!(left.entity_id(), right.entity_id());

    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            let left_path = vec![
                named_id("form"),
                wire::ElementIdWire::Integer(1),
                named_id("field"),
            ];
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: left_path.clone(),
                },
                window,
                cx,
            )
            .unwrap();
            assert!(tree.target_focused(&left_path, window, cx));
            assert!(!tree.target_focused(
                &[named_id("form"), named_id("1"), named_id("field")],
                window,
                cx,
            ));
        });
    });

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
    assert!(
        events.borrow().iter().any(
            |event| matches!(event, wire::Event::Input { handler: 1, text } if text == "hello")
        )
    );
    assert_eq!(
        right.read_with(&native, |state, _| state.value().to_string()),
        ""
    );
}

#[gpui_kit::test]
fn editor_obeys_authored_size_and_height_limits(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for (height, minimum, maximum, expected) in [
        (Some(60.), None, None, 60.),
        (Some(60.), Some(100.), None, 100.),
        (Some(180.), None, Some(100.), 100.),
        (None, None, None, 300.),
    ] {
        let mut authored = div().w(px(240.));
        if let Some(height) = height {
            authored = authored.h(px(height));
        } else {
            authored = authored.h_full();
        }
        if let Some(minimum) = minimum {
            authored = authored.min_h(px(minimum));
        }
        if let Some(maximum) = maximum {
            authored = authored.max_h(px(maximum));
        }
        let root = wire::Node::Editor {
            id: named_id("document"),
            style: authored.style().clone(),
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
            .read_with(&native, |tree, _| {
                tree.measured_bounds(&[named_id("document")])
            })
            .unwrap();
        assert_eq!(bounds.size, size(px(240.), px(expected)));
    }
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
    let mut authored = div()
        .w(px(240.))
        .text_size(px(14.))
        .line_height(px(leading));
    let root = wire::Node::Editor {
        id: named_id("document"),
        style: authored.style().clone(),
        label: None,
        options: Box::new(wire::EditorOptions {
            presentation: Some(Box::new(wire::editor_presentation::EditorPresentation {
                style: div().p_0().style().clone(),
                ..Default::default()
            })),
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
    };
    // In a box with room to spare, which is the only place shrinking means
    // anything: the editor is the root of nothing in a real view, it sits
    // inside the card's own layout.
    let root = sized("card", root, Some(fill()), Some(fill()));
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
        .read_with(&native, |tree, _| {
            tree.measured_bounds(&[named_id("card"), named_id("document")])
        })
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
    use crate::render::canvas::{append_arc, append_arc_to};
    use crate::render::picture_resources::decode_image;
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
