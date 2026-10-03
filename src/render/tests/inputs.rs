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

    let (events, _subscription) = emitted(&tree, &mut native);
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

/// A read-only field, as a disabled one, takes neither typing nor the text
/// assistive technology sets, and tells the view nothing; an editable one
/// takes both.
#[gpui_kit::test]
fn a_read_only_field_takes_no_typing_and_no_set_value(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::accesskit::{Action, ActionData, ActionRequest, TreeId};
    cx.update(gpui_kit::init);
    for (read_only, disabled, changes) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let mut field = input("Room name", false, disabled);
        let wire::Node::Input { options, .. } = &mut field else {
            unreachable!()
        };
        options.read_only = read_only;
        let root = container("form", [field]);
        let window = cx.open_window(size(px(400.), px(200.)), |_, _| ViewTree::new(root));
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, _subscription) = emitted(&tree, &mut native);
        let state = native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            let state = tree.read(cx).fields.values().next().unwrap().state.clone();
            state.update(cx, |state, cx| state.focus(window, cx));
            state
        });
        native.simulate_input(" typed");
        native.update(|window, cx| {
            window.render_frame(cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            let (node, _) = update
                .nodes
                .iter()
                .find(|(_, node)| node.role() == gpui_kit::Role::TextInput)
                .expect("the field has a node");
            let node = *node;
            window.dispatch_a11y_action(
                ActionRequest {
                    action: Action::SetValue,
                    target_tree: TreeId::ROOT,
                    target_node: node,
                    data: Some(ActionData::Value("set".into())),
                },
                cx,
            );
        });
        native.run_until_parked();
        let value = state.read_with(&native, |state, _| state.value().to_string());
        let told: Vec<String> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                wire::Event::Input { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        match changes {
            true => {
                assert_eq!(value, "set");
                assert!(told.contains(&"hunter2 typed".into()), "{told:?}");
                assert_eq!(told.last().map(String::as_str), Some("set"));
            }
            false => assert_eq!((value.as_str(), told), ("hunter2", Vec::new())),
        }
    }
}

/// Forge's filter row, 600 px: a field `width` wide after a 16 px margin,
/// its border `border`, then a spacer and a 100 px box.
fn filter_row(width: impl Into<gpui_kit::Length> + Clone, border: gpui_kit::Hsla) -> wire::Node {
    let mut field = input("Filter", false, false);
    let wire::Node::Input { style, .. } = &mut field else {
        unreachable!()
    };
    *style = div()
        .ml(px(16.))
        .w(width)
        .h(px(32.))
        .px_2()
        .border_1()
        .border_color(border)
        .style()
        .clone();
    container_with_style(
        "row",
        div().flex().items_center().w(px(600.)).style().clone(),
        [
            field,
            container_with_style("spacer", div().flex_1().style().clone(), []),
            container_with_style("end", div().w(px(100.)).h(px(32.)).style().clone(), []),
        ],
    )
}

