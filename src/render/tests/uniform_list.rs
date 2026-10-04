use super::*;

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

fn uniform_node(id: &str, count: usize, rows: Range<usize>) -> wire::Node {
    let wire_id = named_id(id);
    let mut indices = rows.map(|index| index as u32).collect::<Vec<_>>();
    if count > 0 && !indices.contains(&0) {
        indices.insert(0, 0);
    }
    let children = indices
        .iter()
        .map(|index| {
            sized(
                &format!("{id}/row:{index}"),
                text(&format!("{id}/label:{index}"), format!("Row {index}")),
                Some(fill()),
                Some(fixed(24.)),
            )
        })
        .collect();
    wire::Node::UniformList {
        id: wire_id.clone(),
        path: vec![wire_id],
        route: 1,
        style: crate::render::test_style(sized_style(Some(fill()), Some(fixed(96.)))),
        interactivity: Default::default(),
        count,
        measure_index: 0,
        sizing: wire::list::UniformListSizing::Auto,
        horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
        y_flipped: false,
        scroll_request: None,
        revision: 0,
        indices,
        children,
    }
}

/// `list`'s style with `edit` applied, named in this test's table.
fn restyle(list: &mut wire::Node, edit: impl FnOnce(&mut gpui_kit::StyleRefinement)) {
    let wire::Node::UniformList { style, .. } = list else {
        unreachable!()
    };
    let mut edited = test_styles()[*style].clone();
    edit(&mut edited);
    *style = crate::render::test_style(edited);
}

#[gpui_kit::test]
fn uniform_list_measures_row_zero_and_emits_bounded_viewport_ranges(
    cx: &mut gpui_kit::TestAppContext,
) {
    struct Host {
        tree: Entity<ViewTree>,
        _subscription: Subscription,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.tree.clone())
        }
    }

    cx.update(gpui_kit::init);
    let node = uniform_node("uniform", 2_000, 0..1);
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let window = cx.open_window(size(px(240.), px(96.)), |_, cx| {
        let tree = cx.new(|_| ViewTree::new(node));
        let subscription = cx.subscribe(&tree, move |_, _, event: &wire::Event, _| {
            if let wire::Event::UniformListRange { .. } = event {
                received.borrow_mut().push(event.clone());
            }
        });
        Host {
            tree,
            _subscription: subscription,
        }
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |host, _| host.tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();

    let events = events.borrow();
    assert!(
        events.iter().any(|event| matches!(
            event,
            wire::Event::UniformListRange {
                start: 0,
                end,
                item_height,
                ..
            } if *end > 1 && (*end as usize) <= wire::MAX_UNIFORM_LIST_ROWS && *item_height == 24.
        )),
        "initial viewport event, with the measured row height: {events:?}"
    );
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert_eq!(state.rows.len(), 1, "the first frame stores only row zero");
        assert!(state.rows.contains_key(&0));
        assert!(state.requested.is_some());
    });
    // the same rows drawn again are the ones held, not taken again
    native.update(|_, cx| tree.update(cx, |_, cx| cx.notify()));
    native.update(|window, cx| window.render_frame(cx));
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert_eq!(state.replaced, 1, "row zero was taken once");
    });
}

#[gpui_kit::test]
fn uniform_list_scroll_requests_far_rows_without_guest_layout_or_unbounded_state(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let node = uniform_node("uniform", 2_000, 0..1);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::UniformListRange { .. } = event {
                received.borrow_mut().push(event.clone());
            }
        })
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    events.borrow_mut().clear();

    tree.update(&mut native, |tree, _| {
        tree.uniform_lists
            .get(&vec![named_id("uniform")])
            .unwrap()
            .scroll
            .scroll_to_item(1_500, gpui_kit::ScrollStrategy::Top);
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();

    let events = events.borrow();
    let far = events.iter().find_map(|event| match event {
        wire::Event::UniformListRange { start, end, .. } if *start >= 1_500 => Some((*start, *end)),
        _ => None,
    });
    let (start, end) = far.expect("far scroll asks guest for a visible range");
    assert!(end > start);
    assert!((end - start) as usize <= wire::MAX_UNIFORM_LIST_ROWS);
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert!(state.rows.len() <= wire::MAX_UNIFORM_LIST_ROWS + 1);
        assert!(state.rows.contains_key(&0));
    });
}

