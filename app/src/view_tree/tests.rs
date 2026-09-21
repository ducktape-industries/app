use super::*;
use gpui_kit::test::TestWindowExt as _;

#[gpui_kit::test]
fn primitive_canvas_paints_in_the_first_frame_and_after_a_move(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let command = wire::CanvasCommand::Draw {
        shape: wire::CanvasShape::Rectangle {
            position: [10., 10.],
            size: [40., 30.],
            radius: [4.; 4],
        },
        fill: Some(wire::Rgba([1., 0., 0., 1.])),
        stroke: None,
        even_odd: false,
    };
    let node = wire::Node::Canvas {
        key: "canvas".into(),
        width: Some(wire::Length::Fixed(100.)),
        height: Some(wire::Length::Fixed(80.)),
        commands: vec![command.clone()],
    };
    let window = cx.open_window(size(px(100.), px(80.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert!(
            !window.painted_quads().is_empty(),
            "no asynchronous image load needed"
        );
        tree.update(cx, |tree, cx| {
            if let wire::Node::Canvas { commands, .. } = &mut tree.root
                && let wire::CanvasCommand::Draw {
                    shape: wire::CanvasShape::Rectangle { position, .. },
                    ..
                } = &mut commands[0]
            {
                *position = [40., 25.];
            }
            cx.notify();
        });
        window.draw(cx).clear(cx);
        assert!(
            !window.painted_quads().is_empty(),
            "moving keeps visible native geometry"
        );
    })
    .unwrap();
    let mut complex = vec![command];
    complex.push(wire::CanvasCommand::Pop);
    assert!(
        !native_canvas_commands(&complex),
        "transform stacks keep the complete SVG renderer"
    );
}

#[gpui_kit::test]
fn untinted_svg_uses_native_color_decoder_and_retains_pixels_without_resent_bytes(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let bytes = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><path fill="#ff0000" d="M0 0h12v24H0z"/><path fill="#0000ff" d="M12 0h12v24H12z"/></svg>"##.to_vec();
    let image = Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.clone()));
    let node = wire::Node::Svg {
        key: "artwork".into(),
        hash: 42,
        bytes: Some(bytes),
        inherit_button_ink: false,
        label: None,
        color: None,
        hover: None,
        fit: Some(wire::ContentFit::Contain),
        opacity: None,
        width: Some(wire::Length::Fixed(24.)),
        height: Some(wire::Length::Fixed(24.)),
    };
    let window = cx.open_window(size(px(80.), px(80.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    native.update(|window, cx| {
        assert!(
            gpui_kit::ImageSource::Image(image.clone()).is_asset_cached(cx),
            "the rendered SVG must request the native full-color image path"
        );
        let rendered = image
            .clone()
            .get_render_image(window, cx)
            .expect("native SVG decoded");
        let pixels = rendered.as_bytes(0).expect("decoded native pixels");
        assert!(
            pixels.chunks_exact(4).any(|p| p == [0, 0, 255, 255]),
            "red survives"
        );
        assert!(
            pixels.chunks_exact(4).any(|p| p == [255, 0, 0, 255]),
            "blue survives"
        );
        tree.update(cx, |tree, cx| {
            let mut next = tree.root.clone();
            if let wire::Node::Svg { bytes, .. } = &mut next {
                *bytes = None;
            }
            tree.replace(next, cx);
        });
        window.render_frame(cx);
        assert!(
            image.clone().get_render_image(window, cx).is_some(),
            "unchanged artwork remains available when a wire patch omits bytes"
        );
    });
}

#[gpui_kit::test]
fn container_focus_is_native_and_handoff_never_reuses_retired_handles(
    cx: &mut gpui_kit::TestAppContext,
) {
    struct Host {
        tree: Entity<ViewTree>,
        keys: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let keys = self.keys.clone();
            div()
                .capture_key_down(move |_, _, cx| {
                    keys.set(keys.get() + 1);
                    cx.stop_propagation();
                })
                .child(self.tree.clone())
        }
    }
    cx.update(gpui_kit::init);
    let menu = || {
        view_wire::kit::column(
            "menu",
            [view_wire::kit::text(
                "label",
                "A real menu, without an input",
            )],
        )
    };
    let keys = std::rc::Rc::new(std::cell::Cell::new(0));
    let window = cx.open_window(size(px(500.), px(300.)), |_, cx| Host {
        tree: cx.new(|_| ViewTree::new(menu())),
        keys: keys.clone(),
    });
    let host = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let tree = host.read_with(&native, |host, _| host.tree.clone());
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            assert!(tree.fields.is_empty());
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: "menu".into(),
                },
                window,
                cx,
            )
            .unwrap();
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    let retired = native.update(|window, cx| {
        tree.read_with(cx, |tree, cx| {
            assert!(tree.target_focused("menu", window, cx));
            tree.focus_targets["menu"].1.clone()
        })
    });
    native.update(|window, cx| {
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("escape").unwrap(), cx)
    });
    assert_eq!(
        keys.get(),
        1,
        "focused menu participates in native key capture"
    );
    native.update(|window, cx| {
        let saved = tree.read_with(cx, |tree, cx| tree.presentation(window, cx));
        host.update(cx, |host, cx| {
            host.tree = cx.new(|_| ViewTree::new(menu()).with_presentation(saved));
            cx.notify();
        });
    });
    native.update(|window, cx| window.render_frame(cx));
    let replacement = host.read_with(&native, |host, _| host.tree.clone());
    native.update(|window, cx| {
        replacement.read_with(cx, |tree, cx| {
            assert!(tree.target_focused("menu", window, cx));
            assert!(
                !retired.is_focused(window),
                "handoff uses a fresh native handle"
            );
        })
    });
    native.update(|window, cx| {
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("escape").unwrap(), cx)
    });
    assert_eq!(keys.get(), 2);
    native.update(|window, cx| {
        replacement.update(cx, |tree, cx| {
            tree.replace(view_wire::kit::text("closed", "menu closed"), cx);
            assert!(tree.focus_targets.is_empty());
            assert!(!tree.target_focused("menu", window, cx));
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| {
        retired.focus(window, cx);
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("escape").unwrap(), cx);
    });
    assert_eq!(
        keys.get(),
        2,
        "retired menu has no current native dispatch path"
    );
}

#[gpui_kit::test]
fn text_respects_parent_width_and_keeps_nowrap_inside_its_box(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let text = |key: &str, content: String, width, wrapping| wire::Node::Text {
        key: key.into(),
        heading: None,
        live: None,
        content,
        width,
        size: Some(14.),
        color: None,
        font: Default::default(),
        align_x: None,
        options: wire::TextOptions {
            wrapping,
            ..Default::default()
        },
    };
    let paragraph = text(
        "paragraph",
        "A long description with words that must wrap within the available parent width. "
            .repeat(8),
        None,
        None,
    );
    let row = wire::Node::Linear {
        key: "row".into(),
        axis: wire::Axis::Row,
        spacing: Some(8.),
        padding: None,
        width: Some(wire::Length::Fill),
        height: Some(wire::Length::Fill),
        background: None,
        border: None,
        align: None,
        max_width: None,
        clip: false,
        wrap: None,
        children: vec![
            text("height", "17968".into(), None, Some(wire::Wrapping::None)),
            text(
                "hash",
                "0123456789abcdef".repeat(16),
                Some(wire::Length::Fill),
                Some(wire::Wrapping::WordOrGlyph),
            ),
            text("count", "12 ops".into(), None, Some(wire::Wrapping::None)),
        ],
    };
    let mut header = row.clone();
    if let wire::Node::Linear { children, .. } = &mut header {
        *children = vec![
            text("label", "Header".into(), None, Some(wire::Wrapping::None)),
            wire::Node::Space {
                width: Some(wire::Length::Fill),
                height: None,
            },
            text(
                "actions",
                "New page".into(),
                Some(wire::Length::Fixed(100.)),
                Some(wire::Wrapping::None),
            ),
        ];
    }
    let mut reference = row.clone();
    if let wire::Node::Linear {
        children, height, ..
    } = &mut reference
    {
        *height = None;
        *children = vec![text(
            "reference",
            "Header".into(),
            None,
            Some(wire::Wrapping::None),
        )];
    }
    let root = wire::Node::Linear {
        key: "column".into(),
        axis: wire::Axis::Column,
        spacing: Some(8.),
        padding: None,
        width: Some(wire::Length::Fill),
        height: None,
        background: None,
        border: None,
        align: None,
        max_width: Some(620.),
        clip: false,
        wrap: None,
        children: vec![paragraph, row, reference, header],
    };
    let root = wire::Node::Button {
        key: "wrapping-parent".into(),
        role: None,
        selected: None,
        content: wire::ButtonContent::Child(Box::new(root)),
        label: None,
        checked: None,
        expanded: None,
        description: None,
        on_press: Some(1),
        width: Some(wire::Length::Fill),
        height: None,
        padding: None,
        style: Default::default(),
    };
    let window = cx.open_window(size(px(800.), px(500.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let button_bounds = native.update(|window, _| window.find("wrapping-parent").bounds());
    tree.read_with(&native, |tree, _| {
        let paragraph = tree.measured_bounds("paragraph").unwrap();
        let hash = tree.measured_bounds("hash").unwrap();
        assert!(
            button_bounds.bottom() >= hash.bottom(),
            "auto-height button must show every wrapped line"
        );
        let count = tree.measured_bounds("count").unwrap();
        assert!(tree.measured_bounds("height").unwrap().right() <= hash.left());
        assert_eq!(
            tree.measured_bounds("label").unwrap().size.width,
            tree.measured_bounds("reference").unwrap().size.width,
            "intrinsic labels cannot lose letters to a Fill spacer"
        );
        assert!(paragraph.size.width <= px(620.));
        assert!(
            paragraph.size.height > hash.size.height,
            "default wrapping creates multiple lines"
        );
        assert!(hash.right() <= count.left());
        assert!(
            hash.size.height > tree.measured_bounds("reference").unwrap().size.height,
            "WordOrGlyph must override a native Button's inherited nowrap"
        );
        assert!(count.right() <= paragraph.left() + px(620.));
    });
    let mut nowrap = text_options(
        div(),
        Default::default(),
        None,
        &wire::TextOptions {
            wrapping: Some(wire::Wrapping::None),
            ..Default::default()
        },
    );
    assert_eq!(nowrap.style().overflow.x, Some(gpui_kit::Overflow::Hidden));
    assert!(
        nowrap.text_style().text_overflow.is_some(),
        "the native text shaper must truncate glyphs, not only the containing box"
    );
}

#[gpui_kit::test]
fn horizontal_overflow_scrollbar_reveals_offscreen_columns(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::InputEvent as _;
    use view_wire::kit;
    cx.update(gpui_kit::init);
    let columns = kit::sized(
        kit::row(
            "columns",
            (0..4).map(|index| {
                kit::sized(
                    kit::container(
                        format!("column-{index}"),
                        kit::text(format!("name-{index}"), "Folder"),
                    ),
                    Some(wire::Length::Fixed(230.)),
                    Some(wire::Length::Fill),
                )
            }),
        ),
        Some(wire::Length::Fixed(920.)),
        Some(wire::Length::Fill),
    );
    let mut root = kit::scroll("folders", kit::spaced(columns, 0.));
    if let wire::Node::Scroll { direction, .. } = &mut root {
        *direction = wire::ScrollDirection::Horizontal;
    }
    let root = kit::sized(
        kit::column(
            "main",
            [
                kit::sized(
                    kit::container("header", kit::text("header-text", "Header")),
                    None,
                    Some(wire::Length::Fixed(24.)),
                ),
                root,
                kit::sized(
                    kit::container("footer", kit::text("footer-text", "Footer")),
                    None,
                    Some(wire::Length::Fixed(24.)),
                ),
            ],
        ),
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    let root = kit::spaced(root, 0.);
    let window = cx.open_window(size(px(400.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert_eq!(tree.scrolls["folders"].max_offset().x, px(520.));
        assert_eq!(tree.scrolls["folders"].bounds().size.height, px(152.));
        assert_eq!(
            tree.measured_bounds("column-0").unwrap().size.width,
            px(230.)
        );
    });
    native.update(|window, cx| window.render_frame(cx));
    let position = point(px(350.), px(168.));
    native.update(|window, cx| {
        window.dispatch_event(
            MouseDownEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                position,
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.scrolls["folders"].offset().x < px(-200.),
            "scrollbar track click reveals offscreen columns: bounds={:?}, offset={:?}",
            tree.scrolls["folders"].bounds(),
            tree.scrolls["folders"].offset()
        );
        assert_eq!(tree.scrolls["folders"].offset().y, px(0.));
    });
}

#[gpui_kit::test]
fn sensor_preserves_linear_fill_bounds(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = wire::Node::Sensor {
        key: "viewport".into(),
        reset: None,
        on_show: Some(1),
        on_resize: None,
        on_hide: None,
        anticipate: None,
        delay: None,
        child: Box::new(wire::Node::Linear {
            key: "content".into(),
            axis: wire::Axis::Column,
            width: Some(wire::Length::Fill),
            height: Some(wire::Length::Fill),
            max_width: None,
            clip: false,
            wrap: None,
            spacing: None,
            padding: None,
            align: None,
            background: None,
            border: None,
            children: vec![wire::Node::Space {
                width: Some(wire::Length::Fixed(20.)),
                height: Some(wire::Length::Fixed(5.)),
            }],
        }),
    };
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert_eq!(
            tree.sensors["viewport"].size,
            Some(size(px(400.), px(300.)))
        );
    });
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
    let sensor = |y, hide| wire::Node::Flex {
        key: "host".into(),
        layout: Default::default(),
        background: None,
        border: None,
        items: Vec::new(),
        children: vec![wire::Node::Pin {
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
    let root = wire::Node::Container {
        key: "card".into(),
        shadow: Default::default(),
        max_width: None,
        max_height: None,
        clip: false,
        width: Some(wire::Length::Fill),
        height: Some(wire::Length::Fill),
        padding: None,
        align_x: None,
        align_y: None,
        background: None,
        border: None,
        snap: None,
        content: Box::new(root),
    };
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
        key: "i".into(),
        placeholder: "Type here".into(),
        value: "hunter2".into(),
        on_input: 1,
        on_submit: None,
        width: None,
        secure,
        style: Default::default(),
    }
}

fn picture(label: Option<&str>) -> [wire::Node; 3] {
    let label = label.map(str::to_owned);
    [
        wire::Node::Image {
            key: "img".into(),
            hash: 1,
            data: None,
            label: label.clone(),
            fit: None,
            opacity: None,
            width: None,
            height: None,
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
            key: "svg".into(),
            inherit_button_ink: false,
            hash: 1,
            bytes: None,
            label,
            color: None,
            hover: None,
            fit: None,
            opacity: None,
            width: None,
            height: None,
        },
    ]
}

#[test]
fn text_is_a_label_its_content_names() {
    let text = |content: &str| wire::Node::Text {
        options: Default::default(),
        key: "t".into(),
        heading: None,
        live: None,
        content: content.into(),
        size: None,
        color: None,
        font: Default::default(),
        width: None,
        align_x: None,
    };
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

#[test]
fn a_button_reports_checked_expanded_and_its_description() {
    let mut node = button(wire::ButtonContent::Label("Bold".into()), None, Some(1));
    let wire::Node::Button {
        checked,
        expanded,
        description,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *checked = Some(true);
    *expanded = Some(false);
    *description = Some("Ctrl B".into());
    let heard = accessible(&node);
    assert_eq!(heard.toggled, Some(true));
    assert_eq!(heard.expanded, Some(false));
    assert_eq!(heard.description.as_deref(), Some("Ctrl B"));
    let wire::Node::Button { checked, .. } = &mut node else {
        unreachable!()
    };
    *checked = Some(false);
    assert_eq!(accessible(&node).toggled, Some(false));
}

#[test]
fn a_toggle_is_a_checkbox_or_a_switch_reporting_checked() {
    let toggle = |kind, label: &str, checked, on_toggle| wire::Node::Toggle {
        key: "t".into(),
        kind,
        label: label.into(),
        checked,
        on_toggle,
        width: None,
        style: Default::default(),
    };
    let checkbox = accessible(&toggle(wire::ToggleKind::Checkbox, "Notify", true, Some(1)));
    assert_eq!(
        checkbox,
        Accessible {
            role: Some(gpui_kit::Role::CheckBox),
            name: Some("Notify".into()),
            toggled: Some(true),
            ..Default::default()
        }
    );
    let switch = accessible(&toggle(wire::ToggleKind::Switch, "Mute", false, None));
    assert_eq!(switch.role, Some(gpui_kit::Role::Switch));
    assert_eq!(switch.toggled, Some(false));
    assert!(switch.disabled);
    assert_eq!(
        accessible(&toggle(wire::ToggleKind::Switch, "", false, None)).name,
        None
    );
}

#[test]
fn a_radio_reports_whether_it_is_the_selected_one() {
    let radio = |selected| wire::Node::Radio {
        key: "r".into(),
        label: "Weekly".into(),
        selected,
        on_select: 1,
        width: None,
        style: Default::default(),
    };
    assert_eq!(
        accessible(&radio(true)),
        Accessible {
            role: Some(gpui_kit::Role::RadioButton),
            name: Some("Weekly".into()),
            toggled: Some(true),
            ..Default::default()
        }
    );
    assert_eq!(accessible(&radio(false)).toggled, Some(false));
}

#[test]
fn a_slider_and_a_progress_report_their_value_in_range() {
    let slider = wire::Node::Slider {
        key: "s".into(),
        label: None,
        value: 30.,
        min: 0.,
        max: 100.,
        step: 5.,
        on_change: 1,
        on_release: None,
        axis: wire::Axis::Row,
        width: None,
        height: None,
        style: Default::default(),
    };
    assert_eq!(
        accessible(&slider),
        Accessible {
            role: Some(gpui_kit::Role::Slider),
            numeric: Some(30.),
            min: Some(0.),
            max: Some(100.),
            step: Some(5.),
            ..Default::default()
        }
    );
    let progress = wire::Node::Progress {
        key: "p".into(),
        value: 0.25,
        min: 0.,
        max: 1.,
        axis: wire::Axis::Row,
        length: None,
        girth: None,
        tone: None,
        background: None,
        bar: None,
        border: None,
    };
    assert_eq!(
        accessible(&progress),
        Accessible {
            role: Some(gpui_kit::Role::ProgressIndicator),
            numeric: Some(0.25),
            min: Some(0.),
            max: Some(1.),
            ..Default::default()
        }
    );
}

#[test]
fn an_input_is_named_by_its_label_and_a_secure_one_never_reports_its_value() {
    assert_eq!(
        accessible(&input("Room name", false, false)),
        Accessible {
            role: Some(gpui_kit::Role::TextInput),
            name: Some("Room name".into()),
            description: Some("Shown to members".into()),
            value: Some("hunter2".into()),
            ..Default::default()
        }
    );
    let secure = accessible(&input("Password", true, true));
    assert_eq!(secure.role, Some(gpui_kit::Role::PasswordInput));
    assert_eq!(secure.value, None);
    assert!(secure.disabled);
    // the placeholder is not a name
    assert_eq!(accessible(&input("", false, false)).name, None);
}

#[test]
fn an_editor_is_named_by_its_label_and_described_by_its_placeholder_without_one() {
    let mut editor = wire::Node::Editor {
        options: Box::default(),
        key: "e".into(),
        label: None,
        placeholder: "Write something".into(),
        document: wire::editor_document::EditorDocumentRef {
            document: "e".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
        width: None,
        height: None,
        min_height: None,
        max_height: None,
    };
    assert_eq!(
        accessible(&editor),
        Accessible {
            role: Some(gpui_kit::Role::MultilineTextInput),
            description: Some("Write something".into()),
            ..Default::default()
        }
    );
    let wire::Node::Editor { label, .. } = &mut editor else {
        unreachable!()
    };
    *label = Some("Message".into());
    let named = accessible(&editor);
    assert_eq!(named.name.as_deref(), Some("Message"));
    // the placeholder is not a name, nor a second one
    assert_eq!(named.description, None);
}

/// #139: the field reads the text it holds, and one that takes no typing
/// says so and is offered none — the door reads the tree a screen reader
/// does.
#[gpui_kit::test]
fn a_read_only_editor_reads_its_text_and_is_offered_no_typing(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let words = "no channel is open";
    for editable in [true, false] {
        let root = wire::Node::Editor {
            key: "composer".into(),
            label: Some("Message".into()),
            options: Box::default(),
            placeholder: String::new(),
            document: wire::editor_document::EditorDocumentRef {
                document: "composer".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: Default::default(),
                byte_len: words.len() as u32,
            },
            on_document: 0,
            editable,
            width: None,
            height: None,
            min_height: None,
            max_height: None,
        };
        let store = crate::editor::wire::EditorStore::new(91);
        store.replace(&root).unwrap();
        seed_editor_text(&store, words);
        let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
            let mut tree = ViewTree::new(root);
            tree.set_editor_store(store, cx);
            tree
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            crate::ax_door::snapshot("t", window, false)
        });
        let field = nodes
            .iter()
            .find(|node| node.role == "MultilineTextInput")
            .expect("the editor's field is in the tree");
        assert_eq!(field.value.as_deref(), Some(words), "editable: {editable}");
        match editable {
            true => {
                assert!(!field.state.contains(&"disabled"));
                assert!(field.actions.contains(&"type"));
                assert!(field.actions.contains(&"set_value"));
            }
            false => {
                assert!(field.state.contains(&"disabled"));
                assert!(field.actions.is_empty(), "{:?}", field.actions);
            }
        }
    }
}

#[test]
fn a_picker_reports_the_chosen_option_as_its_value() {
    let options = vec!["Low".to_owned(), "High".to_owned()];
    let combo = |selected| wire::Node::ComboBox {
        key: "c".into(),
        label: None,
        state_key: "c".into(),
        options: options.clone(),
        selected,
        reset: 0,
        placeholder: "Priority".into(),
        on_select: 1,
        width: None,
        settings: Box::default(),
    };
    let pick = |selected| wire::Node::PickList {
        settings: Box::default(),
        key: "p".into(),
        label: None,
        options: options.clone(),
        selected,
        placeholder: Some("Priority".into()),
        on_select: 1,
        width: None,
        style: Default::default(),
    };
    for node in [combo(Some(1)), pick(Some(1))] {
        assert_eq!(
            accessible(&node),
            Accessible {
                role: Some(gpui_kit::Role::ComboBox),
                value: Some("High".into()),
                ..Default::default()
            }
        );
    }
    for node in [combo(None), pick(None), combo(Some(7)), pick(Some(7))] {
        assert_eq!(accessible(&node).value, None);
    }
    for mut node in [combo(Some(1)), pick(Some(1))] {
        assert_eq!(accessible(&node).name, None);
        let (wire::Node::ComboBox { label, .. } | wire::Node::PickList { label, .. }) = &mut node
        else {
            unreachable!()
        };
        *label = Some("Priority".into());
        assert_eq!(accessible(&node).name.as_deref(), Some("Priority"));
    }
}

#[test]
fn a_labelled_picture_is_an_image_and_an_unlabelled_one_is_decoration() {
    for node in picture(Some("Ada's avatar")) {
        assert_eq!(
            accessible(&node),
            Accessible {
                role: Some(gpui_kit::Role::Image),
                name: Some("Ada's avatar".into()),
                ..Default::default()
            }
        );
    }
    for node in picture(None).into_iter().chain(picture(Some(""))) {
        assert_eq!(accessible(&node), Accessible::default());
    }
}

#[test]
fn layout_is_not_in_the_accessibility_tree() {
    let space = wire::Node::Space {
        width: None,
        height: None,
    };
    assert_eq!(accessible(&space), Accessible::default());
}

#[test]
fn a_slider_is_named_by_its_label() {
    let slider = |label: Option<&str>| wire::Node::Slider {
        key: "s".into(),
        label: label.map(str::to_owned),
        value: 1.,
        min: 0.,
        max: 2.,
        step: 1.,
        on_change: 1,
        on_release: None,
        axis: wire::Axis::Row,
        width: None,
        height: None,
        style: Default::default(),
    };
    assert_eq!(
        accessible(&slider(Some("Volume"))).name.as_deref(),
        Some("Volume")
    );
    assert_eq!(accessible(&slider(Some(""))).name, None);
    assert_eq!(accessible(&slider(None)).name, None);
}

#[test]
fn a_text_heading_has_its_level_and_a_live_text_its_politeness() {
    let text = |heading, live| wire::Node::Text {
        key: "t".into(),
        content: "Members".into(),
        width: None,
        size: None,
        color: None,
        font: Default::default(),
        align_x: None,
        options: Default::default(),
        heading,
        live,
    };
    for level in 1..=6u8 {
        assert_eq!(
            accessible(&text(Some(level), None)),
            Accessible {
                role: Some(gpui_kit::Role::Heading),
                name: Some("Members".into()),
                level: Some(level.into()),
                ..Default::default()
            }
        );
    }
    for live in [wire::Live::Polite, wire::Live::Assertive] {
        assert_eq!(
            accessible(&text(None, Some(live))),
            Accessible {
                role: Some(gpui_kit::Role::Label),
                name: Some("Members".into()),
                live: Some(live),
                ..Default::default()
            }
        );
    }
}

/// Every role a view can give a mouse area or a button, and what
/// assistive technology hears for it.
const ROLES: [(wire::Role, gpui_kit::Role); 7] = [
    (wire::Role::Button, gpui_kit::Role::Button),
    (wire::Role::Link, gpui_kit::Role::Link),
    (wire::Role::Tab, gpui_kit::Role::Tab),
    (wire::Role::MenuItem, gpui_kit::Role::MenuItem),
    (wire::Role::Row, gpui_kit::Role::Row),
    (wire::Role::Checkbox, gpui_kit::Role::CheckBox),
    (wire::Role::Switch, gpui_kit::Role::Switch),
];

#[test]
fn a_button_with_a_role_is_that_role_and_reports_selected() {
    for (role, heard) in ROLES {
        let mut tab = button(wire::ButtonContent::Label("Tab".into()), None, Some(1));
        let wire::Node::Button {
            role: set,
            selected,
            ..
        } = &mut tab
        else {
            unreachable!()
        };
        (*set, *selected) = (Some(role), Some(true));
        let tab = accessible(&tab);
        assert_eq!(tab.role, Some(heard));
        assert_eq!(tab.selected, Some(true));
        assert_eq!(tab.name.as_deref(), Some("Tab"));
    }
}

#[test]
fn a_mouse_area_is_announced_only_as_the_role_its_view_gives_it() {
    let area = |role, label: Option<&str>| wire::Node::MouseArea {
        key: "m".into(),
        role,
        label: label.map(str::to_owned),
        expanded: Some(true),
        selected: Some(false),
        checked: Some(true),
        on_press: Some(1),
        on_release: None,
        on_double_click: None,
        on_right_press: None,
        on_right_release: None,
        on_middle_press: None,
        on_middle_release: None,
        on_enter: None,
        on_exit: None,
        on_move: None,
        on_press_at: None,
        on_scroll: None,
        content: Box::new(wire::Node::empty()),
    };
    for (role, heard) in ROLES {
        assert_eq!(
            accessible(&area(Some(role), Some("Wrap lines"))),
            Accessible {
                role: Some(heard),
                name: Some("Wrap lines".into()),
                toggled: Some(true),
                expanded: Some(true),
                selected: Some(false),
                ..Default::default()
            }
        );
        assert_eq!(accessible(&area(Some(role), Some(""))).name, None);
    }
    assert_eq!(
        accessible(&area(None, Some("Wrap lines"))),
        Accessible::default()
    );
}

#[test]
fn a_named_overlay_is_a_dialog_and_an_unnamed_one_is_layout() {
    let overlay = |label: Option<&str>, open| wire::Node::Overlay {
        key: "o".into(),
        label: label.map(str::to_owned),
        padding: 0.,
        backdrop: wire::Rgba([0.; 4]),
        align_x: wire::AlignX::Center,
        align_y: wire::AlignY::Center,
        on_dismiss: None,
        children: if open {
            vec![wire::Node::empty(), wire::Node::empty()]
        } else {
            Vec::new()
        },
    };
    assert_eq!(
        accessible(&overlay(Some("Rename channel"), true)),
        Accessible {
            role: Some(gpui_kit::Role::Dialog),
            name: Some("Rename channel".into()),
            ..Default::default()
        }
    );
    assert_eq!(
        accessible(&overlay(Some("Rename channel"), false)),
        Accessible::default()
    );
    assert_eq!(accessible(&overlay(None, true)), Accessible::default());
}

/// A room column's refusal shape: a bordered, washed notice holding
/// one long wrapped line, then a Fill space, then the composer. The sidebar
/// beside it must keep its surface and rule, and the composer must sit at
/// the bottom of the column.
#[gpui_kit::test]
fn a_wrapped_notice_neither_starves_its_column_nor_its_neighbours_paint(
    cx: &mut gpui_kit::TestAppContext,
) {
    use view_wire::kit;
    cx.update(gpui_kit::init);
    let refusal = "Couldn’t read this room: indexer: view: unknown field `viewer_handles`, \
         expected one of `channel_id`, `before_seq`, `limit` at line 1 column 118";
    let room = kit::sized(
        kit::column(
            "room",
            [
                kit::divider("header-rule"),
                kit::notice(
                    "error",
                    kit::wrapping(kit::text("error-text", refusal)),
                    kit::Tone::Danger,
                ),
                kit::space(None, Some(wire::Length::Fill)),
                kit::sized(
                    kit::container("composer", wire::Node::empty()),
                    Some(wire::Length::Fill),
                    Some(wire::Length::Fixed(60.)),
                ),
            ],
        ),
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    let room = kit::sized(
        kit::row(
            "workspace",
            [
                kit::pane("sidebar", wire::Node::empty(), wire::Length::Fixed(236.)),
                kit::vertical_divider("sidebar-resize"),
                room,
            ],
        ),
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    // A room's real root: a viewport sensor around the press area.
    let root = wire::Node::Sensor {
        key: "viewport".into(),
        reset: None,
        on_show: None,
        on_resize: Some(1),
        on_hide: None,
        anticipate: None,
        delay: None,
        child: Box::new(wire::Node::MouseArea {
            key: "press-area".into(),
            role: None,
            label: None,
            expanded: None,
            selected: None,
            checked: None,
            on_press: Some(2),
            on_release: None,
            on_double_click: None,
            on_right_press: None,
            on_right_release: None,
            on_middle_press: None,
            on_middle_release: None,
            on_enter: None,
            on_exit: None,
            on_move: None,
            on_press_at: None,
            on_scroll: None,
            content: Box::new(room),
        }),
    };
    // Mounted the way a module seat mounts a guest: a cached view under a
    // full-size div, not as the window root (which gpui stretches).
    struct Seat(Entity<ViewTree>);
    impl Render for Seat {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.0
                    .clone()
                    .cached(gpui_kit::StyleRefinement::default().size_full()),
            )
        }
    }
    let window = cx.open_window(size(px(800.), px(600.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(root)))
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    let bounds = |key: &str| {
        tree.read_with(&native, |tree, _| tree.measured_bounds(key))
            .unwrap_or_else(|| panic!("{key} was measured"))
    };
    let (sidebar, error, composer) = (bounds("sidebar"), bounds("error"), bounds("composer"));
    assert_eq!(sidebar.size, size(px(236.), px(600.)), "{sidebar:?}");
    assert!(
        error.size.height < px(200.) && error.size.height > px(20.),
        "the notice wraps to a few lines: {error:?}"
    );
    assert_eq!(
        composer.origin.y,
        px(540.),
        "the Fill space pushes the composer to the bottom: {composer:?}"
    );
    native.update(|window, cx| {
        window.render_frame(cx);
        let quads = window.painted_quads();
        let scale = window.scale_factor();
        let surface = |w: f32, h: f32| {
            quads
                .iter()
                .filter(|quad| {
                    quad.bounds.size.width.as_f32() == w * scale
                        && quad.bounds.size.height.as_f32() == h * scale
                })
                .count()
        };
        assert_eq!(surface(236., 600.), 1, "the sidebar surface: {quads:?}");
        let rules = quads
            .iter()
            .filter(|quad| quad.bounds.size.height.as_f32() == 600. * scale)
            .filter(|quad| quad.bounds.size.width.as_f32() <= scale)
            .count();
        assert_eq!(rules, 1, "the sidebar-resize rule: {quads:?}");
    });
}

#[gpui_kit::test]
fn a_float_modal_uses_viewport_coordinates_and_one_surface(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let card = wire::kit::sized(
        wire::kit::container("float-card", wire::Node::empty()),
        Some(wire::Length::Fixed(40.)),
        Some(wire::Length::Fixed(20.)),
    );
    let root = wire::Node::Overlay {
        key: "float-overlay".into(),
        label: Some("Context menu".into()),
        padding: 30.,
        backdrop: wire::Rgba([0.; 4]),
        align_x: wire::AlignX::Right,
        align_y: wire::AlignY::Bottom,
        on_dismiss: None,
        children: vec![
            wire::kit::spacer(),
            wire::Node::Float {
                key: "float".into(),
                x: 37.,
                y: 29.,
                scale: 1.,
                shadow: Default::default(),
                radius: None,
                content: Box::new(card),
            },
        ],
    };
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let card = tree
        .read_with(&native, |tree, _| tree.measured_bounds("float-card"))
        .expect("the floated card was measured");
    assert_eq!(card.origin, point(px(37.), px(29.)));
    native.update(|window, cx| {
        window.render_frame(cx);
        let quads = window.painted_quads();
        let scale = window.scale_factor();
        let card_surfaces = quads
            .iter()
            .filter(|quad| {
                quad.bounds.size.width.as_f32() == 40. * scale
                    && quad.bounds.size.height.as_f32() == 20. * scale
            })
            .count();
        assert_eq!(
            card_surfaces, 1,
            "the Float owns the card surface: {quads:?}"
        );
    });
}