/// A focused field wears one mark, on its own box. A view's input placed by
/// a margin and a width in a row a spacer fills (forge's filter) keeps that
/// box, and its node is that box and no wider: a press in the spacer's room
/// is not a press on the field. Focused, its border takes the ring's colour
/// (with the ring, an inset shadow the scene's quads do not show; `a11y`
/// pins the two together), and the kit's own ring, a border painted round
/// the box, is not drawn.
#[gpui_kit::test]
fn a_focused_field_wears_one_ring_on_its_own_box(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let grey = gpui_kit::hsla(0., 0., 0.5, 1.);
    let root = filter_row(px(260.), grey);
    let window = cx.open_window(size(px(600.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the field's box and border as drawn, the other borders drawn over
    // it, and its node's box
    let drawn = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            let scale = window.scale_factor();
            let quads = window.painted_quads();
            let bordered = || {
                quads
                    .iter()
                    .filter(|quad| quad.border_widths.left.as_f32() > 0.)
            };
            let field = bordered()
                .find(|quad| quad.bounds.size.width.as_f32() == 260. * scale)
                .expect("the field's box is drawn");
            let over = bordered()
                .filter(|quad| quad.bounds != field.bounds && quad.bounds.intersects(&field.bounds))
                .count();
            let update = window.a11y_tree().expect("an a11y tree once activated");
            let node = update
                .nodes
                .iter()
                .find(|(_, node)| node.role() == gpui_kit::Role::TextInput)
                .and_then(|(_, node)| node.bounds())
                .expect("the field's node has a box");
            let scaled = |x: f64| x as f32 / scale;
            let origin = field.bounds.origin.x.as_f32() / scale;
            (
                origin,
                field.border_color,
                over,
                (scaled(node.x0), scaled(node.x1)),
            )
        })
    };
    native.update(|window, _| window.activate_a11y());
    let unfocused = drawn(&mut native);
    assert_eq!((unfocused.0, unfocused.1, unfocused.2), (16., grey, 0));
    native.update(|window, cx| {
        let state = tree.read(cx).fields.values().next().unwrap().state.clone();
        state.update(cx, |state, cx| state.focus(window, cx));
    });
    let (origin, border, over, node) = drawn(&mut native);
    let ink = native.update(|_, cx| crate::a11y::ink(cx));
    assert_eq!(
        (origin, border, over),
        (16., ink, 0),
        "one mark, the field's"
    );
    assert_eq!(node, (16., 276.), "the field's node is its box");
    let end = tree
        .read_with(&native, |tree, _| {
            tree.measured_bounds(&[named_id("row"), named_id("end")])
        })
        .unwrap();
    assert_eq!(
        (end.left(), end.right()),
        (px(500.), px(600.)),
        "the row's last box keeps its width and its place"
    );
}

