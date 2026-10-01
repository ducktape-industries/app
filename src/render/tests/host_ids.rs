//! No id a view can send equals one the host invents beside its children
//! (briefs/guest-id-collision.md). Each frame below passed sanitize and
//! crashed the host on dev with "Duplicate a11y node id", or made two
//! elements share one gpui state; now each draws with every node in the
//! tree and every click on its own element.
use super::*;

fn plain_text(id: Option<wire::ElementIdWire>, content: &str) -> wire::Node {
    wire::Node::Text(view_wire::TextNode {
        id,
        style: gpui_kit::StyleRefinement::default(),
        content: content.into(),
    })
}

fn rich(id: Option<wire::ElementIdWire>, content: &str) -> wire::Node {
    wire::Node::RichText {
        id,
        style: Default::default(),
        text: content.into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    }
}

fn box_of(id: Option<wire::ElementIdWire>, children: Vec<wire::Node>) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id,
        style: div().flex().flex_col().style().clone(),
        interactivity: Default::default(),
        children,
    })
}

/// A button `name` that reports `handler` when clicked, 80 by 24 px.
fn button(id: Option<wire::ElementIdWire>, name: &str, handler: u32) -> wire::Node {
    let mut node = box_of(id, Vec::new());
    if let wire::Node::Container(view_wire::ContainerNode {
        interactivity,
        style,
        ..
    }) = &mut node
    {
        *style = div().w(px(80.)).h(px(24.)).style().clone();
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.aria.label = Some(name.into());
        interactivity.on_click = Some(handler);
    }
    node
}

/// Draws `root` with accessibility on and answers the door's nodes.
fn draw(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> Vec<crate::ax::AxNode> {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        crate::ax::snapshot("t", window, false)
    })
}

fn names(nodes: &[crate::ax::AxNode], role: &str) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.role == role)
        .map(|node| node.name.clone())
        .collect()
}

/// Route 1: two bare `StyledText`s under one parent, as the SDK makes them.
#[gpui_kit::test]
fn two_id_less_rich_texts_are_both_heard(cx: &mut gpui_kit::TestAppContext) {
    let nodes = draw(
        cx,
        axis_container(
            "root",
            Axis::Column,
            [rich(None, "alpha"), rich(None, "beta")],
        ),
    );
    assert_eq!(names(&nodes, "Label"), ["alpha", "beta"]);
}

/// The same pair inside a box with no id of its own.
#[gpui_kit::test]
fn two_id_less_rich_texts_under_an_id_less_box_are_both_heard(cx: &mut gpui_kit::TestAppContext) {
    let nodes = draw(
        cx,
        axis_container(
            "root",
            Axis::Column,
            [box_of(None, vec![rich(None, "alpha"), rich(None, "beta")])],
        ),
    );
    assert_eq!(names(&nodes, "Label"), ["alpha", "beta"]);
}

/// A view spelling the host's old numbered id beside an id-less text; the
/// id-less one's door id is its host name, not a code location.
#[gpui_kit::test]
fn a_view_text_spelled_like_a_host_text_id_is_its_own_node(cx: &mut gpui_kit::TestAppContext) {
    let nodes = draw(
        cx,
        axis_container(
            "root",
            Axis::Column,
            [
                plain_text(None, "first"),
                plain_text(
                    Some(wire::ElementIdWire::NamedInteger("guest-text".into(), 0)),
                    "second",
                ),
            ],
        ),
    );
    assert_eq!(names(&nodes, "Label"), ["first", "second"]);
    let first = nodes.iter().find(|node| node.name == "first").unwrap();
    assert_eq!(first.id, "t:text-0");
}

/// An id-less rich text with links, beside a text spelled the way the host
/// used to name the links' box: the box and the text keep their nodes.
#[gpui_kit::test]
fn a_view_text_spelled_like_a_linked_box_is_its_own_node(cx: &mut gpui_kit::TestAppContext) {
    let mut linked = rich(None, "Read the docs or the code");
    if let wire::Node::RichText {
        clickable_ranges,
        on_click,
        ..
    } = &mut linked
    {
        *clickable_ranges = vec![5..13, 17..25];
        *on_click = Some(7);
    }
    let nodes = draw(
        cx,
        axis_container(
            "root",
            Axis::Column,
            [
                linked,
                plain_text(
                    Some(wire::ElementIdWire::NamedInteger("guest-rich".into(), 0)),
                    "beside",
                ),
            ],
        ),
    );
    assert_eq!(names(&nodes, "Group"), ["Read the docs or the code"]);
    assert!(names(&nodes, "Label").contains(&"beside".to_owned()));
}

