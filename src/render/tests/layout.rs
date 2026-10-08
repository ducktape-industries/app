use super::*;

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
    let error_text = wire::Node::Text(view_wire::TextNode {
        id: Some(named_id("error-text")),
        style: crate::render::test_style(error_text_style.style().clone()),
        content: refusal.into(),
    });
    let mut notice_style = div()
        .p_2()
        .bg(rgb(0x5b2430))
        .border(px(1.))
        .border_color(rgb(0xf06a6a))
        .rounded(px(4.));
    let notice = container_with_style("error", notice_style.style().clone(), [error_text]);
    let mut room_column = axis_container(
        "room-column",
        Axis::Column,
        [
            rule("header-rule", Axis::Row),
            notice,
            wire::Node::Container(view_wire::ContainerNode {
                id: None,
                style: crate::render::test_style(sized_style(None, Some(fill()))),
                interactivity: Default::default(),
                children: vec![],
            }),
            sized(
                "composer",
                container("composer-content", [wire::Node::empty()]),
                Some(fill()),
                Some(fixed(60.)),
            ),
        ],
    );
    if let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut room_column {
        *style = crate::render::test_style(
            div()
                .flex()
                .flex_col()
                .w_full()
                .h_full()
                .gap(px(8.))
                .style()
                .clone(),
        );
    }
    let room = sized("room", room_column, Some(fill()), Some(fill()));
    let mut sidebar_style = div()
        .w(px(236.))
        .min_w(px(236.))
        .h_full()
        .min_h_0()
        .bg(rgb(0x252833))
        .overflow_hidden();
    let mut workspace_row = axis_container(
        "workspace-row",
        Axis::Row,
        [
            container_with_style(
                "sidebar",
                sidebar_style.style().clone(),
                [wire::Node::empty()],
            ),
            rule("sidebar-resize", Axis::Column),
            room,
        ],
    );
    if let wire::Node::Container(view_wire::ContainerNode { style, .. }) = &mut workspace_row {
        *style = crate::render::test_style(
            div()
                .flex()
                .flex_row()
                .w_full()
                .h_full()
                .gap(px(8.))
                .style()
                .clone(),
        );
    }
    let room = sized("workspace", workspace_row, Some(fill()), Some(fill()));
    // A room's real root: a viewport sensor around the press area.
    let root = wire::Node::Sensor {
        id: named_id("viewport"),
        on_show: None,
        on_resize: Some(1),
        style: crate::render::test_style(sized_style(Some(fill()), Some(fill()))),
        child: Box::new(wire::Node::Container(view_wire::ContainerNode {
            id: Some(named_id("press-area")),
            style: crate::render::test_style(sized_style(Some(fill()), Some(fill()))),
            interactivity: Some(Box::new(wire::Interactivity {
                on_click: Some(2),
                ..Default::default()
            })),
            children: vec![room],
        })),
    };
    let window = cx.open_window(size(px(800.), px(600.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(root)))
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    let mut bounds = |key: &'static str| native.update(|window, _| window.find(key).bounds());
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
fn styled_container_uses_native_interactivity_and_typed_identity(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let mut base = div().w(px(120.)).h(px(40.)).bg(rgb(0x20242c));
    let mut hover = div().bg(rgb(0x303846));
    let mut active = div().bg(rgb(0x405060));
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("interactive")),
        style: crate::render::test_style(base.style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            role: Some(gpui_kit::Role::Button),
            aria: Default::default(),
            focusable: true,
            group: Some("card".into()),
            hover: Some(crate::render::test_style(hover.style().clone())),
            active: Some(crate::render::test_style(active.style().clone())),
            group_hover: Some(wire::GroupRefinement {
                group: "card".into(),
                style: crate::render::test_style(hover.style().clone()),
            }),
            group_active: Some(wire::GroupRefinement {
                group: "card".into(),
                style: crate::render::test_style(active.style().clone()),
            }),
            on_click: Some(42),
            ..Default::default()
        })),
        children: vec![text("interactive-label", "Click")],
    });
    let window = cx.open_window(size(px(200.), px(100.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let handle = window.into();
    let mut native = gpui_kit::VisualTestContext::from_window(handle, cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.click("interactive", cx));
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        wire::Event::Click {
            handler: 42,
            event: wire::click::Click::Mouse { .. }
        }
    )));
    // gpui files the label under the id the view gave the container
    let label = native.update(|window, _| window.find("interactive-label").path().to_vec());
    assert!(
        label.ends_with(&["interactive".into(), "interactive-label".into()]),
        "{label:?}"
    );
}