/// A view's input a fraction of its row wide takes that fraction once: the
/// wrapper takes it of the row, and the kit's box the whole of the wrapper.
#[gpui_kit::test]
fn a_fraction_wide_field_takes_its_fraction_once(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let grey = gpui_kit::hsla(0., 0., 0.5, 1.);
    let root = filter_row(gpui_kit::relative(0.5), grey);
    let window = cx.open_window(size(px(600.), px(200.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let width = native.update(|window, cx| {
        window.render_frame(cx);
        let scale = window.scale_factor();
        window
            .painted_quads()
            .iter()
            .find(|quad| quad.border_widths.left.as_f32() > 0. && quad.border_color == grey)
            .expect("the field's box is drawn")
            .bounds
            .size
            .width
            .as_f32()
            / scale
    });
    assert_eq!(width, 300.);
}

/// A disabled field that holds focus (disabled while focused, as a busy
/// dialog's) wears no ring: the kit hides its own focus look there too.
#[gpui_kit::test]
fn a_disabled_field_wears_no_ring(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let grey = gpui_kit::hsla(0., 0., 0.5, 1.);
    let mut field = input("Filter", false, true);
    let wire::Node::Input { style, .. } = &mut field else {
        unreachable!()
    };
    *style = div()
        .w(px(260.))
        .h(px(32.))
        .border_1()
        .border_color(grey)
        .style()
        .clone();
    let root = container_with_style("row", div().w(px(600.)).style().clone(), [field]);
    let window = cx.open_window(size(px(600.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let border = native.update(|window, cx| {
        let state = tree.read(cx).fields.values().next().unwrap().state.clone();
        state.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
        let scale = window.scale_factor();
        window
            .painted_quads()
            .iter()
            .find(|quad| {
                quad.border_widths.left.as_f32() > 0.
                    && quad.bounds.size.width.as_f32() == 260. * scale
            })
            .expect("the field's box is drawn")
            .border_color
    });
    assert_eq!(border, grey);
}

/// An editor's box is the view's, border and padding round the text: that
/// box, not the text inside the padding, wears the ring and its colour.
#[gpui_kit::test]
fn a_focused_editor_wears_the_ring_on_the_views_box(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let grey = gpui_kit::hsla(0., 0., 0.5, 1.);
    let root = wire::Node::Editor {
        id: named_id("document"),
        style: div()
            .w(px(240.))
            .h(px(60.))
            .px_2()
            .border_1()
            .border_color(grey)
            .style()
            .clone(),
        label: None,
        binding: None,
        placeholder: String::new(),
        document: wire::editor_document::EditorDocumentRef {
            document: "ring".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
    };
    let (tree, mut native) = with_editors(root, None, cx);
    let border = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            let scale = window.scale_factor();
            window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.bounds.size.width.as_f32() == 240. * scale)
                .expect("the view's box is drawn")
                .border_color
        })
    };
    assert_eq!(border(&mut native), grey);
    native.update(|window, cx| {
        let path = [named_id("document")];
        let editor_mount::EditorView::Text(editor) = &tree.read(cx).editors[path.as_slice()].view;
        let editor = editor.clone();
        let focus = wire::WidgetCommand::Focus {
            target: path.to_vec(),
        };
        editor.update(cx, |editor, cx| editor.widget_command(&focus, window, cx));
    });
    let ink = native.update(|_, cx| crate::a11y::ink(cx));
    assert_eq!(border(&mut native), ink);
}

/// A cursor command moves the caret where it is and takes no keys: only
/// `Focus` does, and only through the seat's gate.
#[gpui_kit::test]
fn a_cursor_command_moves_the_caret_without_taking_the_keys(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = wire::Node::Editor {
        id: named_id("document"),
        style: div().w(px(240.)).h(px(80.)).style().clone(),
        label: None,
        binding: None,
        placeholder: String::new(),
        document: wire::editor_document::EditorDocumentRef {
            document: "ring".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: "some words".len() as u32,
        },
        on_document: 0,
        editable: true,
    };
    let (tree, mut native) = with_editors(root, Some("some words"), cx);
    native.update(|window, cx| window.render_frame(cx));
    let path = [named_id("document")];
    let run = |native: &mut gpui_kit::VisualTestContext, command: wire::WidgetCommand| {
        native.update(|window, cx| {
            let editor_mount::EditorView::Text(editor) =
                &tree.read(cx).editors[path.as_slice()].view;
            let editor = editor.clone();
            editor.update(cx, |editor, cx| editor.widget_command(&command, window, cx));
            editor.read(cx).is_focused(window, cx)
        })
    };
    assert!(
        !run(
            &mut native,
            wire::WidgetCommand::CursorEnd {
                target: path.to_vec()
            }
        ),
        "CursorEnd took the keys"
    );
    assert!(
        !run(
            &mut native,
            wire::WidgetCommand::SelectAll {
                target: path.to_vec()
            }
        ),
        "SelectAll took the keys"
    );
    assert!(run(
        &mut native,
        wire::WidgetCommand::Focus {
            target: path.to_vec()
        }
    ));
}

/// A key typed into a native editor activates the view as the host
/// receives it, claimed by the binding (chat's ⌘V, acted on a redraw
/// later: the activation is already there) or not; a claimed Escape does
/// not.
#[gpui_kit::test]
fn a_key_in_a_native_editor_activates_the_view_and_escape_does_not(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let claims = vec![
        view_wire::EditorKeyClaim {
            key: view_wire::keyboard::Key::Named(view_wire::keyboard::Named::Escape),
            modifiers: Default::default(),
            command: false,
        },
        view_wire::EditorKeyClaim {
            key: view_wire::keyboard::Key::Named(view_wire::keyboard::Named::Enter),
            modifiers: Default::default(),
            command: false,
        },
    ];
    let root = wire::Node::Editor {
        id: named_id("document"),
        style: div().w(px(240.)).h(px(80.)).style().clone(),
        label: None,
        binding: Some(Box::new(view_wire::EditorBinding {
            claims,
            on_request: 1,
            on_event: 2,
        })),
        placeholder: String::new(),
        document: wire::editor_document::EditorDocumentRef {
            document: "ring".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: "some words".len() as u32,
        },
        on_document: 0,
        editable: true,
    };
    let (tree, mut native) = with_editors(root, Some("some words"), cx);
    native.update(|window, cx| window.render_frame(cx));
    let path = [named_id("document")];
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: path.to_vec(),
                },
                window,
                cx,
            )
            .unwrap();
        });
        window.render_frame(cx);
    });
    tree.read_with(&native, |tree, _| tree.take_activation());
    let press = |native: &mut gpui_kit::VisualTestContext, key: &str| {
        native.update(|window, cx| {
            window.dispatch_keystroke(gpui_kit::Keystroke::parse(key).unwrap(), cx);
            window.render_frame(cx);
        });
    };
    press(&mut native, "escape");
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_none(),
            "Escape activated the view"
        );
    });
    press(&mut native, "enter");
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "a claimed key did not activate the view"
        );
    });
    press(&mut native, "a");
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "typing did not activate the view"
        );
    });
    native.simulate_mouse_down(
        gpui_kit::point(px(5.), px(5.)),
        gpui_kit::MouseButton::Left,
        Default::default(),
    );
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "a press on the field did not activate the view"
        );
    });
    // the view's own cursor commands are no input: they stamp nothing
    for command in [
        wire::WidgetCommand::CursorFront {
            target: path.to_vec(),
        },
        wire::WidgetCommand::SelectAll {
            target: path.to_vec(),
        },
    ] {
        native.update(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.execute_widget_command(command, window, cx).unwrap();
            });
            window.render_frame(cx);
        });
        tree.read_with(&native, |tree, _| {
            assert!(
                tree.take_activation().is_none(),
                "the view's own cursor command activated it"
            );
        });
    }
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
            binding: None,
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
        let (tree, mut native) = with_editors(root, None, cx);
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
        binding: None,
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
    let (tree, mut native) = with_editors(root, Some(words), cx);
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

