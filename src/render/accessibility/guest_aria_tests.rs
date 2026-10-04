//! The renderers that carry the guest's own `Interactivity.aria` announce
//! it alike through the one mapper, `guest_aria`: the properties AccessKit
//! receives, read off the tree gpui hands the OS, not off `ax::snapshot`
//! (which drops half of them).
use super::*;
use crate::render::tests::emitted;
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
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `every_aria` on a node nothing can focus: the one that may claim the
/// active descendant.
fn unfocusable(label: Option<&str>) -> wire::Interactivity {
    wire::Interactivity {
        focusable: false,
        ..every_aria("child", label)
    }
}

/// Where the aria is put: on a Container, an Image, a UniformList itself,
/// the one row of a UniformList, a ResizeHandle or a (variable) List.
#[derive(Debug)]
enum Site {
    Container,
    Image,
    UniformList,
    UniformListRow,
    ResizeHandle,
    List,
}

fn child(site: &Site, interactivity: wire::Interactivity) -> wire::Node {
    match site {
        Site::Container => wire::Node::Container(view_wire::ContainerNode {
            id: Some(key("child")),
            style: boxed(),
            interactivity: Box::new(interactivity),
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
            interactivity: Box::new(interactivity),
        },
        Site::UniformList => wire::Node::UniformList {
            id: key("child"),
            path: vec![key("child")],
            route: 1,
            style: boxed(),
            interactivity: Box::new(interactivity),
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
        Site::UniformListRow => wire::Node::UniformList {
            id: key("list"),
            path: vec![key("list")],
            route: 1,
            style: boxed(),
            interactivity: Default::default(),
            count: 1,
            measure_index: 0,
            sizing: wire::list::UniformListSizing::Auto,
            horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
            y_flipped: false,
            scroll_request: None,
            indices: vec![0],
            children: vec![child(&Site::Container, interactivity)],
        },
        Site::ResizeHandle => wire::Node::ResizeHandle {
            id: key("child"),
            style: boxed(),
            interactivity: Box::new(interactivity),
            on_press: None,
            on_release: None,
            on_drag: None,
            cursor: None,
            content: Box::new(text("child-text")),
        },
        // a list's id is its state's, which no author writes
        Site::List => wire::Node::List {
            id: wire::ElementIdWire::ListState(1),
            path: vec![wire::ElementIdWire::ListState(1)],
            item_count: 1,
            alignment: wire::ListAlignment::Top,
            overdraw: 0.,
            sizing: wire::ListSizingBehavior::Auto,
            following_tail: false,
            revision: 0,
            commands: Vec::new(),
            request_handler: 1,
            scroll_handler: None,
            range_start: 0,
            style: boxed(),
            interactivity: Box::new(interactivity),
            children: vec![text("child-text")],
        },
    }
}

/// The child under a focusable, roled parent that holds the guest focus
/// handle: the composite an active descendant is announced within.
fn root(site: &Site, interactivity: wire::Interactivity) -> wire::Node {
    let mut child = child(site, interactivity);
    // a list's path is the one the walk takes to it, through the parent
    child.for_each_mut(&mut |node| {
        if let wire::Node::UniformList { path, .. } | wire::Node::List { path, .. } = node {
            path.insert(0, key("parent"));
        }
    });
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("parent")),
        style: div().w(px(280.)).h(px(180.)).style().clone(),
        interactivity: Box::new(wire::Interactivity {
            role: Some(Role::Group),
            focusable: true,
            focus_handle: Some(PARENT_FOCUS),
            aria: wire::Aria {
                author_id: Some("parent".into()),
                label: Some("Parent".into()),
                ..Default::default()
            },
            ..Default::default()
        }),
        children: vec![child],
    })
}

