use super::*;
use gpui_kit::relative;

fn variable_list_node(
    count: usize,
    alignment: wire::ListAlignment,
    range_start: usize,
    rows: Vec<wire::Node>,
) -> wire::Node {
    let mut style = gpui_kit::StyleRefinement::default();
    style.size.width = Some(relative(1.).into());
    style.size.height = Some(relative(1.).into());
    wire::Node::List {
        id: named_id("messages"),
        path: vec![named_id("room"), named_id("messages")],
        item_count: count,
        alignment,
        overdraw: 32.,
        sizing: wire::ListSizingBehavior::Auto,
        following_tail: alignment == wire::ListAlignment::Bottom,
        revision: 0,
        commands: Vec::new(),
        request_handler: 41,
        scroll_handler: Some(42),
        range_start,
        style: crate::render::test_style(style),
        interactivity: Default::default(),
        children: rows,
    }
}

fn fixed_row(id: u64, height: f32, color: u32) -> wire::Node {
    let mut style = gpui_kit::StyleRefinement::default().h(px(height)).w_full();
    style.background = Some(rgb(color).into());
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Integer(id)),
        style: crate::render::test_style(style),
        interactivity: Default::default(),
        children: vec![],
    })
}

#[gpui_kit::test]
fn native_variable_list_measures_different_heights_and_keeps_slot_clip(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let root = variable_list_node(
        3,
        wire::ListAlignment::Top,
        0,
        vec![
            fixed_row(10, 20., 0xff0000),
            fixed_row(11, 60., 0x00ff00),
            fixed_row(12, 100., 0x0000ff),
        ],
    );
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    tree.read_with(&native, |tree, _| {
        let list = tree
            .variable_lists
            .values()
            .next()
            .expect("native list state");
        assert_eq!(list.state.bounds_for_item(0).unwrap().size.height, px(20.));
        assert_eq!(list.state.bounds_for_item(1).unwrap().size.height, px(60.));
        assert_eq!(list.state.bounds_for_item(2).unwrap().size.height, px(100.));
    });
    native.update(|window, _| {
        let bound = gpui_kit::ScaledPixels(200. * window.scale_factor());
        for quad in window.painted_quads() {
            assert!(quad.content_mask.bounds.right() <= bound);
        }
    });
}

/// A bottom-anchored list whose rows the view sent do not reach the top
/// of the viewport and its overdraw: the list lays the row above them
/// out, finds it missing and asks for it once, in one bounded request;
/// the rows it has are the tail, drawn from the bottom.
#[gpui_kit::test]
fn missing_far_rows_emit_one_bounded_request_and_bottom_anchor_uses_tail_rows(
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
    let root = variable_list_node(
        2_000,
        wire::ListAlignment::Bottom,
        1_998,
        vec![
            fixed_row(1998, 48., 0x111111),
            fixed_row(1999, 96., 0x222222),
        ],
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let window = cx.open_window(size(px(240.), px(120.)), |_, cx| {
        let tree = cx.new(|_| ViewTree::new(root));
        let subscription = cx.subscribe(&tree, move |_, _, event: &wire::Event, _| {
            received.borrow_mut().push(event.clone());
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
    native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
    let requests = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::ListRequest { request, .. } => Some(*request),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        requests,
        [wire::ListRequest {
            start: 1_997,
            end: 1_998,
        }],
        "the row the overdraw lays out and lacks, asked for once"
    );
    tree.read_with(&native, |tree, _| {
        let list = tree.variable_lists.values().next().unwrap();
        assert!(list.rows.contains_key(&1_998) && list.rows.contains_key(&1_999));
        assert!(matches!(
            &list.rows[&1_998],
            wire::Node::Container(view_wire::ContainerNode {
                id: Some(wire::ElementIdWire::Integer(1998)),
                ..
            })
        ));
        assert!(list.state.logical_scroll_top().item_ix >= 1_998);
    });
}

#[gpui_kit::test]
fn accepted_frames_retain_anonymous_list_scroll(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = axis_container(
        "room",
        Axis::Column,
        [variable_list_node(
            3,
            wire::ListAlignment::Top,
            0,
            vec![
                fixed_row(10, 20., 0xff0000),
                fixed_row(11, 60., 0x00ff00),
                fixed_row(12, 100., 0x0000ff),
            ],
        )],
    );
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(root.clone()));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    native.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            let state = tree.variable_lists.values().next().unwrap().state.clone();
            state.scroll_to(gpui_kit::ListOffset {
                item_ix: 1,
                offset_in_item: px(3.),
            });
            let before = state.logical_scroll_top();
            tree.replace(root.clone(), &[], cx);
            let retained = tree
                .variable_lists
                .values()
                .next()
                .expect("anonymous List remains mounted");
            assert_eq!(retained.state.logical_scroll_top().item_ix, before.item_ix);
            assert_eq!(
                retained.state.logical_scroll_top().offset_in_item,
                before.offset_in_item
            );
            tree.replace(wire::Node::empty(), &[], cx);
            assert!(
                tree.variable_lists.is_empty(),
                "unmounted List state is retired"
            );
        })
    });
}

