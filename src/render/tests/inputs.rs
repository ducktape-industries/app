use super::*;
use gpui_kit::Keystroke;

/// The one-line kit state behind the first field, for a test to drive.
fn line(tree: &Entity<ViewTree>, cx: &gpui_kit::VisualTestContext) -> Entity<InputState> {
    tree.read_with(cx, |tree, _| {
        tree.first_input_for_test().expect("a one-line field")
    })
}

/// `root` in a window, drawn once.
fn mounted(
    root: wire::Node,
    cx: &mut gpui_kit::TestAppContext,
) -> (Entity<ViewTree>, gpui_kit::VisualTestContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    (tree, native)
}

/// The field at `path` takes the keys, through the view's own Focus.
fn focus(
    tree: &Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
    path: &[wire::ElementIdWire],
) {
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
}

/// The text the first field holds, as the engine has it.
fn text(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext) -> String {
    native.update(|window, cx| {
        window.render_frame(cx);
        let tree = tree.read(cx);
        tree.fields
            .values()
            .next()
            .expect("a field")
            .presentation(window, cx)
            .value
    })
}

/// The text the field at `path` holds, as the engine has it.
fn text_at(
    tree: &Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
    path: &[wire::ElementIdWire],
) -> String {
    native.update(|window, cx| {
        window.render_frame(cx);
        tree.read(cx).fields[path].presentation(window, cx).value
    })
}

/// `keys`, pressed one frame apart.
fn pressed(native: &mut gpui_kit::VisualTestContext, keys: &[&str]) {
    native.update(|window, cx| {
        for key in keys {
            window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx);
            window.render_frame(cx);
        }
    });
    native.run_until_parked();
}

/// What the guest heard of the first field's text, in order.
fn told(events: &Rc<RefCell<Vec<wire::Event>>>) -> Vec<String> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Text { change, .. } => Some(change.text.clone()),
            _ => None,
        })
        .collect()
}

/// The last change the guest heard.
fn last_change(events: &Rc<RefCell<Vec<wire::Event>>>) -> wire::TextChange {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Text { change, .. } => Some(change.clone()),
            _ => None,
        })
        .next_back()
        .expect("the guest heard a change")
}

/// The guest's `Replace`: `range` of the text it read at `revision` of
/// its document `generation` becomes `text`, the caret after it.
fn replace_in(
    generation: u64,
    target: &[wire::ElementIdWire],
    revision: u64,
    range: std::ops::Range<usize>,
    text: &str,
) -> wire::WidgetCommand {
    wire::WidgetCommand::Replace {
        target: target.to_vec(),
        generation,
        revision,
        range: range.clone().into(),
        text: text.into(),
        token: None,
        cursor: wire::TextRange::caret(range.start + text.len()),
    }
}

/// `replace_in` on the generation `input` and `area` mount.
fn replace(
    target: &[wire::ElementIdWire],
    revision: u64,
    range: std::ops::Range<usize>,
    text: &str,
) -> wire::WidgetCommand {
    replace_in(1, target, revision, range, text)
}