/// `root` as the host takes it from a guest: through the sanitizer.
fn sanitized(root: wire::Node) -> wire::Node {
    let mut frame = wire::Frame {
        root: Some(root),
        ..Default::default()
    };
    wire::sanitize(&mut frame).expect("the host takes the tree");
    frame.root.expect("the tree stays")
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

fn expected(name: &str, focusable: bool) -> Heard {
    Heard {
        role: Role::Button,
        name: Some(name.to_owned()),
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
        focus_action: focusable,
    }
}

/// Renders `root`, focuses the parent through its guest focus handle,
/// and returns the child's node as heard (None when the tree has no node
/// for it) plus whether the tree reports the child as the focused
/// (active descendant) node.
fn announce(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> (Option<Heard>, bool) {
    let root = sanitized(root);
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

/// Every renderer announces the guest's aria alike, a roled node with no
/// label named by the text it draws; a focusable node never claims the
/// active descendant (gpui panics, in debug, when the claimant is the
/// focused node): the sanitizer drops its claim, so focus stays on the
/// parent. A UniformList itself has
/// no node at all: gpui's `UniformList` element reports no `a11y_role`, so
/// its aria is dead until the fork gives it one.
#[gpui_kit::test]
fn guest_aria_announces_what_each_renderer_did(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for site in [Site::Container, Site::Image, Site::UniformListRow] {
        for (label, name) in [(None, WORDS), (Some("Labelled"), "Labelled")] {
            let (heard, focus_on_child) = announce(cx, root(&site, every_aria("child", label)));
            assert_eq!(heard, Some(expected(name, true)));
            assert!(
                !focus_on_child,
                "a focusable node claims no active descendant"
            );
        }
    }
    for label in [None, Some("Labelled")] {
        let list = root(&Site::UniformList, every_aria("child", label));
        assert_eq!(announce(cx, list), (None, false));
    }
}

/// The same Aria on a row of a UniformList and on an Image is what it is
/// on a Container: every property, the name from the drawn text when
/// unlabelled, and the active descendant within the focused parent.
#[gpui_kit::test]
fn a_uniform_list_row_and_an_image_get_the_aria_a_container_gets(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    for label in [None, Some("Labelled")] {
        let container = announce(cx, root(&Site::Container, unfocusable(label)));
        assert_eq!(
            container,
            (Some(expected(label.unwrap_or(WORDS), false)), true)
        );
        for site in [Site::UniformListRow, Site::Image] {
            assert_eq!(announce(cx, root(&site, unfocusable(label))), container);
        }
    }
}

/// A picture the guest labelled and gave no role is an Image named by its
/// label, as `accessible` says; an unlabelled one stays out of the tree.
#[gpui_kit::test]
fn a_labelled_picture_without_a_role_is_an_image_in_the_tree(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for label in [Some("Ada's avatar"), None] {
        let aria = wire::Interactivity {
            aria: wire::Aria {
                author_id: Some("picture".into()),
                // the SDK sends the label on the node and in its aria
                label: label.map(Into::into),
                ..Default::default()
            },
            ..Default::default()
        };
        let image = wire::Node::Image {
            id: Some(key("picture")),
            hash: 0,
            data: None,
            label: label.map(Into::into),
            image_style: wire::ImageStyle {
                grayscale: false,
                object_fit: wire::ImageObjectFit::Contain,
            },
            loading: false,
            fallback: false,
            state_children: Vec::new(),
            style: boxed(),
            interactivity: Box::new(aria.clone()),
        };
        let vector = wire::Node::Svg {
            id: Some(key("picture")),
            source: wire::SvgSource::None,
            transformation: wire::SvgTransformation {
                scale: [1., 1.],
                translate: [0., 0.],
                rotate: 0.,
            },
            label: label.map(Into::into),
            style: boxed(),
            interactivity: Box::new(aria),
        };
        for picture in [image, vector] {
            let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(picture));
            let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
            let heard = native.update(|window, cx| {
                window.activate_a11y();
                window.render_frame(cx);
                window.render_frame(cx);
                let update = window.a11y_tree().expect("an a11y tree once activated");
                heard(update, "picture").map(|(_, heard)| (heard.role, heard.name))
            });
            let want = label.map(|label| (Role::Image, Some(label.to_owned())));
            assert_eq!(heard, want);
        }
    }
}

/// A view's own focus handle is a Tab stop when the view says the element
/// is one, as gpui makes the handle of an element it gives one; one the
/// view left out of the Tab order stays out.
#[gpui_kit::test]
fn a_tracked_handle_the_view_makes_a_tab_stop_takes_tab(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let button = |key_name: &str, focus: u64, tab_stop: Option<bool>| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(key_name)),
            style: div().w(px(80.)).h(px(40.)).style().clone(),
            interactivity: Box::new(wire::Interactivity {
                role: Some(Role::Button),
                focusable: true,
                focus_handle: Some(focus),
                tab_stop,
                aria: wire::Aria {
                    author_id: Some(key_name.into()),
                    label: Some(key_name.into()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            children: Vec::new(),
        })
    };
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("row")),
        style: div().flex().w(px(280.)).h(px(80.)).style().clone(),
        interactivity: Default::default(),
        children: vec![button("skipped", 1, None), button("stop", 2, Some(true))],
    });
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(sanitized(root))
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        // Tab is the app's binding to `focus_next`; a bare window has none
        window.focus_next(cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let (stop, _) = heard(update, "stop").expect("the stop has a node");
        assert_eq!(update.focus, stop, "Tab reaches the view's tab stop");
    });
}

