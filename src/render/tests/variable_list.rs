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
        state: 17,
        path: vec![wire::ElementIdWire::Name("room".into())],
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
        style,
        interactivity: Default::default(),
        children: rows,
    }
}

fn fixed_row(id: u64, height: f32, color: u32) -> wire::Node {
    let mut style = gpui_kit::StyleRefinement::default().h(px(height)).w_full();
    style.background = Some(rgb(color).into());
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Integer(id)),
        style,
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
    native.update(|window, cx| window.render_frame(cx));
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

#[gpui_kit::test]
fn missing_far_rows_emit_one_bounded_request_and_bottom_anchor_uses_tail_rows(
    cx: &mut gpui_kit::TestAppContext,
) {
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
    let window = cx.open_window(size(px(240.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
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
    assert!(
        !requests.is_empty(),
        "leading overdraw requests its missing row"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.end - request.start <= wire::MAX_LIST_ROWS)
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
    native.update(|window, cx| window.render_frame(cx));
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
    native.update(|window, cx| window.render_frame(cx));
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
        native.update(|window, cx| window.render_frame(cx));
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
        style: div().w_full().h(px(32.)).style().clone(),
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
        window.render_frame(cx);
        window.render_frame(cx);
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
        style: div().w(px(80.)).h(px(24.)).style().clone(),
        interactivity: Default::default(),
        children: Vec::new(),
    };
    open.interactivity.role = Some(gpui_kit::Role::Button);
    open.interactivity.aria.label = Some(name.into());
    open.interactivity.on_click = Some(1);
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