#[gpui_kit::test]
fn typed_input_state_is_scoped_by_its_authored_parent(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let field = |handler| {
        let mut node = input("Message", false, false);
        let wire::Node::Field {
            id,
            value,
            cursor,
            on_change,
            on_submit,
            ..
        } = &mut node
        else {
            unreachable!()
        };
        *id = wire::ElementIdWire::Name("field".into());
        value.clear();
        *cursor = Default::default();
        *on_change = Some(handler);
        *on_submit = Some(handler + 10);
        node
    };
    let branch = |id, child| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(id),
            style: crate::render::test_style(gpui_kit::StyleRefinement::default()),
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

    let left_path = vec![
        named_id("form"),
        wire::ElementIdWire::Integer(1),
        named_id("field"),
    ];
    let right_path = vec![named_id("form"), named_id("1"), named_id("field")];
    let (left, right) = tree.read_with(&native, |tree, _| {
        assert_eq!(tree.fields.len(), 2);
        let line = |path: &AuthoredPath| match &tree.fields[path].engine {
            crate::render::inputs::Engine::Line(state) => state.clone(),
            crate::render::inputs::Engine::Area(_) => unreachable!(),
        };
        (line(&left_path), line(&right_path))
    });
    assert_ne!(left.entity_id(), right.entity_id());

    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: left_path.clone(),
                },
                window,
                cx,
            )
            .unwrap();
            assert!(tree.target_focused(&left_path, window, cx));
            assert!(!tree.target_focused(&right_path, window, cx));
        });
    });

    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| left.update(cx, |state, cx| state.focus(window, cx)));
    native.simulate_input("hello");
    native.run_until_parked();
    assert!(events.borrow().iter().any(
        |event| matches!(event, wire::Event::Text { handler: 1, change } if change.text == "hello")
    ));
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
        let wire::Node::Field { options, .. } = &mut field else {
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
            let state = tree.read(cx).first_input_for_test().unwrap();
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
        let told = told(&events);
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
    let wire::Node::Field { style, .. } = &mut field else {
        unreachable!()
    };
    *style = crate::render::test_style(
        div()
            .ml(px(16.))
            .w(width)
            .h(px(32.))
            .px_2()
            .border_1()
            .border_color(border)
            .style()
            .clone(),
    );
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
    // eight bits a channel, as a colour crosses the wire
    let grey: gpui_kit::Hsla = gpui_kit::rgb(0x808080).into();
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
    let state = line(&tree, &native);
    native.update(|window, cx| state.update(cx, |state, cx| state.focus(window, cx)));
    let (origin, border, over, node) = drawn(&mut native);
    let ink = native.update(|_, cx| crate::a11y::ink(cx));
    assert_eq!(
        (origin, border, over),
        (16., ink, 0),
        "one mark, the field's"
    );
    assert_eq!(node, (16., 276.), "the field's node is its box");
    let end = native.update(|window, _| window.find("end").bounds());
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
    // eight bits a channel, as a colour crosses the wire
    let grey: gpui_kit::Hsla = gpui_kit::rgb(0x808080).into();
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
    // eight bits a channel, as a colour crosses the wire
    let grey: gpui_kit::Hsla = gpui_kit::rgb(0x808080).into();
    let mut field = input("Filter", false, true);
    let wire::Node::Field { style, .. } = &mut field else {
        unreachable!()
    };
    *style = crate::render::test_style(
        div()
            .w(px(260.))
            .h(px(32.))
            .border_1()
            .border_color(grey)
            .style()
            .clone(),
    );
    let root = container_with_style("row", div().w(px(600.)).style().clone(), [field]);
    let window = cx.open_window(size(px(600.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let border = native.update(|window, cx| {
        let state = tree.read(cx).first_input_for_test().unwrap();
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

/// A growing field's box is the view's, border and padding round the text:
/// that box, not the text inside the padding, wears the ring and its colour.
#[gpui_kit::test]
fn a_focused_editor_wears_the_ring_on_the_views_box(cx: &mut gpui_kit::TestAppContext) {
    // eight bits a channel, as a colour crosses the wire
    let grey: gpui_kit::Hsla = gpui_kit::rgb(0x808080).into();
    let style = div()
        .w(px(240.))
        .h(px(60.))
        .px_2()
        .border_1()
        .border_color(grey)
        .style()
        .clone();
    let (tree, mut native) = mounted(area("document", None, "", style), cx);
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
    focus(&tree, &mut native, &[named_id("document")]);
    let ink = native.update(|_, cx| crate::a11y::ink(cx));
    assert_eq!(border(&mut native), ink);
}

/// A cursor command moves the caret where it is and takes no keys: only
/// `Focus` does, and only through the seat's gate.
#[gpui_kit::test]
fn a_cursor_command_moves_the_caret_without_taking_the_keys(cx: &mut gpui_kit::TestAppContext) {
    let style = div().w(px(240.)).h(px(80.)).style().clone();
    let (tree, mut native) = mounted(area("document", None, "some words", style), cx);
    let path = [named_id("document")];
    let run = |native: &mut gpui_kit::VisualTestContext, command: wire::WidgetCommand| {
        native.update(|window, cx| {
            tree.update(cx, |tree, cx| {
                tree.execute_widget_command(command, window, cx).unwrap();
            });
            let tree = tree.read(cx);
            let field = &tree.fields[path.as_slice()];
            (
                field.is_focused(window, cx),
                field.presentation(window, cx).selection,
            )
        })
    };
    assert_eq!(
        run(
            &mut native,
            wire::WidgetCommand::CursorFront {
                target: path.to_vec()
            }
        ),
        (false, 0..0),
        "CursorFront took the keys"
    );
    assert_eq!(
        run(
            &mut native,
            wire::WidgetCommand::SelectAll {
                target: path.to_vec()
            }
        ),
        (false, 0..10),
        "SelectAll took the keys"
    );
    assert!(
        run(
            &mut native,
            wire::WidgetCommand::Focus {
                target: path.to_vec()
            }
        )
        .0
    );
}

/// A key typed into a field activates the view as the host receives it,
/// claimed by the guest (chat's Enter, acted on a redraw later: the
/// activation is already there) or not; a claimed Escape does not.
#[gpui_kit::test]
fn a_key_in_a_native_editor_activates_the_view_and_escape_does_not(
    cx: &mut gpui_kit::TestAppContext,
) {
    let claim = |named| view_wire::KeyClaim {
        key: view_wire::keyboard::Key::Named(named),
        modifiers: Default::default(),
        command: false,
    };
    let mut root = area(
        "document",
        None,
        "some words",
        div().w(px(240.)).h(px(80.)).style().clone(),
    );
    let wire::Node::Field { claims, .. } = &mut root else {
        unreachable!()
    };
    *claims = Box::new([
        claim(view_wire::keyboard::Named::Escape),
        claim(view_wire::keyboard::Named::Enter),
    ]);
    let (tree, mut native) = mounted(root, cx);
    let path = [named_id("document")];
    focus(&tree, &mut native, &path);
    let (events, _subscription) = emitted(&tree, &mut native);
    tree.read_with(&native, |tree, _| tree.take_activation());
    let press = |native: &mut gpui_kit::VisualTestContext, key: &str| {
        native.update(|window, cx| {
            window.dispatch_keystroke(gpui_kit::Keystroke::parse(key).unwrap(), cx);
            window.render_frame(cx);
        });
        native.run_until_parked();
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
    let claimed: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::KeyDown { handler, event, .. } => {
                Some((*handler, event.state.key.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        claimed,
        vec![
            (
                2,
                view_wire::keyboard::Key::Named(view_wire::keyboard::Named::Escape)
            ),
            (
                2,
                view_wire::keyboard::Key::Named(view_wire::keyboard::Named::Enter)
            ),
        ],
        "the guest hears the keys it claimed, and the engine does not"
    );
    assert_eq!(text(&tree, &mut native), "some words");
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
        native.run_until_parked();
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
        let mut root = area("document", None, "", authored.style().clone());
        let wire::Node::Field { options, .. } = &mut root else {
            unreachable!()
        };
        options.read_only = true;
        let (_, mut native) = mounted(root, cx);
        native.update(|window, cx| window.render_frame(cx));
        let bounds = native.update(|window, _| window.find("document").bounds());
        assert_eq!(bounds.size, size(px(240.), px(expected)));
    }
}

/// A field asked to lay out to its own content is as tall as the words in
/// it — every line of them.
///
/// A card that grows with what you type is the whole reason a view asks for
/// `Shrink`, and a shrunk field that reported one line's height made every
/// such card hide what had just been written in it.
#[gpui_kit::test]
fn a_shrunk_editor_is_as_tall_as_all_of_its_lines(cx: &mut gpui_kit::TestAppContext) {
    let words = "one\ntwo\nthree\nfour\nfive\nsix";
    let leading = 20.;
    let mut authored = div()
        .w(px(240.))
        .text_size(px(14.))
        .line_height(px(leading));
    let root = area("document", None, words, authored.style().clone());
    // In a box with room to spare, which is the only place shrinking means
    // anything: the field is the root of nothing in a real view, it sits
    // inside the card's own layout.
    let root = sized("card", root, Some(fill()), Some(fill()));
    let (_, mut native) = mounted(root, cx);
    native.update(|window, cx| window.render_frame(cx));
    let bounds = native.update(|window, _| window.find("document").bounds());
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
        let state = tree.read(cx).first_input_for_test().unwrap();
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
        let state = tree.read(cx).first_input_for_test().unwrap();
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
        let wire::Node::Field { id, .. } = &mut node else {
            unreachable!()
        };
        *id = named_id("input");
        node
    };
    // an id-less root: every path starts at the scopes under it
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::plain_style(),
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

/// S9 (IM-2): a commit the IME makes between the key the guest last heard
/// and the guest's echo of it is kept; the echo is what the guest knows,
/// not what the field reads.
#[gpui_kit::test]
fn an_ime_commit_inside_the_echo_window_is_kept(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::EntityInputHandler as _;
    cx.update(gpui_kit::init);
    let field = |value: &str| {
        let mut node = input("Message", false, false);
        let wire::Node::Field {
            value: shown,
            cursor,
            ..
        } = &mut node
        else {
            unreachable!()
        };
        *shown = value.into();
        *cursor = wire::TextRange::caret(value.len());
        node
    };
    let window = cx.open_window(size(px(400.), px(200.)), |_, _| {
        ViewTree::new(container("form", [field("ab")]))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let state = native.update(|window, cx| {
        window.render_frame(cx);
        let state = tree.read(cx).first_input_for_test().unwrap();
        state.update(cx, |state, cx| state.focus(window, cx));
        state
    });
    native.simulate_input("c");
    native.run_until_parked();
    // the IME commits while the guest's frame, built on "abc", is in flight
    native.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.replace_text_in_range(None, "d", window, cx)
        });
    });
    native.run_until_parked();
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.replace(container("form", [field("abc")]), &[], cx)
        });
        window.render_frame(cx);
    });
    assert_eq!(
        state.read_with(&native, |state, _| state.value().to_string()),
        "abcd"
    );
}

// S9: the headline cases of one text model, against the engine. Each failed
// against the editor lane as it stood (TE.verify's drive of the real store).

/// The document after the field selected `lo..hi` of `text`, the writer
/// pressed Backspace and typed `typed`, and the guest's next frame still
/// showed the text it had: the engine's text stands, the guest's is
/// behind it.
fn typed_over_a_selection(
    text_before: &str,
    lo: usize,
    hi: usize,
    typed: &str,
    cx: &mut gpui_kit::TestAppContext,
) -> String {
    let style = div().w(px(240.)).h(px(80.)).style().clone();
    let (tree, mut native) = mounted(area("doc", None, text_before, style.clone()), cx);
    let path = [named_id("doc")];
    focus(&tree, &mut native, &path);
    native.update(|window, cx| {
        let crate::render::inputs::Engine::Area(state) =
            tree.read(cx).fields[path.as_slice()].engine.clone()
        else {
            unreachable!()
        };
        state.update(cx, |state, cx| state.set_selected_range(lo..hi, cx));
        window.render_frame(cx);
    });
    pressed(&mut native, &["backspace"]);
    native.simulate_input(typed);
    native.run_until_parked();
    // the guest's frame, built before any of it, echoes the old text
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.replace(area("doc", None, text_before, style), &[], cx)
        });
        window.render_frame(cx);
    });
    text(&tree, &mut native)
}