/// A view's `scroll_to` (no wheel, no drag) reports once where the list
/// settled: the visible range reaches the end; nothing more is reported
/// while the list stands still.
#[gpui_kit::test]
fn a_programmatic_scroll_reports_where_the_list_settled(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let rows = (0..40)
        .map(|index| fixed_row(100 + index, 20., 0x404040))
        .collect();
    let root = container_with_style(
        "room",
        div().w(px(200.)).h(px(120.)).style().clone(),
        [variable_list_node(40, wire::ListAlignment::Top, 0, rows)],
    );
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    let frame = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            window.simulate_next_frame(cx);
        });
    };
    frame(&mut native);
    let scrolls = |events: &Rc<RefCell<Vec<wire::Event>>>| -> Vec<wire::ListScroll> {
        events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                wire::Event::ListScroll { handler: 42, event } => Some(event.clone()),
                _ => None,
            })
            .collect()
    };
    // the mount's own report: the top six rows
    let mounted = scrolls(&events);
    assert_eq!(mounted.len(), 1);
    assert_eq!(
        (mounted[0].visible_start, mounted[0].is_scrolled),
        (0, false)
    );
    events.borrow_mut().clear();
    native.update(|_, cx| {
        tree.update(cx, |tree, cx| {
            let state = tree.variable_lists.values().next().unwrap().state.clone();
            state.scroll_to(gpui_kit::ListOffset {
                item_ix: 39,
                offset_in_item: px(0.),
            });
            cx.notify();
        })
    });
    frame(&mut native);
    let settled = scrolls(&events);
    assert_eq!(settled.len(), 1, "one report for the scroll");
    assert_eq!(settled[0].visible_end, 40, "reaches the end");
    assert!(settled[0].visible_start >= 34 && settled[0].is_scrolled);
    assert_eq!(settled[0].count, 40);
    assert_eq!(settled[0].offset.item_ix, settled[0].visible_start);
    // idle: nothing more
    frame(&mut native);
    frame(&mut native);
    assert_eq!(scrolls(&events).len(), 1, "nothing while idle");
}