/// An input `i` beside a text spelled the way the host used to name the
/// input's wrapper; the input's door id is the view's own.
#[gpui_kit::test]
fn a_view_text_spelled_like_an_inputs_field_is_its_own_node(cx: &mut gpui_kit::TestAppContext) {
    let nodes = draw(
        cx,
        axis_container(
            "root",
            Axis::Column,
            [
                input("Room name", false, false),
                plain_text(
                    Some(wire::ElementIdWire::NamedChild {
                        base: view_wire::ElementIdAtom::Name("i".into()),
                        names: vec!["field".into()],
                    }),
                    "beside",
                ),
            ],
        ),
    );
    assert_eq!(names(&nodes, "TextInput"), ["Room name"]);
    assert_eq!(names(&nodes, "Label"), ["beside"]);
    let field = nodes.iter().find(|node| node.role == "TextInput").unwrap();
    assert_eq!(field.id, "t:i");
}

/// A named overlay holding another named one, whose base text is spelled
/// `layer`: both dialogs and the text keep their nodes.
#[gpui_kit::test]
fn a_view_text_spelled_layer_keeps_a_nested_dialog_its_node(cx: &mut gpui_kit::TestAppContext) {
    let overlay =
        |key: &str, label: &str, base: wire::Node, modal: wire::Node| wire::Node::Overlay {
            id: named_id(key),
            label: Some(label.into()),
            on_dismiss: None,
            children: vec![base, modal],
            style: div().size_full().style().clone(),
        };
    let inner = overlay(
        "inner",
        "Inner",
        plain_text(Some(named_id("under")), "under"),
        plain_text(Some(named_id("sheet")), "sheet"),
    );
    let outer = overlay(
        "outer",
        "Outer",
        plain_text(Some(named_id("layer")), "base"),
        inner,
    );
    // gpui's own tree, not the door's: the door shows only the topmost modal
    cx.update(gpui_kit::init);
    let root = sized("root", outer, Some(fill()), Some(fill()));
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let labels = native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        let tree = window.a11y_tree().unwrap();
        let mut labels: Vec<(gpui_kit::Role, String)> = tree
            .nodes
            .iter()
            .filter_map(|(_, node)| Some((node.role(), node.label()?.to_owned())))
            .collect();
        labels.sort();
        labels
    });
    let dialogs: Vec<&str> = labels
        .iter()
        .filter(|(role, _)| *role == gpui_kit::Role::Dialog)
        .map(|(_, name)| name.as_str())
        .collect();
    assert_eq!(dialogs, ["Inner", "Outer"]);
    assert!(labels.contains(&(gpui_kit::Role::Label, "base".to_owned())));
}

/// Two clickable containers, the second spelled like the host's old id for
/// the first: a pointer click on the second reaches its own handler.
#[gpui_kit::test]
fn a_click_on_a_container_spelled_like_a_host_id_fires_its_own_handler(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let root = axis_container(
        "root",
        Axis::Column,
        [
            button(None, "A", 1),
            button(
                Some(wire::ElementIdWire::NamedInteger(
                    "guest-container".into(),
                    0,
                )),
                "B",
                2,
            ),
        ],
    );
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| window.render_frame(cx));
    // the second button: 24px of the first, an 8px gap, then its own 24px
    let centre = point(px(40.), px(24. + 8. + 12.));
    native.simulate_mouse_move(centre, None, Default::default());
    native.simulate_mouse_down(centre, MouseButton::Left, Default::default());
    native.simulate_mouse_up(centre, MouseButton::Left, Default::default());
    let clicked: Vec<u32> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::Click { handler, .. } => Some(*handler),
            _ => None,
        })
        .collect();
    assert_eq!(clicked, [2]);
}