/// A word selected, Backspace, a letter typed: the person sees "say x now",
/// and the guest's late frame takes nothing back.
#[gpui_kit::test]
fn a_letter_typed_behind_a_claimed_backspace_replaces_what_was_selected(
    cx: &mut gpui_kit::TestAppContext,
) {
    assert_eq!(
        typed_over_a_selection("say word now", 4, 8, "x", cx),
        "say x now"
    );
}

/// The first word selected, Backspace, a letter: the view must not stop.
#[gpui_kit::test]
fn a_letter_typed_behind_a_claimed_backspace_on_the_first_word_never_stops_the_view(
    cx: &mut gpui_kit::TestAppContext,
) {
    assert_eq!(
        typed_over_a_selection("say word now", 0, 3, "x", cx),
        "x word now"
    );
}

/// Enter claimed (the composer's Send clears the draft), a letter typed
/// ahead of the guest's answer: the draft is cleared once and the letter is
/// kept, since the guest's ask is carried over the typing it did not see.
#[gpui_kit::test]
fn a_letter_typed_ahead_of_a_claimed_enter_is_kept(cx: &mut gpui_kit::TestAppContext) {
    let mut root = area(
        "doc",
        None,
        "hello",
        div().w(px(240.)).h(px(80.)).style().clone(),
    );
    let wire::Node::Field { claims, .. } = &mut root else {
        unreachable!()
    };
    *claims = Box::new([view_wire::KeyClaim {
        key: view_wire::keyboard::Key::Named(view_wire::keyboard::Named::Enter),
        modifiers: Default::default(),
        command: false,
    }]);
    let (tree, mut native) = mounted(root, cx);
    let path = [named_id("doc")];
    focus(&tree, &mut native, &path);
    let (events, _subscription) = emitted(&tree, &mut native);
    pressed(&mut native, &["enter"]);
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, wire::Event::KeyDown { handler: 2, .. })),
        "the guest heard its Enter"
    );
    native.simulate_input("x");
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "hellox");
    // the guest answers the Enter against the text it read: revision 0
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace(&path, 0, 0..5, ""), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "x");
    // the caret stays where the writer is, after the letter
    let heard = last_change(&events);
    assert_eq!((heard.text.as_str(), heard.cursor), ("x", (1..1).into()));
}