#[gpui_kit::test]
fn container_interactivity_emits_native_pointer_and_key_payloads(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("events")),
        style: crate::render::test_style(sized_style(Some(fixed(120.)), Some(fixed(60.)))),
        interactivity: Some(Box::new(wire::Interactivity {
            focusable: true,
            on_mouse_down: Some(10),
            capture_mouse_down: Some(11),
            on_key_down: Some(12),
            ..Default::default()
        })),
        children: vec![text("event-label", "Events")],
    });
    let window = cx.open_window(size(px(200.), px(100.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.click("events", cx));
    native.update(|window, cx| {
        let modifiers = gpui_kit::Modifiers {
            shift: true,
            ..Default::default()
        };
        window.dispatch_event(
            MouseDownEvent {
                position: point(px(20.), px(20.)),
                button: MouseButton::Left,
                modifiers,
                click_count: 3,
                first_mouse: true,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            KeyDownEvent {
                keystroke: Keystroke::parse("shift-a").unwrap(),
                is_held: true,
                prefer_character_input: true,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    let events = events.borrow();
    assert!(events.iter().any(|event| matches!(
        event,
        wire::Event::MouseDown {
            handler: 10,
            phase: wire::DispatchPhase::Bubble,
            event: wire::interactivity::MouseDown {
                button: wire::click::MouseButton::Left,
                click_count: 3,
                first_mouse: true,
                modifiers: gpui_kit::Modifiers { shift: true, .. },
                ..
            }
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        wire::Event::MouseDown {
            handler: 11,
            phase: wire::DispatchPhase::Capture,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        wire::Event::KeyDown {
            handler: 12,
            event: wire::interactivity::KeyDown {
                repeat: true,
                prefer_character_input: true,
                ..
            },
            ..
        }
    )));
}

/// A guest tree that did not change is not drawn again: the seat's cache
/// holds from one frame to the next. An id-less box measured on its named
/// ancestor's path, and a paragraph's selection refreshed the window each
/// time a cached frame swept it, so every guest on the desk was drawn in
/// full on every frame. The named box keeps its own bounds.
#[gpui_kit::test]
fn an_unchanged_guest_tree_is_not_drawn_again(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let bare = |children: Vec<wire::Node>| {
        wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::test_style(div().p_2().style().clone()),
            interactivity: Default::default(),
            children,
        })
    };
    let paragraph = wire::Node::RichText {
        id: Some(named_id("line")),
        style: crate::render::test_style(div().h(px(20.)).style().clone()),
        text: "a line".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    };
    let root = container_with_style(
        "card",
        div().w(px(200.)).h(px(100.)).style().clone(),
        [bare(vec![bare(vec![paragraph])])],
    );
    struct Seat(Entity<ViewTree>);
    impl Render for Seat {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(gpui_kit::base::TextSelectionLayer)
                .child(
                    self.0
                        .clone()
                        .cached(gpui_kit::StyleRefinement::default().size_full()),
                )
        }
    }
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(root)))
    });
    let seat = window.root(cx).unwrap();
    let tree = seat.read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let frame = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            // the desk draws its seats again: `render_frame` would refresh
            seat.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        native.run_until_parked();
    };
    for _ in 0..3 {
        frame(&mut native);
    }
    let settled = tree.read_with(&native, |tree, _| tree.renders);
    for _ in 0..5 {
        frame(&mut native);
    }
    assert_eq!(tree.read_with(&native, |tree, _| tree.renders), settled);
    let card = native.update(|window, _| window.find("card").bounds());
    assert_eq!(card.size, size(px(200.), px(100.)));
}

/// A list box in a plain scroller whose `claim`ed row is its active
/// descendant: ten rows of 40 px in 100 px.
fn claiming_list(claim: usize) -> wire::Node {
    let rows = (0..10).map(|n| {
        let mut row = container_with_style(
            &format!("row-{n}"),
            div().h(px(40.)).flex_shrink_0().style().clone(),
            [],
        );
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut row {
            let interactivity = interactivity.get_or_insert_default();
            interactivity.role = Some(gpui_kit::Role::ListBoxOption);
            interactivity.aria.label = Some(format!("Row {n}").into());
            interactivity.aria.selected = Some(false);
            interactivity.aria.active_descendant = n == claim;
        }
        row
    });
    let mut style = div()
        .flex()
        .flex_col()
        .w(px(200.))
        .h(px(100.))
        .style()
        .clone();
    style.overflow.y = Some(gpui_kit::Overflow::Scroll);
    let mut list = container_with_style("list", style, rows);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut list {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(gpui_kit::Role::ListBox);
        interactivity.aria.label = Some("Rows".into());
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
    }
    list
}

/// A view opens on a claimed row below the fold (the open room), and its
/// arrows move the claim; the host scrolls the plain scroller around it by
/// the least that shows the claimed row, as it opens, up to a row above the
/// fold and back down to one below it, and leaves the offset alone while
/// the claim stays (the wheel may move it).
#[gpui_kit::test]
fn a_claimed_row_below_the_fold_is_scrolled_into_view(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(claiming_list(7))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the scroller's offset, and the claimed row's top and bottom inside it
    let shown = |native: &mut gpui_kit::VisualTestContext, claim: usize| {
        native.update(|window, cx| {
            for _ in 0..3 {
                window.render_frame(cx);
            }
            let row = window.find(format!("row-{claim}")).bounds();
            tree.read_with(cx, |tree, _| {
                let list = &tree.scrolls[&vec![named_id("list")]];
                let (view, offset) = (list.bounds(), f32::from(list.offset().y));
                let (top, bottom) = (row.top() - view.top(), row.bottom() - view.top());
                (offset, f32::from(top), f32::from(bottom))
            })
        })
    };
    assert_eq!(
        shown(&mut native, 7),
        (-220., 60., 100.),
        "opened on row 7 (280..320), it sits on the bottom edge"
    );
    let claim = |native: &mut gpui_kit::VisualTestContext, row: usize| {
        tree.update(native, |tree, cx| tree.replace(claiming_list(row), &[], cx));
    };
    claim(&mut native, 3);
    assert_eq!(
        shown(&mut native, 3),
        (-120., 0., 40.),
        "row 3 (120..160) sits on the top edge"
    );
    claim(&mut native, 8);
    assert_eq!(
        shown(&mut native, 8),
        (-260., 60., 100.),
        "row 8 (320..360) sits on the bottom edge"
    );
    tree.read_with(&native, |tree, _| {
        tree.scrolls[&vec![named_id("list")]].set_offset(gpui_kit::point(px(0.), px(-100.)))
    });
    assert_eq!(
        shown(&mut native, 8).0,
        -100.,
        "a claim that stays does not pull the scroller back"
    );
}
