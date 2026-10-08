use super::*;

#[gpui_kit::test]
fn primitive_canvas_paints_in_the_first_frame_and_after_a_move(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let command = wire::CanvasCommand::Draw {
        shape: wire::CanvasShape::Rectangle {
            position: [10., 10.],
            size: [40., 30.],
            radius: [4.; 4],
        },
        fill: Some(gpui_kit::rgb(0xff0000).into()),
        stroke: None,
        even_odd: false,
    };
    let mut canvas_style = div().w(px(100.)).h(px(80.));
    let node = wire::Node::Canvas {
        commands: vec![command.clone()],
        style: crate::render::test_style(canvas_style.style().clone()),
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
    let menu = || container("menu", [text("label", "A real menu, without an input")]);
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
            let target = vec![named_id("menu")];
            tree.execute_widget_command(wire::WidgetCommand::Focus { target }, window, cx)
                .unwrap();
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    let retired = native.update(|window, cx| {
        tree.read_with(cx, |tree, cx| {
            let target = vec![named_id("menu")];
            assert!(tree.target_focused(&target, window, cx));
            tree.focus_targets[&target].1.clone()
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
            assert!(tree.target_focused(&[named_id("menu")], window, cx));
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
            tree.replace(text("closed", "menu closed"), &[], cx);
            assert!(tree.focus_targets.is_empty());
            assert!(!tree.target_focused(&[named_id("menu")], window, cx));
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
    let text = |key: &str, content: String, width, nowrap| {
        let mut element = div().min_w_0().max_w_full().text_size(px(14.));
        element.style().size.width = width;
        element = if nowrap {
            element.truncate().flex_shrink_0()
        } else {
            element.whitespace_normal()
        };
        wire::Node::Text(view_wire::TextNode {
            id: Some(named_id(key)),
            style: crate::render::test_style(element.style().clone()),
            content,
        })
    };
    let paragraph = text(
        "paragraph",
        "A long description with words that must wrap within the available parent width. "
            .repeat(8),
        None,
        false,
    );
    let row = axis_container(
        "row",
        Axis::Row,
        [
            text("height", "17968".into(), None, true),
            text("hash", "0123456789abcdef".repeat(16), Some(fill()), false),
            text("count", "12 ops".into(), None, true),
        ],
    );
    let mut header = row.clone();
    if let wire::Node::Container(view_wire::ContainerNode { children, .. }) = &mut header {
        *children = vec![
            text("label", "Header".into(), None, true),
            wire::Node::Container(view_wire::ContainerNode {
                id: None,
                style: crate::render::test_style(sized_style(Some(fill()), None)),
                interactivity: Default::default(),
                children: vec![],
            }),
            text("actions", "New page".into(), Some(fixed(100.)), true),
        ];
    }
    let mut reference = row.clone();
    if let wire::Node::Container(view_wire::ContainerNode { children, .. }) = &mut reference {
        *children = vec![text("reference", "Header".into(), None, true)];
    }
    let mut root = axis_container("column", Axis::Column, [paragraph, row, reference, header]);
    if let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut root {
        let mut root_style = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .min_h_0()
            .gap(px(8.))
            .max_w(px(620.));
        *style = crate::render::test_style(root_style.style().clone());
    }
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("wrapping-parent")),
        style: crate::render::test_style(div().w_full().style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            on_click: Some(1),
            ..Default::default()
        })),
        children: vec![root],
    });
    let window = cx.open_window(size(px(800.), px(500.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, _| {
        let bounds = |key: &'static str| window.find(key).bounds();
        let button_bounds = bounds("wrapping-parent");
        let (paragraph, hash, count) = (bounds("paragraph"), bounds("hash"), bounds("count"));
        assert!(
            button_bounds.bottom() >= hash.bottom(),
            "an auto-height clickable must show every wrapped line"
        );
        assert!(bounds("height").right() <= hash.left());
        assert_eq!(
            bounds("label").size.width,
            bounds("reference").size.width,
            "intrinsic labels cannot lose letters to a Fill spacer"
        );
        assert!(paragraph.size.width <= px(620.));
        assert!(
            paragraph.size.height > hash.size.height,
            "default wrapping creates multiple lines"
        );
        assert!(hash.right() <= count.left());
        assert!(
            hash.size.height > bounds("reference").size.height,
            "WordOrGlyph must override a native Button's inherited nowrap"
        );
        assert!(count.right() <= paragraph.left() + px(620.));
    });
    let mut nowrap = div().truncate();
    assert_eq!(nowrap.style().overflow.x, Some(gpui_kit::Overflow::Hidden));
    assert!(
        nowrap.text_style().text_overflow.is_some(),
        "the native text shaper must truncate glyphs, not only the containing box"
    );
}

#[gpui_kit::test]
fn sensor_preserves_linear_fill_bounds(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = wire::Node::Sensor {
        id: named_id("viewport"),
        on_show: Some(1),
        on_resize: None,
        child: Box::new({
            let mut content = axis_container(
                "content",
                Axis::Column,
                [wire::Node::Container(view_wire::ContainerNode {
                    id: None,
                    style: crate::render::test_style(sized_style(
                        Some(fixed(20.)),
                        Some(fixed(5.)),
                    )),
                    interactivity: Default::default(),
                    children: vec![],
                })],
            );
            if let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut content {
                *style = crate::render::test_style(
                    div().flex().flex_col().w_full().h_full().style().clone(),
                );
            }
            content
        }),
        style: crate::render::test_style(sized_style(Some(fill()), Some(fill()))),
    };
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert_eq!(
            tree.sensors[&vec![named_id("viewport")]].size,
            Some(size(px(400.), px(300.)))
        );
    });
}