/// The guest hears the edit a clear made, not a diff of the texts: "ok"
/// sent while "o" was typed ahead reads "oko", then "o", and a diff of
/// those takes the first "o" for the one that stayed. The composer cuts
/// what a send spoke for by this edit, so the typed "o" stays the writer's.
#[gpui_kit::test]
fn the_guest_hears_the_edit_a_clear_made_not_a_diff_of_the_texts(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (tree, mut native) = mounted(
        area(
            "doc",
            None,
            "ok",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    let path = [named_id("doc")];
    focus(&tree, &mut native, &path);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.simulate_input("o");
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "oko");
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace(&path, 0, 0..2, ""), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    let edits: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Text { change, .. } => Some((change.text.clone(), change.edit)),
            _ => None,
        })
        .collect();
    let edit = |range: std::ops::Range<usize>, len| {
        Some(wire::Edit {
            range: range.into(),
            len,
        })
    };
    assert_eq!(
        edits,
        [
            ("oko".to_owned(), edit(2..2, 1)),
            ("o".to_owned(), edit(0..2, 0))
        ]
    );
}

/// Enter claimed with the caret inside the words, a letter typed before the
/// clear lands: the clear takes only the words the guest read, the letter
/// stays with the caret after it, and one undo brings the words back round
/// it.
#[gpui_kit::test]
fn a_letter_typed_inside_the_words_a_send_clears_is_kept(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(
        area(
            "doc",
            None,
            "hello",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    let path = [named_id("doc")];
    focus(&tree, &mut native, &path);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        let crate::render::inputs::Engine::Area(state) =
            tree.read(cx).fields[path.as_slice()].engine.clone()
        else {
            unreachable!()
        };
        state.update(cx, |state, cx| state.set_selected_range(3..3, cx));
        window.render_frame(cx);
    });
    native.simulate_input("x");
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "helxlo");
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace(&path, 0, 0..5, ""), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    let heard = last_change(&events);
    assert_eq!((heard.text.as_str(), heard.cursor), ("x", (1..1).into()));
    pressed(&mut native, &["ctrl-z"]);
    assert_eq!(text(&tree, &mut native), "helxlo", "one undo step");
}