#[gpui_kit::test]
fn uniform_list_row_cache_contains_only_the_current_frame(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let first = uniform_node("uniform", 2_000, 0..4);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(first));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();

    tree.update(&mut native, |tree, cx| {
        tree.replace(uniform_node("uniform", 2_000, 1_000..1_020), &[], cx);
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        assert!(
            state.rows.contains_key(&0),
            "measurement row survives patch"
        );
        assert!(state.rows.contains_key(&1_000));
        assert_eq!(state.rows.len(), 21);
        assert!(
            state
                .rows
                .keys()
                .all(|index| *index == 0 || *index >= 1_000)
        );
        assert!(
            !state.rows.contains_key(&1),
            "an old row callback cannot survive into the new frame"
        );
    });
}

#[gpui_kit::test]
fn uniform_list_click_uses_native_identity_and_records_user_activation(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let mut node = uniform_node("uniform-click", 4, 0..4);
    let wire::Node::UniformList { interactivity, .. } = &mut node else {
        unreachable!()
    };
    interactivity.on_click = Some(42);
    interactivity.role = Some(gpui_kit::Role::List);
    interactivity.aria.label = Some("Rows".into());
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    native.update(|window, cx| {
        window
            .within("uniform-click")
            .click("uniform-click/row:0", cx)
    });

    assert!(events.borrow().iter().any(|event| matches!(
        event,
        wire::Event::Click {
            handler: 42,
            event: wire::click::Click::Mouse { .. }
        }
    )));
    assert!(tree.read_with(&native, |tree, _| tree.take_activation().is_some()));
}

/// A view that asks for the keys on a uniform list by its path hears the
/// host say no: the host focuses a container, a field or an editor, and a
/// list is none of them (a guest focus handle tracked on it is the way).
#[gpui_kit::test]
fn a_focus_on_a_uniform_list_is_refused(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let node = uniform_node("rows", 4, 0..4);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let answer = native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: vec![named_id("rows")],
                },
                window,
                cx,
            )
        })
    });
    assert!(answer.is_err(), "{answer:?}");
}

/// A scroll request is the view's one-shot: applied when its revision
/// moves, not on every frame that carries it on, so the reader's wheel
/// keeps its place across a frame that changed nothing else (V3/V4).
#[gpui_kit::test]
fn a_scroll_request_is_applied_once_per_revision(cx: &mut gpui_kit::TestAppContext) {
    struct Host {
        tree: Entity<ViewTree>,
        _subscription: Subscription,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.tree.clone())
        }
    }
    cx.update(gpui_kit::init);
    let asked = |index: usize, revision: u64| {
        let mut node = uniform_node("uniform", 2_000, 0..1);
        let wire::Node::UniformList {
            scroll_request,
            revision: carried,
            ..
        } = &mut node
        else {
            unreachable!()
        };
        *scroll_request = Some(wire::list::UniformListScrollRequest {
            index,
            strategy: wire::list::UniformListScrollStrategy::Top,
            offset: 0,
            strict: true,
        });
        *carried = revision;
        node
    };
    let shown = Rc::new(RefCell::new(Vec::new()));
    let received = shown.clone();
    let window = cx.open_window(size(px(240.), px(96.)), |_, cx| {
        let tree = cx.new(|_| ViewTree::new(asked(1_500, 1)));
        let subscription = cx.subscribe(&tree, move |_, _, event: &wire::Event, _| {
            if let wire::Event::UniformListRange { start, .. } = event {
                received.borrow_mut().push(*start);
            }
        });
        Host {
            tree,
            _subscription: subscription,
        }
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |host, _| host.tree.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let drawn = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        native.run_until_parked();
    };
    drawn(&mut native);
    assert_eq!(
        shown.borrow().last(),
        Some(&1_500),
        "the request scrolled the list"
    );
    // the reader wheels up to row 1_000
    tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        state
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-1_000. * 24.)));
    });
    native.update(|_, cx| tree.update(cx, |_, cx| cx.notify()));
    drawn(&mut native);
    assert_eq!(
        shown.borrow().last(),
        Some(&1_000),
        "a frame carrying the request on leaves the wheel where it is: {:?}",
        shown.borrow()
    );
    // the view asks again, under the next revision: applied
    native.update(|_, cx| tree.update(cx, |tree, cx| tree.replace(asked(100, 2), &[], cx)));
    drawn(&mut native);
    assert_eq!(shown.borrow().last(), Some(&100), "{:?}", shown.borrow());
}

