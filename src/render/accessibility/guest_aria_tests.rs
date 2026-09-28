//! The three renderers that carry the guest's own `Interactivity.aria` must
//! keep announcing exactly what they did before the setter chain was folded
//! into `guest_aria`: the properties AccessKit receives, read off the tree
//! gpui hands the OS, not off `ax::snapshot` (which drops half of them).
use super::*;
use gpui_kit::accesskit::{self, Action, NodeId, Toggled, TreeUpdate};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Role, size};

const PARENT_FOCUS: u64 = 7;
const WORDS: &str = "Named by text";

fn key(name: &str) -> wire::ElementIdWire {
    wire::ElementIdWire::Name(name.into())
}

fn text(id: &str) -> wire::Node {
    wire::Node::Text(view_wire::TextNode {
        id: Some(key(id)),
        style: Default::default(),
        content: WORDS.into(),
        heading: None,
        live: None,
    })
}

fn boxed() -> gpui_kit::StyleRefinement {
    div().w(px(200.)).h(px(96.)).style().clone()
}

/// Every aria field set to a value of its own, a role, focusable, the
/// active-descendant claim, and `label` as the pass asks.
fn every_aria(author_id: &str, label: Option<&str>) -> wire::Interactivity {
    wire::Interactivity {
        role: Some(Role::Button),
        focusable: true,
        aria: wire::Aria {
            author_id: Some(author_id.into()),
            label: label.map(Into::into),
            description: Some("described".into()),
            keyshortcuts: Some("Ctrl+K".into()),
            active_descendant: true,
            value: Some("worth".into()),
            placeholder: Some("hint".into()),
            selected: Some(true),
            expanded: Some(false),
            disabled: Some(true),
            numeric_value: Some(3.),
            numeric_value_step: Some(0.5),
            min_numeric_value: Some(1.),
            max_numeric_value: Some(9.),
            level: Some(2),
            position_in_set: Some(4),
            size_of_set: Some(6),
            row_index: Some(1),
            column_index: Some(2),
            row_count: Some(5),
            column_count: Some(3),
            toggled: Some(Toggled::Mixed),
            orientation: Some(accesskit::Orientation::Vertical),
        },
        ..Default::default()
    }
}

/// What one node's renderer announces, as the three call sites build it.
enum Site {
    Container,
    Image,
    UniformList,
}

fn child(site: &Site, interactivity: wire::Interactivity) -> wire::Node {
    match site {
        Site::Container => wire::Node::Container(view_wire::ContainerNode {
            id: Some(key("child")),
            style: boxed(),
            interactivity,
            children: vec![text("child-text")],
        }),
        // no picture yet: the loading child, a text, is what is drawn
        Site::Image => wire::Node::Image {
            id: Some(key("child")),
            hash: 0,
            data: None,
            label: None,
            image_style: wire::ImageStyle {
                grayscale: false,
                object_fit: wire::ImageObjectFit::Contain,
            },
            loading: true,
            fallback: false,
            state_children: vec![text("child-text")],
            style: boxed(),
            interactivity,
        },
        Site::UniformList => wire::Node::UniformList {
            id: key("child"),
            path: vec![key("child")],
            route: 1,
            style: boxed(),
            interactivity,
            count: 1,
            measure_index: 0,
            sizing: wire::list::UniformListSizing::Auto,
            horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
            y_flipped: false,
            scroll_request: None,
            indices: vec![0],
            children: vec![wire::Node::Container(view_wire::ContainerNode {
                id: Some(key("child/row:0")),
                style: boxed(),
                interactivity: Default::default(),
                children: vec![text("child-text")],
            })],
        },
    }
}

/// The child under a focusable, roled parent that holds the guest focus
/// handle: the composite an active descendant is announced within.
fn root(site: &Site, label: Option<&str>) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("parent")),
        style: div().w(px(280.)).h(px(180.)).style().clone(),
        interactivity: wire::Interactivity {
            role: Some(Role::Group),
            focusable: true,
            focus_handle: Some(PARENT_FOCUS),
            aria: wire::Aria {
                author_id: Some("parent".into()),
                label: Some("Parent".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        children: vec![child(site, every_aria("child", label))],
    })
}