/// A scrolling container keeps its handle at its own id: the offset holds
/// across frames that insert a sibling before it, and goes with it. One
/// without an id keeps none — its path is its parent's, shared with any
/// sibling scroller there.
#[gpui_kit::test]
fn a_scrolling_container_keeps_its_handle_at_its_own_id(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let scroll_style = || {
        let mut style = div().flex().flex_col().style().clone();
        style.overflow.y = Some(gpui_kit::Overflow::Scroll);
        style.size.width = Some(fixed(300.));
        style.size.height = Some(fixed(200.));
        style.min_size.height = Some(fixed(200.));
        style
    };
    let rows = |tag: &str, n: usize| -> Vec<wire::Node> {
        (0..n)
            .map(|i| {
                sized(
                    &format!("{tag}-row-{i}"),
                    text(&format!("{tag}-t-{i}"), "x"),
                    Some(fixed(300.)),
                    Some(fixed(100.)),
                )
            })
            .collect()
    };
    let banner = || {
        sized(
            "banner",
            text("bt", "b"),
            Some(fixed(300.)),
            Some(fixed(50.)),
        )
    };
    let root = |with_banner: bool, n: usize| {
        container(
            "main",
            with_banner
                .then(banner)
                .into_iter()
                .chain([container_with_style("list", scroll_style(), rows("a", n))]),
        )
    };
    let window = cx.open_window(size(px(400.), px(600.)), |_, _| {
        ViewTree::new(root(false, 10))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let path = vec![named_id("main"), named_id("list")];
    assert_eq!(
        native.update(|window, _| window.find("list").bounds().size),
        size(px(300.), px(200.)),
        "the bar leaves the scroller's layout alone"
    );
    tree.read_with(&native, |tree, _| {
        assert_eq!(tree.scrolls[&path].max_offset().y, px(800.));
        tree.scrolls[&path].set_offset(point(px(0.), px(-300.)));
    });
    native.update(|window, cx| window.render_frame(cx));
    tree.update(&mut native, |tree, cx| {
        tree.replace(root(true, 12), &[], cx)
    });
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert_eq!(tree.scrolls[&path].offset().y, px(-300.));
        assert_eq!(tree.scrolls[&path].max_offset().y, px(1000.));
    });
    tree.update(&mut native, |tree, cx| {
        tree.replace(container("main", [banner()]), &[], cx)
    });
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.scrolls.is_empty(),
            "a removed scroller's handle is dropped"
        );
    });
    let anonymous = |tag: &str, n: usize| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::test_style(scroll_style()),
            interactivity: Default::default(),
            children: rows(tag, n),
        })
    };
    tree.update(&mut native, |tree, cx| {
        tree.replace(
            container("main", [anonymous("b", 10), anonymous("c", 20)]),
            &[],
            cx,
        )
    });
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        assert!(tree.scrolls.is_empty(), "id-less scrollers share no handle");
    });
}
