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