/// The frames that arrive before an ask runs keep the edits it read
/// against: an ask runs frames after the one that carried it, and the frames
/// between (a keystroke's tick, a Drawn-owed turn, a redraw) arrive first.
/// The host holds the ask, so it knows what the ask still needs.
#[gpui_kit::test]
fn frames_arriving_before_an_ask_runs_keep_the_edits_it_read_against(
    cx: &mut gpui_kit::TestAppContext,
) {
    let style = div().w(px(240.)).h(px(80.)).style().clone();
    let (tree, mut native) = mounted(area("doc", None, "hello", style.clone()), cx);
    let path = [named_id("doc")];
    focus(&tree, &mut native, &path);
    let (events, _subscription) = emitted(&tree, &mut native);
    // revision 1: a letter at the end; revision 2: a letter inside "hello"
    native.simulate_input("a");
    native.update(|window, cx| {
        let crate::render::inputs::Engine::Area(state) =
            tree.read(cx).fields[path.as_slice()].engine.clone()
        else {
            unreachable!()
        };
        state.update(cx, |state, cx| state.set_selected_range(2..2, cx));
        window.render_frame(cx);
    });
    native.simulate_input("b");
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "heblloa");
    // the guest heard both; two frames arrive and draw before its ask,
    // built between the two letters and still queued, runs
    native.update(|window, cx| {
        for _ in 0..2 {
            let mut root = area("doc", None, "heblloa", style.clone());
            let wire::Node::Field { revision, .. } = &mut root else {
                unreachable!()
            };
            *revision = 2;
            tree.update(cx, |tree, cx| tree.replace(root, &[(path.to_vec(), 1)], cx));
            window.render_frame(cx);
        }
    });
    // the ask, against "helloa": clear "hello". The "b" typed into it since
    // stays; without the edit it was typed by, the clear would take "hebll"
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace(&path, 1, 0..5, ""), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "ba");
    assert_eq!(told(&events).last().map(String::as_str), Some("ba"));
}