/// A field's selection crosses a new generation of its guest, a backward one
/// included: its words stay selected. Its caret comes back at the far end,
/// since gpui-base 0.7.0 reads a backward range as empty (0.6.4 kept it at 2).
#[gpui_kit::test]
fn a_backward_selection_crosses_a_new_generation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = || container("form", [input("Room name", false, false)]);
    let window = cx.open_window(size(px(400.), px(200.)), |_, _| ViewTree::new(root()));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // "hunter2", the caret at 5, then Shift+Left three times: 2..5, caret at 2
    let saved = native.update(|window, cx| {
        window.render_frame(cx);
        let state = tree.read(cx).fields.values().next().unwrap().state.clone();
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(5..5, cx);
        });
        window.render_frame(cx);
        for _ in 0..3 {
            window.dispatch_keystroke(Keystroke::parse("shift-left").unwrap(), cx);
        }
        let selected = state.read_with(cx, |state, _| (state.selected_range(), state.cursor()));
        assert_eq!(selected, (2..5, 2));
        tree.read(cx).presentation(window, cx)
    });
    let window = cx.open_window(size(px(400.), px(200.)), |_, _| {
        ViewTree::new(root()).with_presentation(saved)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let restored = native.update(|window, cx| {
        window.render_frame(cx);
        let state = tree.read(cx).fields.values().next().unwrap().state.clone();
        state.read_with(cx, |state, _| (state.selected_range(), state.cursor()))
    });
    assert_eq!(restored, (2..5, 5));
}

/// A target is a whole authored path, as the guest SDK sends it. Here one
/// scope `b` sits inside `a` and another `b` stands alone, each with a
/// field: the lone one's is focused, never the first field whose path
/// only ends the same way.
#[gpui_kit::test]
fn a_focus_takes_the_field_at_its_whole_path(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let field = || {
        let mut node = input("Message", false, false);
        let wire::Node::Input { id, .. } = &mut node else {
            unreachable!()
        };
        *id = named_id("input");
        node
    };
    // an id-less root: every path starts at the scopes under it
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![
            container("a", [container("b", [field()])]),
            container("b", [field()]),
        ],
    });
    let window = cx.open_window(size(px(500.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let nested = vec![named_id("a"), named_id("b"), named_id("input")];
    let alone = vec![named_id("b"), named_id("input")];
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: alone.clone(),
                },
                window,
                cx,
            )
            .unwrap();
            assert!(tree.target_focused(&alone, window, cx), "the one asked for");
            assert!(!tree.target_focused(&nested, window, cx));
        })
    });
}