/// A guest uniform list draws the host's scroll bar, as the host's own
/// scrollers do: the reader sees where in the rows they are. The bar is the
/// only paint in the list's right strip (the rows start at its left edge).
#[gpui_kit::test]
fn a_uniform_list_draws_the_hosts_scroll_bar(cx: &mut gpui_kit::TestAppContext) {
    struct Host {
        tree: Entity<ViewTree>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.tree.clone())
        }
    }
    cx.update(gpui_kit::init);
    let width = 240.;
    let window = cx.open_window(size(px(width), px(96.)), |_, cx| Host {
        tree: cx.new(|_| ViewTree::new(uniform_node("uniform", 2_000, 0..1))),
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    native.run_until_parked();
    let in_the_strip = native.update(|window, _| {
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.bounds.origin.x.0 >= width - 16.)
            .count()
    });
    assert!(
        in_the_strip > 0,
        "the bar's track and thumb are painted in the list's right strip"
    );
}

/// The host keeps the bar's gutter beside every guest uniform list, outside
/// the list's own box: the box (its fill here) and its rows end where the
/// gutter begins, and the room the view gave the list holds both. A list of
/// few rows keeps the gutter, as a view's scroller does.
#[gpui_kit::test]
fn a_uniform_list_ends_where_the_bars_gutter_begins(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let width = 240.;
    let gutter = width - 16.;
    for count in [2_000, 3] {
        let mut node = uniform_node("uniform", count, 0..3);
        restyle(&mut node, |style| {
            style.background = Some(gpui_kit::red().into());
        });
        let window = cx.open_window(size(px(width), px(96.)), |_, _| ViewTree::new(node));
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        native.run_until_parked();
        // the rows draw no quad: the list's fill starts at the window's left
        // edge, the bar's track and thumb are the quads past it
        let (fill, bar): (Vec<_>, Vec<_>) = native.update(|window, _| {
            let scale = window.scale_factor();
            let quads = window.painted_quads();
            let edges = quads.iter().map(|quad| {
                let left = quad.bounds.origin.x.0 / scale;
                (left, left + quad.bounds.size.width.0 / scale)
            });
            edges.partition(|(left, _)| *left == 0.)
        });
        assert_eq!(
            fill,
            [(0., gutter)],
            "{count} rows: the list's box ends where the gutter begins"
        );
        let row =
            native.update(|window, _| window.within("uniform").find("uniform/row:0").bounds());
        assert_eq!(
            row.right(),
            px(gutter),
            "{count} rows: the rows end with it"
        );
        assert!(
            bar.iter()
                .all(|(left, right)| *left >= gutter && *right <= width),
            "{count} rows: the bar is in the gutter: {bar:?}"
        );
        assert_eq!(
            bar.is_empty(),
            count == 3,
            "{count} rows: a list that scrolls draws the bar: {bar:?}"
        );
    }
}

/// The wheel over the bar's gutter scrolls the list, as it does over a
/// scroller's bar: the gutter is outside the list's box, not outside its
/// reach.
#[gpui_kit::test]
fn the_wheel_over_the_bars_gutter_scrolls_the_list(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let node = uniform_node("uniform", 2_000, 0..3);
    let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_event(gpui_kit::ScrollWheelEvent {
        position: point(px(232.), px(48.)),
        delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-240.))),
        ..Default::default()
    });
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    let top = tree.read_with(&native, |tree, _| {
        let state = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
        state.requested.clone().map(|rows| rows.start)
    });
    assert_eq!(top, Some(10), "ten rows of 24 px down");
}