/// A pressable button that is `focusable`, with the Tab stop `tab_stop`,
/// alone in a row: whether its node offers focus to assistive technology,
/// and whether the first Tab lands on it.
fn pressable_after_tab(cx: &mut gpui_kit::TestAppContext, tab_stop: Option<bool>) -> (bool, bool) {
    cx.update(gpui_kit::init);
    let button = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("press")),
        style: div().w(px(80.)).h(px(40.)).style().clone(),
        interactivity: Box::new(wire::Interactivity {
            role: Some(Role::Button),
            focusable: true,
            tab_stop,
            on_click: Some(7),
            aria: wire::Aria {
                author_id: Some("press".into()),
                label: Some("Rename".into()),
                ..Default::default()
            },
            ..Default::default()
        }),
        children: Vec::new(),
    });
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("row")),
        style: div().flex().w(px(280.)).h(px(80.)).style().clone(),
        interactivity: Default::default(),
        children: vec![button],
    });
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(sanitized(root))
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        // Tab is the app's binding to `focus_next`; a bare window has none
        window.focus_next(cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let (press, heard) = heard(update, "press").expect("the button has a node");
        (heard.focus_action, update.focus == press)
    })
}

/// A view control as the SDK lowers `focusable()` (option B: `tab_stop:
/// Some(true)` unless the view said otherwise) is a Tab stop the host
/// honours: Tab lands on it.
#[gpui_kit::test]
fn a_lowered_focusable_with_tab_stop_takes_tab(cx: &mut gpui_kit::TestAppContext) {
    assert_eq!(pressable_after_tab(cx, Some(true)), (true, true));
}

/// A raw-wire guest's `focusable` without a `tab_stop` is taken at its
/// word: the node offers focus to assistive technology and Tab skips it.
/// The host adds no stop the wire did not ask for (79b524d3); the SDK's
/// lint faults such a control as `Unreachable`.
#[gpui_kit::test]
fn a_raw_focusable_without_tab_stop_offers_focus_but_tab_skips_it(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (offers, reached) = pressable_after_tab(cx, None);
    assert!(offers, "the door reads it as offering focus");
    assert!(!reached, "Tab skips a focusable the wire left out");
}

/// A container the view focuses by id (a menu frame it opens) stays in the
/// Tab order: the handle the host makes for it takes the wire's Tab stop,
/// as the handle gpui makes for an element does.
#[gpui_kit::test]
fn a_container_the_view_focuses_by_id_stays_a_tab_stop(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let stop = |key_name: &str, role: Role| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(key_name)),
            style: div().w(px(80.)).h(px(40.)).style().clone(),
            interactivity: Box::new(wire::Interactivity {
                role: Some(role),
                focusable: true,
                tab_stop: Some(true),
                aria: wire::Aria {
                    author_id: Some(key_name.into()),
                    label: Some(key_name.into()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            children: Vec::new(),
        })
    };
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("row")),
        style: div().flex().w(px(280.)).h(px(80.)).style().clone(),
        interactivity: Default::default(),
        children: vec![stop("menu", Role::Menu), stop("next", Role::Button)],
    });
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(sanitized(root))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let focused = |native: &mut gpui_kit::VisualTestContext| {
        native.update(|window, cx| {
            window.render_frame(cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            ["menu", "next"]
                .into_iter()
                .find(|name| heard(update, name).is_some_and(|(id, _)| id == update.focus))
        })
    };
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        tree.update(cx, |tree, cx| {
            let target = vec![key("row"), key("menu")];
            tree.execute_widget_command(wire::WidgetCommand::Focus { target }, window, cx)
                .unwrap();
        });
    });
    assert_eq!(focused(&mut native), Some("menu"), "the view focused it");
    // Tab is the app's binding to `focus_next`; a bare window has none
    native.update(|window, cx| window.focus_next(cx));
    assert_eq!(focused(&mut native), Some("next"));
    native.update(|window, cx| window.focus_next(cx));
    assert_eq!(
        focused(&mut native),
        Some("menu"),
        "Tab comes back round to the container it focused"
    );
}