/// A row is kept while its node is equal, and equal ids are equal styles
/// only in one table. A whole frame brings its own: its rows, equal by id
/// to the ones held, are other rows (here twice as tall), and each is
/// taken and measured again.
#[gpui_kit::test]
fn a_row_equal_by_its_ids_is_measured_again_under_another_table(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    const ROWS: usize = 4;
    let tree = |height: f32| {
        let list = gpui_kit::StyleRefinement::default().size_full();
        let mut row = gpui_kit::StyleRefinement::default().h(px(height)).w_full();
        row.background = Some(rgb(0x111111).into());
        let mut styles = wire::Styles::default();
        styles
            .extend(vec![wire::Style::new(&list), wire::Style::new(&row)])
            .unwrap();
        let rows = (0..ROWS as u64).map(|id| {
            wire::Node::Container(view_wire::ContainerNode {
                id: Some(wire::ElementIdWire::Integer(id)),
                style: wire::StyleId(1),
                interactivity: Default::default(),
                children: vec![],
            })
        });
        let mut root = variable_list_node(ROWS, wire::ListAlignment::Top, 0, rows.collect());
        let wire::Node::List { style, .. } = &mut root else {
            unreachable!()
        };
        *style = wire::StyleId(0);
        Tree { root, styles }
    };
    let window = cx.open_window(size(px(200.), px(240.)), |_, _| ViewTree::new(tree(20.)));
    let view = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let drawn = |native: &mut gpui_kit::VisualTestContext| {
        let heights: Vec<f32> = native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
            let scale = window.scale_factor();
            let quads = window.painted_quads();
            let rows = quads
                .iter()
                .filter(|quad| quad.background.as_solid() == Some(rgb(0x111111).into()));
            rows.map(|quad| quad.bounds.size.height.as_f32() / scale)
                .collect()
        });
        let remeasured = view.read_with(native, |tree, _| {
            tree.variable_lists.values().next().unwrap().remeasured
        });
        (heights, remeasured)
    };
    assert_eq!(drawn(&mut native), (vec![20.; ROWS], ROWS));
    view.update(&mut native, |view, cx| view.replace(tree(40.), &[], cx));
    assert_eq!(
        drawn(&mut native),
        (vec![40.; ROWS], 2 * ROWS),
        "another table's rows were taken for the ones held"
    );
}