/// Edits the guest has not acknowledged never stop the view, however many.
#[gpui_kit::test]
fn unacknowledged_edits_never_stop_the_view(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(
        area(
            "doc",
            None,
            "",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    focus(&tree, &mut native, &[named_id("doc")]);
    let (events, _subscription) = emitted(&tree, &mut native);
    for _ in 0..129 {
        native.simulate_input("a");
    }
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "a".repeat(129));
    let told = told(&events);
    assert_eq!(told.len(), 129, "129 edits typed ahead of the guest");
    assert_eq!(told.last().map(String::len), Some(129));
}

/// A held Backspace drains at the key's repeat rate: five presses in one
/// frame take five characters by the next.
#[gpui_kit::test]
fn a_held_backspace_drains_at_repeat_rate(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(
        area(
            "doc",
            None,
            "aaaaa",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    focus(&tree, &mut native, &[named_id("doc")]);
    native.update(|window, cx| {
        for _ in 0..5 {
            window.dispatch_keystroke(Keystroke::parse("backspace").unwrap(), cx);
        }
    });
    native.run_until_parked();
    assert_eq!(
        text(&tree, &mut native),
        "",
        "five Backspaces, one frame later"
    );
}

/// Undo crosses an indent: Tab is an edit like any other.
#[gpui_kit::test]
fn undo_crosses_a_tab(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(
        area(
            "tabs",
            None,
            "",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    focus(&tree, &mut native, &[named_id("tabs")]);
    native.simulate_input("one");
    native.run_until_parked();
    pressed(&mut native, &["tab"]);
    assert_eq!(text(&tree, &mut native), "one  ");
    pressed(&mut native, &["ctrl-z"]);
    assert_eq!(text(&tree, &mut native), "one");
}

/// Undo works after the guest cleared a sent draft: the words come back.
#[gpui_kit::test]
fn undo_after_the_guest_cleared_a_sent_draft_brings_the_words_back(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (tree, mut native) = mounted(
        area(
            "draft",
            None,
            "",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    let path = [named_id("draft")];
    focus(&tree, &mut native, &path);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.simulate_input("hi");
    native.run_until_parked();
    let revision = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Text { change, .. } => Some(change.revision),
            _ => None,
        })
        .next_back()
        .expect("the guest heard the draft");
    // the guest sends the draft and clears its field at the revision it knows
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace(&path, revision, 0..2, ""), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "");
    pressed(&mut native, &["ctrl-z"]);
    assert_eq!(text(&tree, &mut native), "hi");
}

/// Esc lets go of Tab for the next key: Tab then leaves the field; any
/// other key takes Tab back for indenting (owner, 2026-09-28; AX-022).
#[gpui_kit::test]
fn esc_then_tab_leaves_the_field_and_any_other_key_takes_tab_back(
    cx: &mut gpui_kit::TestAppContext,
) {
    // a stop after the field, for Tab to leave to, under the kit's Root,
    // which walks the focus ring on Tab as the shell's window does
    cx.update(gpui_kit::init);
    let root = container(
        "form",
        [
            area(
                "doc",
                None,
                "",
                div().w(px(240.)).h(px(80.)).style().clone(),
            ),
            input("Next", false, false),
        ],
    );
    let mut mounted = None;
    let window = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(root));
        mounted = Some(tree.clone());
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = mounted.expect("the tree is mounted");
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let path = [named_id("form"), named_id("doc")];
    let focused = |tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| tree.read(cx).fields[path.as_slice()].is_focused(window, cx))
    };
    focus(&tree, &mut native, &path);
    pressed(&mut native, &["escape", "a", "tab"]);
    assert_eq!(
        text_at(&tree, &mut native, &path),
        "a  ",
        "a key between took Tab back"
    );
    assert!(focused(&tree, &mut native));
    pressed(&mut native, &["escape", "tab"]);
    assert!(
        !focused(&tree, &mut native),
        "Esc then Tab leaves the field"
    );
    assert_eq!(text_at(&tree, &mut native, &path), "a  ");
}

/// A mention the guest asks for lands as one span with the space it asked
/// for after it, both carried over the first ask by the second: the guest
/// speaks at one revision for both.
#[gpui_kit::test]
fn a_mention_lands_as_one_span_and_its_space_follows_it(cx: &mut gpui_kit::TestAppContext) {
    let (tree, mut native) = mounted(
        area(
            "doc",
            None,
            "hi @al",
            div().w(px(240.)).h(px(80.)).style().clone(),
        ),
        cx,
    );
    let path = [named_id("doc")];
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            let label = wire::WidgetCommand::Replace {
                target: path.to_vec(),
                generation: 1,
                revision: 0,
                range: (3..6).into(),
                text: "@Alice".into(),
                token: Some("<@1>".into()),
                cursor: wire::TextRange::caret(9),
            };
            tree.execute_widget_command(label, window, cx).unwrap();
            tree.execute_widget_command(replace(&path, 0, 6..6, " "), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "hi @Alice ");
    let last = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Text { change, .. } => Some(change.clone()),
            _ => None,
        })
        .next_back()
        .expect("the guest heard the mention");
    assert_eq!(last.cursor, wire::TextRange::caret(10));
    assert_eq!(
        last.tokens,
        vec![wire::TextToken {
            range: (3..9).into(),
            id: "<@1>".into(),
        }]
    );
}