/// The wheel over a list's gutter is the wheel over its rows: the list
/// takes it and so does a page that scrolls around the list, whether the
/// list has rows to scroll or not.
#[gpui_kit::test]
fn the_wheel_over_the_bars_gutter_reaches_the_page_as_over_the_rows(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_base::ScrollbarHandle as _;
    cx.update(gpui_kit::init);
    // the page's and the list's offsets after one wheel of 50 px at `x`
    let offsets = |cx: &mut gpui_kit::TestAppContext, count: usize, x: f32| {
        let mut style = div()
            .flex()
            .flex_col()
            .w(px(240.))
            .h(px(96.))
            .style()
            .clone();
        style.overflow.y = Some(gpui_kit::Overflow::Scroll);
        let filler = sized(
            "filler",
            text("filler/label", "Filler"),
            Some(fill()),
            Some(fixed(300.)),
        );
        let mut list = uniform_node("uniform", count, 0..3);
        // as the SDK sends a list: it scrolls
        restyle(&mut list, |own| {
            own.overflow.y = Some(gpui_kit::Overflow::Scroll);
        });
        let node = container_with_style("page", style, [list, filler]);
        let window = cx.open_window(size(px(240.), px(96.)), |_, _| ViewTree::new(node));
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        });
        native.simulate_event(gpui_kit::ScrollWheelEvent {
            position: point(px(x), px(48.)),
            delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-50.))),
            ..Default::default()
        });
        native.update(|window, cx| window.render_frame(cx));
        native.run_until_parked();
        tree.read_with(&native, |tree, _| {
            let list = tree.uniform_lists.get(&vec![named_id("uniform")]).unwrap();
            (
                f32::from(tree.scrolls[&vec![named_id("page")]].offset().y),
                f32::from(list.scroll.offset().y),
            )
        })
    };
    for count in [3, 2_000] {
        let over_rows = offsets(cx, count, 100.);
        let over_gutter = offsets(cx, count, 232.);
        assert_eq!(over_rows.0, -50., "{count} rows: the page scrolls");
        assert_eq!(
            over_gutter, over_rows,
            "{count} rows: (page, list) over the gutter as over the rows"
        );
    }
}

/// Each list's bar is its own: of two lists side by side, a drag on the
/// second's thumb scrolls the second and leaves the first where it was.
#[gpui_kit::test]
fn each_of_two_lists_side_by_side_drags_its_own_bar(cx: &mut gpui_kit::TestAppContext) {
    use gpui_base::ScrollbarHandle as _;
    cx.update(gpui_kit::init);
    let node = axis_container(
        "pair",
        Axis::Row,
        [
            uniform_node("a", 2_000, 0..3),
            uniform_node("b", 2_000, 0..3),
        ],
    );
    let window = cx.open_window(size(px(248.), px(96.)), |_, _| ViewTree::new(node));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    });
    native.run_until_parked();
    // b's thumb is at the top of b's gutter, past b's rows
    let rows_end = native.update(|window, _| window.within("b").find("b/row:0").bounds().right());
    let x = rows_end + px(8.);
    native.simulate_mouse_move(point(x, px(12.)), None, Default::default());
    native.simulate_mouse_down(
        point(x, px(12.)),
        gpui_kit::MouseButton::Left,
        Default::default(),
    );
    native.update(|window, cx| window.render_frame(cx));
    native.simulate_mouse_move(
        point(x, px(60.)),
        gpui_kit::MouseButton::Left,
        Default::default(),
    );
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    let [a, b] = tree.read_with(&native, |tree, _| {
        ["a", "b"].map(|id| {
            let list = tree.uniform_lists.get(&vec![named_id(id)]).unwrap();
            f32::from(list.scroll.offset().y)
        })
    });
    assert_eq!(a, 0., "the first list stays (the second is at {b})");
    assert!(b < 0., "the second list scrolls: {b}");
}