/// Everything gpui writes onto a node from the guest's aria.
#[derive(Debug, PartialEq)]
struct Heard {
    role: Role,
    name: Option<String>,
    description: Option<String>,
    keyshortcuts: Option<String>,
    value: Option<String>,
    placeholder: Option<String>,
    selected: Option<bool>,
    expanded: Option<bool>,
    disabled: bool,
    numeric: Option<f64>,
    step: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    level: Option<usize>,
    position_in_set: Option<usize>,
    size_of_set: Option<usize>,
    row_index: Option<usize>,
    column_index: Option<usize>,
    row_count: Option<usize>,
    column_count: Option<usize>,
    toggled: Option<Toggled>,
    orientation: Option<accesskit::Orientation>,
    focus_action: bool,
}

fn heard(update: &TreeUpdate, author_id: &str) -> Option<(NodeId, Heard)> {
    let (id, node) = update
        .nodes
        .iter()
        .find(|(_, node)| node.author_id() == Some(author_id))?;
    let owned = |text: Option<&str>| text.map(str::to_owned);
    Some((
        *id,
        Heard {
            role: node.role(),
            name: owned(node.label()),
            description: owned(node.description()),
            keyshortcuts: owned(node.keyboard_shortcut()),
            value: owned(node.value()),
            placeholder: owned(node.placeholder()),
            selected: node.is_selected(),
            expanded: node.is_expanded(),
            disabled: node.is_disabled(),
            numeric: node.numeric_value(),
            step: node.numeric_value_step(),
            min: node.min_numeric_value(),
            max: node.max_numeric_value(),
            level: node.level(),
            position_in_set: node.position_in_set(),
            size_of_set: node.size_of_set(),
            row_index: node.row_index(),
            column_index: node.column_index(),
            row_count: node.row_count(),
            column_count: node.column_count(),
            toggled: node.toggled(),
            orientation: node.orientation(),
            focus_action: node.supports_action(Action::Focus),
        },
    ))
}

fn expected(name: Option<&str>) -> Heard {
    Heard {
        role: Role::Button,
        name: name.map(str::to_owned),
        description: Some("described".into()),
        keyshortcuts: Some("Ctrl+K".into()),
        value: Some("worth".into()),
        placeholder: Some("hint".into()),
        selected: Some(true),
        expanded: Some(false),
        disabled: true,
        numeric: Some(3.),
        step: Some(0.5),
        min: Some(1.),
        max: Some(9.),
        level: Some(2),
        position_in_set: Some(4),
        size_of_set: Some(6),
        row_index: Some(1),
        column_index: Some(2),
        row_count: Some(5),
        column_count: Some(3),
        toggled: Some(Toggled::Mixed),
        orientation: Some(accesskit::Orientation::Vertical),
        focus_action: true,
    }
}

/// Renders `root`, focuses the parent through its guest focus handle,
/// and returns the child's node as heard (None when the tree has no node
/// for it) plus whether the tree reports the child as the focused
/// (active descendant) node.
fn announce(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> (Option<Heard>, bool) {
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        let parent = tree
            .read(cx)
            .guest_focus_targets
            .get(&PARENT_FOCUS)
            .cloned()
            .expect("the parent's guest focus handle is made on first render");
        parent.focus(window, cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let (parent_id, _) = heard(update, "parent").expect("the parent has a node");
        let Some((child_id, child)) = heard(update, "child") else {
            assert_eq!(update.focus, parent_id);
            return (None, false);
        };
        assert!(
            update.focus == child_id || update.focus == parent_id,
            "focus is on the parent or its active descendant, not {:?}",
            update.focus
        );
        (Some(child), update.focus == child_id)
    })
}

/// Each renderer keeps announcing what its own copy of the chain set:
/// every aria property alike; a roled Container with no label named by
/// its descendant text and honouring `active_descendant`; an Image
/// unnamed, leaving focus on the parent (the `Drift` the owner decides
/// on). A UniformList has no node at all: gpui's `UniformList` element
/// reports no `a11y_role`, so its whole chain is dead until the fork
/// gives it one. Written and green before the fold, kept after it.
#[gpui_kit::test]
fn guest_aria_announces_what_each_renderer_did(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for (site, unlabelled_name, active) in [
        (Site::Container, Some(WORDS), true),
        (Site::Image, None, false),
    ] {
        let (unlabelled, focus_on_child) = announce(cx, root(&site, None));
        assert_eq!(unlabelled, Some(expected(unlabelled_name)));
        assert_eq!(focus_on_child, active, "active descendant of the parent");
        let (labelled, focus_on_child) = announce(cx, root(&site, Some("Labelled")));
        assert_eq!(labelled, Some(expected(Some("Labelled"))));
        assert_eq!(focus_on_child, active);
    }
    for label in [None, Some("Labelled")] {
        assert_eq!(announce(cx, root(&Site::UniformList, label)), (None, false));
    }
}