/// A focusable composite `id` of three `item` rows whose second is its
/// active descendant, as the SDK builds one.
fn claiming_composite(id: &str, role: Role, item: Role) -> wire::Node {
    let rows = (0..3)
        .map(|n| {
            let row = format!("{id}-{n}");
            wire::Node::Container(view_wire::ContainerNode {
                id: Some(key(&row)),
                style: div().w(px(60.)).h(px(20.)).style().clone(),
                interactivity: Box::new(wire::Interactivity {
                    role: Some(item),
                    aria: wire::Aria {
                        author_id: Some(row.clone().into()),
                        label: Some(row.into()),
                        selected: Some(n == 1),
                        active_descendant: n == 1,
                        ..Default::default()
                    },
                    on_click: Some(20 + n),
                    ..Default::default()
                }),
                children: Vec::new(),
            })
        })
        .collect();
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(key(id)),
        style: div().flex().w(px(200.)).h(px(20.)).style().clone(),
        interactivity: Box::new(wire::Interactivity {
            role: Some(role),
            focusable: true,
            tab_stop: Some(true),
            on_key_down: Some(9),
            aria: wire::Aria {
                author_id: Some(id.into()),
                label: Some(id.into()),
                ..Default::default()
            },
            ..Default::default()
        }),
        children: rows,
    })
}

/// During a hold (⌘⇧M) the shell's pane box has the keys and is an
/// ancestor of every claim its view makes. With two composites on the
/// screen each claiming its active row (Forge's Code: a tab list and the
/// tree), neither has the keys, so neither claim counts: the pane box is
/// what assistive technology hears focused, not a row, and no build
/// panics on two claims under one focused box.
#[gpui_kit::test]
fn a_held_pane_over_two_claiming_composites_reports_the_pane_box(
    cx: &mut gpui_kit::TestAppContext,
) {
    struct Pane {
        view: Entity<ViewTree>,
        own: gpui_kit::FocusHandle,
    }
    impl Render for Pane {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("pane")
                .role(Role::Group)
                .aria_label("Held window")
                .track_focus(&self.own)
                .size_full()
                .child(self.view.clone())
        }
    }
    cx.update(gpui_kit::init);
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("screen")),
        style: div()
            .flex()
            .flex_col()
            .w(px(280.))
            .h(px(80.))
            .style()
            .clone(),
        interactivity: Default::default(),
        children: vec![
            claiming_composite("tabs", Role::TabList, Role::Tab),
            claiming_composite("files", Role::ListBox, Role::ListBoxOption),
        ],
    });
    let root = sanitized(root);
    let claims = |node: &wire::Node| {
        let mut claims = 0;
        super::super::commands::walk_authored_paths(node, None, &mut Vec::new(), &mut |node, _| {
            if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = node {
                claims += usize::from(interactivity.aria.active_descendant);
            }
        });
        claims
    };
    let kept = claims(&root);
    let window = cx.open_window(size(px(300.), px(200.)), |_, cx| Pane {
        view: cx.new(|_| ViewTree::new(root)),
        own: cx.focus_handle(),
    });
    let pane = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        let own = pane.read(cx).own.clone();
        own.focus(window, cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let focused = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.focus)
            .map(|(_, node)| (node.role(), node.label().map(str::to_owned)));
        assert_eq!(focused, Some((Role::Group, Some("Held window".into()))));
    });
    assert_eq!(kept, 2, "the sanitizer keeps a claim per composite");
}

mod phase_two {
    //! What phase 2 added to the one mapper: the aria gpui has no setter for,
    //! through one `a11y::Patch`; a Status's words as its value; a List and a
    //! ResizeHandle carrying a view's interactivity.
    use super::*;