/// A changed frame that leaves a list's rows as they were (here the title
/// above it moved) remeasures none of them: a row is kept while its node is
/// equal, routes included, and a list row's routes are keyed by its row,
/// not its place in the frame. Before, every frame dropped the rows and
/// remeasured each one it sent: a chat timeline relaid out every visible
/// message's text per changed frame.
#[gpui_kit::test]
fn a_changed_frame_remeasures_no_row_it_kept(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let screen = |title: &str| {
        axis_container(
            "room",
            Axis::Column,
            [
                text("title", title),
                variable_list_node(
                    24,
                    wire::ListAlignment::Bottom,
                    0,
                    (0..24).map(|id| fixed_row(id, 20., 0x111111)).collect(),
                ),
            ],
        )
    };
    let window = cx.open_window(size(px(200.), px(240.)), |_, _| {
        ViewTree::new(screen("Room"))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let remeasured = |native: &mut gpui_kit::VisualTestContext| {
        tree.read_with(native, |tree, _| {
            tree.variable_lists.values().next().unwrap().remeasured
        })
    };
    assert_eq!(
        remeasured(&mut native),
        24,
        "the first frame measures its rows"
    );
    for title in ["Room ·", "Room"] {
        tree.update(&mut native, |tree, cx| tree.replace(screen(title), &[], cx));
        native.update(crate::render::draw_frame);
    }
    assert_eq!(
        remeasured(&mut native),
        24,
        "two changed frames with the same rows remeasured them"
    );
}

/// A list row with no id of its own, 32 px tall, holding a text field
/// `name` labelled `label` and holding `value`.
fn field_row(label: &str, value: &str) -> wire::Node {
    let mut field = input(label, false, false);
    let wire::Node::Field {
        id,
        value: held,
        cursor,
        ..
    } = &mut field
    else {
        unreachable!()
    };
    *id = wire::ElementIdWire::Name("name".into());
    *held = value.into();
    *cursor = wire::TextRange::caret(value.len());
    wire::Node::Container(view_wire::ContainerNode {
        id: None,
        style: crate::render::test_style(div().w_full().h(px(32.)).style()),
        interactivity: Default::default(),
        children: vec![field],
    })
}

/// Two rows of a list, neither with an id of its own, each with a field
/// `name` inside (P28's shape): each row is filed under its index, so the
/// two are two native fields, each holding its own text, and two nodes a
/// screen reader reads. Filed under one path they were one field.
#[gpui_kit::test]
fn rows_holding_one_id_are_each_their_own_field(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = variable_list_node(
        2,
        wire::ListAlignment::Top,
        0,
        vec![field_row("Name 0", "ada"), field_row("Name 1", "grace")],
    );
    let window = cx.open_window(size(px(300.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let nodes = native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        crate::ax::snapshot("t", window, false)
    });
    let fields: Vec<(&str, Option<&str>)> = nodes
        .iter()
        .filter(|node| node.role == "TextInput")
        .map(|node| (node.name.as_str(), node.value.as_deref()))
        .collect();
    assert_eq!(fields, [("Name 0", Some("ada")), ("Name 1", Some("grace"))]);
    assert_eq!(tree.read_with(&native, |tree, _| tree.fields.len()), 2);
}

/// A list row with no id of its own and no box either (a deferred draw),
/// holding an `open` button named `name`.
fn deferred_row(name: &str) -> wire::Node {
    let mut open = view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("open".into())),
        style: crate::render::test_style(div().w(px(80.)).h(px(24.)).style()),
        interactivity: Default::default(),
        children: Vec::new(),
    };
    open.interactivity.get_or_insert_default().role = Some(gpui_kit::Role::Button);
    open.interactivity.get_or_insert_default().aria.label = Some(name.into());
    open.interactivity.get_or_insert_default().on_click = Some(1);
    wire::Node::Deferred {
        priority: 1,
        content: Box::new(wire::Node::Container(open)),
    }
}

/// Rows that lay out no element of their own still draw under their index:
/// two `open` buttons, two nodes. Under one id gpui keeps the first node
/// and drops the second.
#[gpui_kit::test]
fn rows_without_a_box_draw_under_their_index(cx: &mut gpui_kit::TestAppContext) {
    let nodes = draw(
        cx,
        variable_list_node(
            2,
            wire::ListAlignment::Top,
            0,
            vec![deferred_row("Open 0"), deferred_row("Open 1")],
        ),
    );
    let buttons: Vec<&str> = nodes
        .iter()
        .filter(|node| node.role == "Button")
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(buttons, ["Open 0", "Open 1"]);
}

/// Two lists under one parent, their rows with no id of their own, each
/// holding an `open` button: a list is a scope of its own, so each list's
/// rows are drawn under the list, and a row of one never meets the row of
/// the other at its index. Four buttons, four nodes; under one id gpui
/// keeps the first node and drops the second.
#[gpui_kit::test]
fn two_lists_under_one_parent_each_draw_their_own_rows(cx: &mut gpui_kit::TestAppContext) {
    let lists = (1..=2).map(|at| {
        let rows = (0..2)
            .map(|row| deferred_row(&format!("Open {at}.{row}")))
            .collect();
        let mut list = variable_list_node(2, wire::ListAlignment::Top, 0, rows);
        let wire::Node::List { id, path, .. } = &mut list else {
            unreachable!()
        };
        *id = named_id(&format!("list-{at}"));
        *path = vec![named_id("page"), id.clone()];
        list
    });
    let page = div().size_full().flex().flex_col().style().clone();
    let nodes = draw(cx, container_with_style("page", page, lists));
    let buttons: Vec<&str> = nodes
        .iter()
        .filter(|node| node.role == "Button")
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(buttons, ["Open 1.0", "Open 1.1", "Open 2.0", "Open 2.1"]);
}

/// A field in a list row keeps what the host holds for it (here, the words
/// selected in it) when the view is instantiated again. The new
/// instance files the row under the path the old one did, the list's id in
/// it, and the host finds the field by that path, whichever list the new
/// instance draws first. A list draws its rows after the tree's render
/// returns: what the old tree carried over is theirs to claim until the
/// frame is drawn.
#[gpui_kit::test]
fn a_field_in_a_list_row_crosses_a_new_generation(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = |order: [&str; 2]| {
        let panes = order.map(|pane| {
            let rows = vec![field_row(pane, "hunter2")];
            let mut list = variable_list_node(1, wire::ListAlignment::Top, 0, rows);
            let wire::Node::List { id, path, .. } = &mut list else {
                unreachable!()
            };
            *id = named_id(pane);
            *path = vec![named_id("page"), id.clone()];
            list
        });
        let page = div().size_full().flex().flex_col().style().clone();
        container_with_style("page", page, panes)
    };
    let field = |pane: &str| {
        let row = wire::ElementIdWire::Integer(0);
        vec![named_id("page"), named_id(pane), row, named_id("name")]
    };
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(root(["timeline", "thread"]))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let saved = native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        let (target, start, end) = (field("thread"), 2, 5);
        let select = wire::WidgetCommand::Select { target, start, end };
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(select, window, cx).unwrap();
        });
        tree.read(cx).presentation(window, cx)
    });
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(root(["thread", "timeline"])).with_presentation(saved)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let selected = native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        let tree = tree.read(cx);
        ["timeline", "thread"]
            .map(|pane| tree.fields[field(pane).as_slice()].presentation(window, cx))
            .map(|field| field.selection)
    });
    assert_eq!(selected, [7..7, 2..5]);
}

