/// A room column's refusal shape: a bordered, washed notice holding
/// one long wrapped line, then a Fill space, then the composer. The sidebar
/// beside it must keep its surface and rule, and the composer must sit at
/// the bottom of the column.
#[gpui_kit::test]
fn a_wrapped_notice_neither_starves_its_column_nor_its_neighbours_paint(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let refusal = "Couldn’t read this room: indexer: view: unknown field `viewer_handles`, \
         expected one of `channel_id`, `before_seq`, `limit` at line 1 column 118";
    let mut error_text_style = div().w_full().whitespace_normal();
    let error_text = wire::Node::Text {
        id: Some(named_id("error-text")),
        style: error_text_style.style().clone(),
        content: refusal.into(),
        heading: None,
        live: None,
    };
    let mut notice_style = div()
        .p_2()
        .bg(rgb(0x5b2430))
        .border(px(1.))
        .border_color(rgb(0xf06a6a))
        .rounded(px(4.));
    let notice = container_with_style("error", notice_style.style().clone(), [error_text]);
    let mut room_column = linear(
        "room-column",
        wire::Axis::Column,
        [
            wire::Node::Rule {
                key: "header-rule".into(),
                axis: wire::Axis::Row,
                thickness: 1.,
                color: Some(wire::Rgba([0.4, 0.4, 0.4, 1.])),
                weak: true,
                radius: None,
                snap: None,
            },
            notice,
            wire::Node::Space {
                width: None,
                height: Some(wire::Length::Fill),
            },
            sized(
                "composer",
                container("composer-content", [wire::Node::empty()]),
                Some(wire::Length::Fill),
                Some(wire::Length::Fixed(60.)),
            ),
        ],
    );
    if let wire::Node::Container { style, .. } = &mut room_column {
        *style = div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .gap(px(8.))
            .style()
            .clone();
    }
    let room = sized(
        "room",
        room_column,
        Some(wire::Length::Fill),
        Some(wire::Length::Fill),
    );
    let mut sidebar_style = div()
        .w(px(236.))
        .min_w(px(236.))
        .h_full()
        .min_h_0()
        .bg(rgb(0x252833))
        .overflow_hidden();
    let mut workspace_row = linear(
        "workspace-row",
        wire::Axis::Row,
        [
            container_with_style(
                "sidebar",
                sidebar_style.style().clone(),
                [wire::Node::empty()],
            ),
            wire::Node::Rule {
                key: "sidebar-resize".into(),
                axis: wire::Axis::Column,
                thickness: 1.,
                color: Some(wire::Rgba([0.4, 0.4, 0.4, 1.])),
                weak: true,
                radius: None,
                snap: None,
            },
            room,
        ],
    );
    if let wire::Node::Container { style, .. } = &mut workspace_row {
        *style = div()
            .flex()
            .flex_row()
            .w_full()
            .h_full()
            .gap(px(8.))
            .style()
            .clone();
    }
    let room = sized(
        "workspace",
        workspace_row,
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
    let card = sized(
        "float-card-box",
        container("float-card", [wire::Node::empty()]),
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
            wire::Node::Space {
                width: Some(wire::Length::Fill),
                height: Some(wire::Length::Fill),
            },
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

#[gpui_kit::test]
fn styled_container_uses_native_interactivity_and_typed_identity(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let mut base = div().w(px(120.)).h(px(40.)).bg(rgb(0x20242c));
    let mut hover = div().bg(rgb(0x303846));
    let mut active = div().bg(rgb(0x405060));
    let root = wire::Node::Container {
        id: Some(named_id("interactive")),
        style: base.style().clone(),
        interactivity: wire::Interactivity {
            role: Some(gpui_kit::Role::Button),
            aria: Default::default(),
            focusable: true,
            group: Some("card".into()),
            hover: Some(hover.style().clone()),
            active: Some(active.style().clone()),
            group_hover: Some(wire::GroupRefinement {
                group: "card".into(),
                style: hover.style().clone(),
            }),
            group_active: Some(wire::GroupRefinement {
                group: "card".into(),
                style: active.style().clone(),
            }),
            on_click: Some(42),
            ..Default::default()
        },
        children: vec![text("interactive-label", "Click")],
    };
    let window = cx.open_window(size(px(200.), px(100.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let handle = window.into();
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let mut native = gpui_kit::VisualTestContext::from_window(handle, cx);
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone());
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.click("interactive", cx));
    assert!(events
        .borrow()
        .iter()
        .any(|event| matches!(event, wire::Event::Click { handler: 42, event: wire::click::Click::Mouse { .. } })));
    assert!(tree.read_with(&native, |tree, _| tree.mounted.contains("interactive")));
}