/// A Restore and an Enter the guest handled in one tick: the frame carries
/// the seeded text as a new generation and the ask a clear of it, both in
/// the seeded text's bytes. The adopt is a new document, told with no edit;
/// the clear names the generation and lands on it whole, not rebased over
/// the adopt as if it were typing. An ask on the generation the guest left
/// edits nothing, and one for a generation the field has not seen waits
/// for its frame.
#[gpui_kit::test]
fn a_clear_asked_against_a_seed_lands_after_the_adopt(cx: &mut gpui_kit::TestAppContext) {
    let style = div().w(px(240.)).h(px(80.)).style().clone();
    let (tree, mut native) = mounted(area("doc", None, "", style.clone()), cx);
    let path = [named_id("doc")];
    let (events, _subscription) = emitted(&tree, &mut native);
    let seed = |generation: u64, value: &str| {
        let mut root = area("doc", None, value, style.clone());
        let wire::Node::Field {
            generation: shown, ..
        } = &mut root
        else {
            unreachable!()
        };
        *shown = generation;
        root
    };
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.replace(seed(2, "hello"), &[(path.to_vec(), 0)], cx)
        });
        window.render_frame(cx);
    });
    native.run_until_parked();
    let adopt = last_change(&events);
    assert_eq!(
        (
            adopt.generation,
            adopt.revision,
            adopt.edit,
            adopt.text.as_str()
        ),
        (2, 1, None, "hello")
    );
    // the old generation's ask edits a document that is gone
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace_in(1, &path, 0, 0..5, "x"), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "hello");
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace_in(2, &path, 0, 0..5, ""), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "");
    let cleared = last_change(&events);
    assert_eq!(
        (cleared.generation, cleared.revision, cleared.edit),
        (
            2,
            2,
            Some(wire::Edit {
                range: (0..5).into(),
                len: 0
            })
        )
    );
    // an ask that outran its frame waits for it
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(replace_in(3, &path, 0, 0..3, "yo"), window, cx)
                .unwrap();
        });
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "");
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| tree.replace(seed(3, "abcde"), &[], cx));
        window.render_frame(cx);
    });
    native.run_until_parked();
    assert_eq!(text(&tree, &mut native), "yode");
    assert_eq!(told(&events).last().map(String::as_str), Some("yode"));
}