const LIST_HEIGHT: f32 = 600.;

fn chat_list(
    range_start: usize,
    rows: Vec<wire::Node>,
    overdraw: f32,
    revision: u64,
    commands: Vec<wire::ListCommand>,
) -> wire::Node {
    let mut node = variable_list_node(256, wire::ListAlignment::Bottom, range_start, rows);
    if let wire::Node::List {
        overdraw: drawn,
        revision: carried,
        commands: queued,
        ..
    } = &mut node
    {
        *drawn = overdraw;
        *carried = revision;
        *queued = commands;
    }
    node
}

fn keyed_rows(keys: std::ops::Range<u64>, height: f32) -> Vec<wire::Node> {
    keys.map(|key| fixed_row(key, height, 0x111111)).collect()
}

fn seated(
    cx: &mut gpui_kit::TestAppContext,
    root: wire::Node,
) -> (Entity<ViewTree>, gpui_kit::VisualTestContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(LIST_HEIGHT)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(root)))
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
    (tree, native)
}

fn scroll_top(
    tree: &Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
) -> gpui_kit::ListOffset {
    tree.read_with(native, |tree, _| {
        tree.variable_lists
            .values()
            .next()
            .expect("the list is mounted")
            .state
            .logical_scroll_top()
    })
}

fn wheel_frame(native: &mut gpui_kit::VisualTestContext, up: &[f32]) {
    native.update(|window, cx| {
        for up in up {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: point(px(200.), px(LIST_HEIGHT / 2.)),
                    delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(*up))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        }
    });
    native.update(crate::render::draw_frame);
    native.run_until_parked();
}

#[gpui_kit::test]
fn one_frame_of_the_wheel_moves_a_list_as_far_as_its_overdraw(cx: &mut gpui_kit::TestAppContext) {
    let mut moved = |overdraw: f32| {
        let root = chat_list(192, keyed_rows(192..256, 100.), overdraw, 0, vec![]);
        let (tree, mut native) = seated(cx, root);
        let top = 256. - LIST_HEIGHT / 100.;
        wheel_frame(&mut native, &[2_000.]);
        let after = scroll_top(&tree, &mut native);
        (top - after.item_ix as f32) * 100. - f32::from(after.offset_in_item)
    };
    assert_eq!(moved(160.), 200., "two rows: the overdraw, to a row");
    assert_eq!(moved(600.), 600., "chat's overdraw");
}