    /// Every phase-2 field a view sets that gpui has no setter for, on a
    /// roled, named node.
    fn phase_two(role: Role) -> wire::Interactivity {
        wire::Interactivity {
            role: Some(role),
            aria: wire::Aria {
                author_id: Some("child".into()),
                label: Some("Phase two".into()),
                live: Some(accesskit::Live::Polite),
                busy: true,
                required: true,
                read_only: true,
                invalid: Some(accesskit::Invalid::Spelling),
                has_popup: Some(accesskit::HasPopup::Menu),
                current: Some(accesskit::AriaCurrent::Page),
                custom_actions: vec![(3, "Pin".into())],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// The node `root` builds for author id `child`, once the tree is on.
    fn built(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> Option<accesskit::Node> {
        let root = sanitized(root);
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            update
                .nodes
                .iter()
                .find(|(_, node)| node.author_id() == Some("child"))
                .map(|(_, node)| node.clone())
        })
    }

    /// Each phase-2 aria field reaches the node, all of them through one
    /// patch (a second closure would replace the first), on every renderer
    /// that carries a view's interactivity: a ResizeHandle and a List too.
    #[gpui_kit::test]
    fn every_renderer_puts_the_phase_two_aria_on_its_node(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for site in [
            Site::Container,
            Site::Image,
            Site::UniformListRow,
            Site::ResizeHandle,
            Site::List,
        ] {
            let node = built(cx, child(&site, phase_two(Role::Button)))
                .unwrap_or_else(|| panic!("{site:?} has a node"));
            assert_eq!(node.role(), Role::Button);
            assert_eq!(node.label(), Some("Phase two"));
            assert_eq!(node.live(), Some(accesskit::Live::Polite));
            assert!(node.is_busy());
            assert!(node.is_required());
            assert!(node.is_read_only());
            assert_eq!(node.invalid(), Some(accesskit::Invalid::Spelling));
            assert_eq!(node.has_popup(), Some(accesskit::HasPopup::Menu));
            assert_eq!(node.aria_current(), Some(accesskit::AriaCurrent::Page));
            assert_eq!(
                node.custom_actions()
                    .iter()
                    .map(|action| (action.id, &*action.description))
                    .collect::<Vec<_>>(),
                [(3, "Pin")]
            );
        }
    }

    /// A list the view left plain is gpui's list alone: no box, no node.
    #[gpui_kit::test]
    fn a_plain_list_has_no_node(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let mut plain = wire::Interactivity::default();
        let node = built(cx, child(&Site::List, plain.clone()));
        assert!(node.is_none());
        plain.aria.author_id = Some("child".into());
        plain.role = Some(Role::List);
        assert_eq!(
            built(cx, child(&Site::List, plain)).map(|node| node.role()),
            Some(Role::List)
        );
    }

    /// A Status or Alert the view did not give a value speaks the words it
    /// draws as its value as well as its name: macOS reads the value.
    #[gpui_kit::test]
    fn a_status_speaks_its_drawn_words_as_its_value(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        for role in [Role::Status, Role::Alert] {
            let status = wire::Interactivity {
                role: Some(role),
                aria: wire::Aria {
                    author_id: Some("child".into()),
                    live: Some(accesskit::Live::Polite),
                    ..Default::default()
                },
                ..Default::default()
            };
            let node = built(cx, child(&Site::Container, status)).expect("the status has a node");
            assert_eq!(node.label(), Some(WORDS));
            assert_eq!(node.value(), Some(WORDS));
        }
    }

    /// A divider as the SDK builds it is a named Splitter Tab reaches, and a
    /// key pressed on it goes to the view.
    #[gpui_kit::test]
    fn a_resize_handle_takes_focus_and_keys(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let divider = wire::Interactivity {
            role: Some(Role::Splitter),
            focusable: true,
            tab_stop: Some(true),
            on_key_down: Some(9),
            aria: wire::Aria {
                author_id: Some("child".into()),
                label: Some("Resize the list".into()),
                orientation: Some(accesskit::Orientation::Vertical),
                ..Default::default()
            },
            ..Default::default()
        };
        let root = child(&Site::ResizeHandle, divider);
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, _subscription) = emitted(&tree, &mut native);
        // Tab is the app's binding to `focus_next`; a bare window has none
        native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.focus_next(cx);
        });
        native.simulate_keystrokes("left");
        native.update(|window, cx| {
            window.render_frame(cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            let (id, node) = update
                .nodes
                .iter()
                .find(|(_, node)| node.author_id() == Some("child"))
                .expect("the divider has a node");
            assert_eq!(node.role(), Role::Splitter);
            assert_eq!(node.label(), Some("Resize the list"));
            assert_eq!(update.focus, *id, "Tab reaches the divider");
        });
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, wire::Event::KeyDown { handler: 9, .. })),
            "{:?}",
            events.borrow()
        );
    }

    /// A divider's arrows move it, end to end: Tab reaches it, each arrow
    /// reaches the view's key route as the keystroke the SDK's divider reads,
    /// and the view, stepping as `design::divider` does (8 px, 32 with
    /// shift), draws its pane that much wider or narrower. No built view
    /// wasm is loadable here, so the view is this test, re-rendering from
    /// its key route.
    #[gpui_kit::test]
    fn a_dividers_arrows_move_its_pane_by_eight_or_thirty_two(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        const ROUTE: u32 = 9;
        let panes = |width: f32| {
            let named = |role: Role, label: &str| wire::Interactivity {
                role: Some(role),
                aria: wire::Aria {
                    label: Some(label.into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let pane = wire::Node::Container(view_wire::ContainerNode {
                id: Some(key("list-pane")),
                style: div().w(px(width)).h_full().flex_shrink_0().style().clone(),
                interactivity: Box::new(named(Role::Group, "List")),
                children: Vec::new(),
            });
            let divider = wire::Node::ResizeHandle {
                id: key("list-resize"),
                style: div().w(px(1.)).h_full().style().clone(),
                interactivity: Box::new(wire::Interactivity {
                    focusable: true,
                    tab_stop: Some(true),
                    on_key_down: Some(ROUTE),
                    aria: wire::Aria {
                        orientation: Some(accesskit::Orientation::Vertical),
                        ..named(Role::Splitter, "Resize the list").aria
                    },
                    ..named(Role::Splitter, "Resize the list")
                }),
                on_press: None,
                on_release: None,
                on_drag: None,
                cursor: None,
                content: Box::new(wire::Node::Container(view_wire::ContainerNode {
                    id: Some(key("list-rule")),
                    style: div().w(px(1.)).h_full().style().clone(),
                    interactivity: Default::default(),
                    children: Vec::new(),
                })),
            };
            sanitized(wire::Node::Container(view_wire::ContainerNode {
                id: Some(key("panes")),
                style: div().flex().w(px(600.)).h(px(200.)).style().clone(),
                interactivity: Default::default(),
                children: vec![pane, divider],
            }))
        };
        let window = cx.open_window(size(px(640.), px(240.)), |_, _| ViewTree::new(panes(200.)));
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let (events, _subscription) = emitted(&tree, &mut native);
        let wide = |native: &mut gpui_kit::VisualTestContext| {
            native.update(|window, cx| {
                window.render_frame(cx);
                let nodes = serde_json::to_value(crate::ax::snapshot("t", window, true)).unwrap();
                let nodes = nodes.as_array().unwrap();
                let pane = nodes
                    .iter()
                    .find(|node| node["name"] == "List")
                    .expect("the pane is in the tree");
                let bounds = &pane["bounds"];
                let focus = nodes.iter().find(|node| {
                    node["state"]
                        .as_array()
                        .unwrap()
                        .contains(&"focused".into())
                });
                (
                    bounds[2].as_i64().unwrap() - bounds[0].as_i64().unwrap(),
                    focus.map(|node| node["role"].as_str().unwrap().to_owned()),
                )
            })
        };
        // Tab is the app's binding to `focus_next`; a bare window has none
        native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.focus_next(cx);
        });
        let mut width = 200.;
        assert_eq!(wide(&mut native), (200, Some("Splitter".into())));
        for (keystroke, moved) in [("right", 208), ("shift-right", 240), ("left", 232)] {
            native.simulate_keystrokes(keystroke);
            for event in events.borrow_mut().drain(..) {
                // the view: design::divider's step, on the event it is handed
                let wire::Event::KeyDown {
                    handler: ROUTE,
                    event,
                    ..
                } = event
                else {
                    continue;
                };
                let event = event.into_gpui();
                let step = match event.keystroke.modifiers.shift {
                    true => 32.,
                    false => 8.,
                };
                width += match event.keystroke.key.as_str() {
                    "left" => -step,
                    "right" => step,
                    _ => 0.,
                };
                native.update(|_, cx| {
                    tree.update(cx, |tree, cx| tree.replace(panes(width), &[], cx))
                });
            }
            assert_eq!(
                wide(&mut native),
                (moved, Some("Splitter".into())),
                "after {keystroke}"
            );
        }
    }

    /// A handle a view names on a divider or a list outlives the frame that
    /// made it: a new frame keeps focus where it was.
    #[gpui_kit::test]
    fn a_guest_focus_handle_on_a_divider_or_a_list_survives_a_frame(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        for site in [Site::ResizeHandle, Site::List] {
            let focused = wire::Interactivity {
                focus_handle: Some(PARENT_FOCUS),
                ..phase_two(Role::Button)
            };
            let root = child(&site, focused);
            let again = root.clone();
            let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
            let tree = window.root(cx).unwrap();
            let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
            native.update(|window, cx| {
                window.render_frame(cx);
                let handle = tree.read(cx).guest_focus_targets[&PARENT_FOCUS].clone();
                tree.update(cx, |tree, cx| tree.replace(again, &[], cx));
                window.render_frame(cx);
                assert!(
                    tree.read(cx).guest_focus_targets[&PARENT_FOCUS] == handle,
                    "{site:?} made its handle anew"
                );
            });
        }
    }

    /// A roled row named `row-<n>`, maybe wrapping a roled child `inner-<n>`.
    fn row(n: usize, role: Option<Role>) -> wire::Node {
        let named = |id: String| wire::Interactivity {
            role: Some(Role::ListBoxOption),
            aria: wire::Aria {
                author_id: Some(id.clone().into()),
                label: Some(id.into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let inner = wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(&format!("inner-{n}"))),
            style: div().h(px(20.)).style().clone(),
            interactivity: Box::new(named(format!("inner-{n}"))),
            children: Vec::new(),
        });
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(&format!("row-{n}"))),
            style: div().h(px(20.)).style().clone(),
            interactivity: Box::new(wire::Interactivity {
                role,
                ..named(format!("row-{n}"))
            }),
            children: vec![inner],
        })
    }

    /// `(author id, position, size)` of every node the rows built.
    fn set_places(
        cx: &mut gpui_kit::TestAppContext,
        root: wire::Node,
    ) -> Vec<(String, Option<usize>, Option<usize>)> {
        let root = sanitized(root);
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            let mut places: Vec<_> = update
                .nodes
                .iter()
                .filter_map(|(_, node)| {
                    let id = node.author_id()?;
                    (id.starts_with("row-") || id.starts_with("inner-"))
                        .then(|| (id.to_owned(), node.position_in_set(), node.size_of_set()))
                })
                .collect();
            places.sort();
            places
        })
    }

    /// Each row a virtualized list draws says where it is in the whole list,
    /// 1-based, unless the view said; what a row holds says nothing of it, and
    /// a row with no role has no node to say it on (AX-112).
    #[gpui_kit::test]
    fn a_list_row_says_where_it_is_in_the_list(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let mut told = row(2, Some(Role::ListBoxOption));
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut told {
            interactivity.aria.position_in_set = Some(9);
        }
        let uniform = wire::Node::UniformList {
            id: key("list"),
            path: vec![key("list")],
            route: 1,
            style: boxed(),
            interactivity: Default::default(),
            count: 5,
            measure_index: 0,
            sizing: wire::list::UniformListSizing::Auto,
            horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
            y_flipped: false,
            scroll_request: None,
            indices: vec![0, 1, 2],
            children: vec![row(0, Some(Role::ListBoxOption)), row(1, None), told],
        };
        let none = || (None, None);
        let place = |id: &str, (at, of): (Option<usize>, Option<usize>)| (id.to_owned(), at, of);
        assert_eq!(
            set_places(cx, uniform),
            [
                place("inner-0", none()),
                place("inner-1", none()),
                place("inner-2", none()),
                place("row-0", (Some(1), Some(5))),
                place("row-2", (Some(9), Some(5))),
            ]
        );
        let variable = wire::Node::List {
            id: wire::ElementIdWire::ListState(1),
            path: vec![wire::ElementIdWire::ListState(1)],
            item_count: 4,
            alignment: wire::ListAlignment::Top,
            overdraw: 0.,
            sizing: wire::ListSizingBehavior::Auto,
            following_tail: false,
            revision: 0,
            commands: Vec::new(),
            request_handler: 1,
            scroll_handler: None,
            range_start: 0,
            style: boxed(),
            interactivity: Default::default(),
            children: vec![
                row(0, Some(Role::ListBoxOption)),
                row(1, Some(Role::ListBoxOption)),
            ],
        };
        assert_eq!(
            set_places(cx, variable),
            [
                place("inner-0", none()),
                place("inner-1", none()),
                place("row-0", (Some(1), Some(4))),
                place("row-1", (Some(2), Some(4))),
            ]
        );
    }
}