/// A guest's reset arrives while an IME composes: the adopt waits for the
/// commit, and until then the field is still the old document, so what the
/// IME does to it is told as the old generation's. The adopt lands at the
/// commit as the new generation, with no edit.
#[gpui_kit::test]
fn an_adopt_held_by_an_ime_keeps_the_old_generation_until_it_lands(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::EntityInputHandler as _;
    let style = div().w(px(240.)).h(px(80.)).style().clone();
    let (tree, mut native) = mounted(area("doc", None, "ab", style.clone()), cx);
    let path = [named_id("doc")];
    let (events, _subscription) = emitted(&tree, &mut native);
    let state = native.update(|_, cx| {
        let crate::render::inputs::Engine::Area(state) =
            tree.read(cx).fields[&path[..]].engine.clone()
        else {
            unreachable!()
        };
        state
    });
    // the IME starts composing at the end of "ab"
    native.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(2..2, cx);
            state.replace_and_mark_text_in_range(None, "k", None, window, cx);
        });
    });
    native.run_until_parked();
    // the guest resets to "hello" meanwhile: a frame with generation 2
    native.update(|window, cx| {
        let mut root = area("doc", None, "hello", style.clone());
        let wire::Node::Field { generation, .. } = &mut root else {
            unreachable!()
        };
        *generation = 2;
        tree.update(cx, |tree, cx| tree.replace(root, &[], cx));
        window.render_frame(cx);
    });
    native.run_until_parked();
    // the IME goes on composing: the old document, told as the old generation
    native.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "ka", None, window, cx);
        });
    });
    native.run_until_parked();
    let composing = last_change(&events);
    assert_eq!(
        (composing.generation, composing.text.as_str()),
        (1, "abka"),
        "the field is the old document until the adopt lands"
    );
    // the commit: the adopt lands as generation 2, a new document, no edit
    native.update(|window, cx| {
        state.update(cx, |state, cx| {
            state.replace_text_in_range(None, "か", window, cx);
        });
    });
    native.run_until_parked();
    let adopt = last_change(&events);
    assert_eq!(
        (adopt.generation, adopt.edit, adopt.text.as_str()),
        (2, None, "hello")
    );
    assert_eq!(text(&tree, &mut native), "hello");
}