#[gpui_kit::test]
fn a_window_slid_by_two_splices_keeps_a_scrolled_reader_on_their_row(
    cx: &mut gpui_kit::TestAppContext,
) {
    let root = chat_list(192, keyed_rows(192..256, 60.), 600., 0, vec![]);
    let (tree, mut native) = seated(cx, root);
    for _ in 0..6 {
        wheel_frame(&mut native, &[125.]);
    }
    let scrolled = scroll_top(&tree, &mut native);
    assert!(
        scrolled.item_ix < 250 && scrolled.offset_in_item > px(0.),
        "{scrolled:?}"
    );
    let slid = chat_list(
        191,
        keyed_rows(192..257, 60.),
        600.,
        1,
        vec![
            wire::ListCommand::Splice {
                start: 256,
                end: 256,
                count: 1,
            },
            wire::ListCommand::Splice {
                start: 0,
                end: 1,
                count: 0,
            },
        ],
    );
    tree.update(&mut native, |tree, cx| tree.replace(slid, &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    let after = scroll_top(&tree, &mut native);
    assert_eq!(
        after.item_ix,
        scrolled.item_ix - 1,
        "the same row, one place up"
    );
    assert_eq!(after.offset_in_item, scrolled.offset_in_item);
}

/// A thread's list as chat-view sends it once every reply is in: top
/// aligned, following its end, opened on its first row.
fn following_list(count: usize, revision: u64, commands: Vec<wire::ListCommand>) -> wire::Node {
    let rows = (0..count)
        .map(|index| fixed_row(100 + index as u64, 20., 0x404040))
        .collect();
    let mut list = variable_list_node(count, wire::ListAlignment::Top, 0, rows);
    if let wire::Node::List {
        following_tail,
        revision: sent_at,
        commands: sent,
        ..
    } = &mut list
    {
        *following_tail = true;
        *sent_at = revision;
        *sent = commands;
    }
    let style = div().w(px(200.)).h(px(120.)).style().clone();
    container_with_style("room", style, [list])
}

fn laid_out(native: &mut gpui_kit::VisualTestContext) {
    native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        window.simulate_next_frame(cx);
        crate::render::draw_frame(window, cx);
    });
}

fn sent(tree: &Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext, node: wire::Node) {
    native.update(|_, cx| tree.update(cx, |tree, cx| tree.replace(node, &[], cx)));
    laid_out(native);
}

/// A list that follows its end moves for a row that arrives only while it
/// stands at its end: scrolled up, nothing moves; back at the end, it
/// follows again. gpui decides it at each layout by where the list stands,
/// which is what chat-view leans on (J-075).
#[gpui_kit::test]
fn a_following_list_follows_arrivals_only_while_it_stands_at_its_end(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let top = wire::ListOffset {
        item_ix: 0,
        offset_in_item: 0.,
    };
    let opened = following_list(
        40,
        1,
        vec![
            wire::ListCommand::SetFollowMode { tail: true },
            wire::ListCommand::ScrollTo(top),
        ],
    );
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(opened));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    laid_out(&mut native);
    let state = tree.read_with(&native, |tree, _| {
        let list = tree.variable_lists.values().next().expect("the list");
        list.state.clone()
    });
    assert!(!state.is_following_tail(), "opened on its first row");
    let arrival = |count: usize, revision| {
        let at = count - 1;
        let splice = wire::ListCommand::Splice {
            start: at,
            end: at,
            count: 1,
        };
        following_list(count, revision, vec![splice])
    };
    sent(&tree, &mut native, arrival(41, 2));
    assert_eq!(
        state.logical_scroll_top().item_ix,
        0,
        "scrolled up: nothing moves"
    );
    assert!(!state.is_following_tail());
    let to_end = following_list(41, 3, vec![wire::ListCommand::ScrollToEnd]);
    sent(&tree, &mut native, to_end);
    assert!(state.is_following_tail(), "at its end, it follows");
    sent(&tree, &mut native, arrival(42, 4));
    assert!(state.is_following_tail());
    assert_eq!(
        state.item_is_below_viewport(41),
        Some(false),
        "the row that arrived is on screen"
    );
}
