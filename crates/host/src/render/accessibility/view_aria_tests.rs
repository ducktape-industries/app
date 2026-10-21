//! The renderers that carry the view's own `Interactivity.aria` announce
//! it alike through the one mapper, `view_aria`: the properties AccessKit
//! receives, read off the tree gpui hands the OS, not off `ax::snapshot`
//! (which drops half of them).
use super::*;
use crate::render::tests::emitted;
use gpui_kit::accesskit::{self, Action, NodeId, Toggled, TreeUpdate};
use gpui_kit::{Role, size};

const PARENT_FOCUS: u64 = 7;
const WORDS: &str = "Named by text";

fn key(name: &str) -> wire::ElementIdWire {
    wire::ElementIdWire::Name(name.into())
}

fn text(id: &str) -> wire::Node {
    wire::Node::Text(view_wire::TextNode {
        id: Some(key(id)),
        style: crate::render::plain_style(),
        content: WORDS.into(),
        transform: None,
    })
}

fn boxed() -> gpui_kit::StyleRefinement {
    div().w(px(200.)).h(px(96.)).style().clone()
}

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

fn unfocusable(label: Option<&str>) -> wire::Interactivity {
    wire::Interactivity {
        focusable: false,
        ..every_aria("child", label)
    }
}

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
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(interactivity)),
            children: vec![text("child-text")],
        }),
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
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(interactivity)),
        },
        Site::UniformList => wire::Node::UniformList {
            id: key("child"),
            path: vec![key("child")],
            route: 1,
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(interactivity)),
            count: 1,
            measure_index: 0,
            sizing: wire::list::UniformListSizing::Auto,
            horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
            y_flipped: false,
            scroll_request: None,
            revision: 0,
            indices: vec![0],
            children: vec![wire::Node::Container(view_wire::ContainerNode {
                id: Some(key("child/row:0")),
                style: crate::render::test_style(boxed()),
                interactivity: Default::default(),
                children: vec![text("child-text")],
            })],
        },
        Site::UniformListRow => wire::Node::UniformList {
            id: key("list"),
            path: vec![key("list")],
            route: 1,
            style: crate::render::test_style(boxed()),
            interactivity: Default::default(),
            count: 1,
            measure_index: 0,
            sizing: wire::list::UniformListSizing::Auto,
            horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
            y_flipped: false,
            scroll_request: None,
            revision: 0,
            indices: vec![0],
            children: vec![child(&Site::Container, interactivity)],
        },
        Site::ResizeHandle => wire::Node::ResizeHandle {
            id: key("child"),
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(interactivity)),
            on_press: None,
            on_release: None,
            on_drag: None,
            cursor: None,
            content: Box::new(text("child-text")),
        },
        Site::List => wire::Node::List {
            id: key("rows"),
            path: vec![key("rows")],
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
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(interactivity)),
            children: vec![text("child-text")],
        },
    }
}

fn root(site: &Site, interactivity: wire::Interactivity) -> wire::Node {
    let mut child = child(site, interactivity);
    child.for_each_mut(&mut |node| {
        if let wire::Node::UniformList { path, .. } | wire::Node::List { path, .. } = node {
            path.insert(0, key("parent"));
        }
    });
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("parent")),
        style: crate::render::test_style(div().w(px(280.)).h(px(180.)).style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
            role: Some(Role::Group),
            focusable: true,
            focus_handle: Some(PARENT_FOCUS),
            aria: wire::Aria {
                author_id: Some("parent".into()),
                label: Some("Parent".into()),
                ..Default::default()
            },
            ..Default::default()
        })),
        children: vec![child],
    })
}

fn sanitized(root: wire::Node) -> wire::Node {
    let mut frame = wire::Frame {
        root: Some(root),
        ..Default::default()
    };
    crate::render::sanitize_whole(&mut frame).expect("the host takes the tree");
    frame.root.expect("the tree stays")
}

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

fn announce(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> (Option<Heard>, bool) {
    let root = sanitized(root);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let parent = tree
            .read(cx)
            .view_focus_targets
            .get(&PARENT_FOCUS)
            .cloned()
            .expect("the parent's view focus handle is made on first render");
        parent.focus(window, cx);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
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

#[gpui_kit::test]
fn view_aria_announces_what_each_renderer_did(cx: &mut gpui_kit::TestAppContext) {
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

#[gpui_kit::test]
fn a_labelled_picture_without_a_role_is_an_image_in_the_tree(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for label in [Some("Ada's avatar"), None] {
        let aria = wire::Interactivity {
            aria: wire::Aria {
                author_id: Some("picture".into()),
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
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(aria.clone())),
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
            style: crate::render::test_style(boxed()),
            interactivity: Some(Box::new(aria)),
        };
        for picture in [image, vector] {
            let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(picture));
            let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
            let heard = native.update(|window, cx| {
                window.activate_a11y();
                crate::render::draw_frame(window, cx);
                crate::render::draw_frame(window, cx);
                let update = window.a11y_tree().expect("an a11y tree once activated");
                heard(update, "picture").map(|(_, heard)| (heard.role, heard.name))
            });
            let want = label.map(|label| (Role::Image, Some(label.to_owned())));
            assert_eq!(heard, want);
        }
    }
}

#[gpui_kit::test]
fn a_tracked_handle_the_view_makes_a_tab_stop_takes_tab(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let button = |key_name: &str, focus: u64, tab_stop: Option<bool>| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(key_name)),
            style: crate::render::test_style(div().w(px(80.)).h(px(40.)).style().clone()),
            interactivity: Some(Box::new(wire::Interactivity {
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
            })),
            children: Vec::new(),
        })
    };
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("row")),
        style: crate::render::test_style(div().flex().w(px(280.)).h(px(80.)).style().clone()),
        interactivity: Default::default(),
        children: vec![button("skipped", 1, None), button("stop", 2, Some(true))],
    });
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(sanitized(root))
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        window.focus_next(cx);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let (stop, _) = heard(update, "stop").expect("the stop has a node");
        assert_eq!(update.focus, stop, "Tab reaches the view's tab stop");
    });
}

fn pressable_after_tab(cx: &mut gpui_kit::TestAppContext, tab_stop: Option<bool>) -> (bool, bool) {
    cx.update(gpui_kit::init);
    let button = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("press")),
        style: crate::render::test_style(div().w(px(80.)).h(px(40.)).style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
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
        })),
        children: Vec::new(),
    });
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("row")),
        style: crate::render::test_style(div().flex().w(px(280.)).h(px(80.)).style().clone()),
        interactivity: Default::default(),
        children: vec![button],
    });
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(sanitized(root))
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        window.focus_next(cx);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let (press, heard) = heard(update, "press").expect("the button has a node");
        (heard.focus_action, update.focus == press)
    })
}

#[gpui_kit::test]
fn a_lowered_focusable_with_tab_stop_takes_tab(cx: &mut gpui_kit::TestAppContext) {
    assert_eq!(pressable_after_tab(cx, Some(true)), (true, true));
}

#[gpui_kit::test]
fn a_raw_focusable_without_tab_stop_offers_focus_but_tab_skips_it(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (offers, reached) = pressable_after_tab(cx, None);
    assert!(offers, "the door reads it as offering focus");
    assert!(!reached, "Tab skips a focusable the wire left out");
}

#[gpui_kit::test]
fn a_container_the_view_focuses_by_id_stays_a_tab_stop(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let stop = |key_name: &str, role: Role| {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(key_name)),
            style: crate::render::test_style(div().w(px(80.)).h(px(40.)).style().clone()),
            interactivity: Some(Box::new(wire::Interactivity {
                role: Some(role),
                focusable: true,
                tab_stop: Some(true),
                aria: wire::Aria {
                    author_id: Some(key_name.into()),
                    label: Some(key_name.into()),
                    ..Default::default()
                },
                ..Default::default()
            })),
            children: Vec::new(),
        })
    };
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("row")),
        style: crate::render::test_style(div().flex().w(px(280.)).h(px(80.)).style().clone()),
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
            crate::render::draw_frame(window, cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            ["menu", "next"]
                .into_iter()
                .find(|name| heard(update, name).is_some_and(|(id, _)| id == update.focus))
        })
    };
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        tree.update(cx, |tree, cx| {
            let target = vec![key("row"), key("menu")];
            tree.execute_widget_command(wire::WidgetCommand::Focus { target }, window, cx)
                .unwrap();
        });
    });
    assert_eq!(focused(&mut native), Some("menu"), "the view focused it");
    native.update(|window, cx| window.focus_next(cx));
    assert_eq!(focused(&mut native), Some("next"));
    native.update(|window, cx| window.focus_next(cx));
    assert_eq!(
        focused(&mut native),
        Some("menu"),
        "Tab comes back round to the container it focused"
    );
}

fn claiming_composite(id: &str, role: Role, item: Role) -> wire::Node {
    let rows = (0..3)
        .map(|n| {
            let row = format!("{id}-{n}");
            wire::Node::Container(view_wire::ContainerNode {
                id: Some(key(&row)),
                style: crate::render::test_style(div().w(px(60.)).h(px(20.)).style().clone()),
                interactivity: Some(Box::new(wire::Interactivity {
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
                })),
                children: Vec::new(),
            })
        })
        .collect();
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(key(id)),
        style: crate::render::test_style(div().flex().w(px(200.)).h(px(20.)).style().clone()),
        interactivity: Some(Box::new(wire::Interactivity {
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
        })),
        children: rows,
    })
}

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
        style: crate::render::test_style(
            div()
                .flex()
                .flex_col()
                .w(px(280.))
                .h(px(80.))
                .style()
                .clone(),
        ),
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
            if let wire::Node::Container(view_wire::ContainerNode {
                interactivity: Some(interactivity),
                ..
            }) = node
            {
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
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let focused = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.focus)
            .map(|(_, node)| (node.role(), node.label().map(str::to_owned)));
        assert_eq!(focused, Some((Role::Group, Some("Held window".into()))));
    });
    let claimed = pane.read_with(&native, |pane, cx| pane.view.read(cx).claimed.clone());
    assert_eq!(
        claimed,
        Vec::<AuthoredPath>::new(),
        "no composite holds the keys"
    );
    assert_eq!(kept, 2, "the sanitizer keeps a claim per composite");
}

fn two_composites(around: Option<&str>) -> wire::Node {
    let composites = vec![
        claiming_composite("tabs", Role::TabList, Role::Tab),
        claiming_composite("files", Role::ListBox, Role::ListBoxOption),
    ];
    let interactivity = around.map(|name| {
        Box::new(wire::Interactivity {
            role: Some(Role::Group),
            focusable: true,
            tab_stop: Some(true),
            aria: wire::Aria {
                author_id: Some(name.into()),
                label: Some(name.into()),
                ..Default::default()
            },
            ..Default::default()
        })
    });
    sanitized(wire::Node::Container(view_wire::ContainerNode {
        id: Some(key("screen")),
        style: crate::render::test_style(
            div()
                .flex()
                .flex_col()
                .w(px(280.))
                .h(px(80.))
                .style()
                .clone(),
        ),
        interactivity,
        children: composites,
    }))
}

fn focus_on(
    tree: &Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
    target: Vec<wire::ElementIdWire>,
) {
    native.update(|window, cx| {
        tree.update(cx, |tree, cx| {
            tree.execute_widget_command(wire::WidgetCommand::Focus { target }, window, cx)
                .unwrap();
        });
        crate::render::draw_frame(window, cx);
    });
}

fn claimed(tree: &Entity<ViewTree>, native: &gpui_kit::VisualTestContext) -> Vec<AuthoredPath> {
    tree.read_with(native, |tree, _| tree.claimed.clone())
}

fn row(composite: &str, n: usize) -> AuthoredPath {
    vec![
        key("screen"),
        key(composite),
        key(&format!("{composite}-{n}")),
    ]
}

#[gpui_kit::test]
fn a_claim_counts_only_while_its_own_composite_holds_the_keys(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(two_composites(None))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
    });
    assert_eq!(
        claimed(&tree, &native),
        Vec::<AuthoredPath>::new(),
        "with the keys nowhere, no row claims"
    );
    for composite in ["files", "tabs"] {
        focus_on(&tree, &mut native, vec![key("screen"), key(composite)]);
        assert_eq!(
            claimed(&tree, &native),
            vec![row(composite, 1)],
            "{composite} holds the keys: its row alone claims"
        );
        let focused = native.update(|window, _| {
            let update = window.a11y_tree().expect("an a11y tree once activated");
            heard(update, &format!("{composite}-1")).is_some_and(|(id, _)| id == update.focus)
        });
        assert!(focused, "assistive technology hears {composite}-1 focused");
    }
}

#[gpui_kit::test]
fn a_composite_named_by_a_view_focus_handle_gates_its_rows(cx: &mut gpui_kit::TestAppContext) {
    const FILES: u64 = 5;
    cx.update(gpui_kit::init);
    let mut root = two_composites(None);
    let wire::Node::Container(view_wire::ContainerNode { children, .. }) = &mut root else {
        unreachable!()
    };
    let Some(wire::Node::Container(view_wire::ContainerNode {
        interactivity: Some(files),
        ..
    })) = children.get_mut(1)
    else {
        unreachable!()
    };
    files.focusable = false;
    files.focus_handle = Some(FILES);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    assert_eq!(
        claimed(&tree, &native),
        Vec::<AuthoredPath>::new(),
        "with the keys nowhere, no row claims"
    );
    let files = tree.read_with(&native, |tree, _| tree.view_focus_targets[&FILES].clone());
    native.update(|window, cx| {
        window.focus(&files, cx);
        crate::render::draw_frame(window, cx);
    });
    assert_eq!(claimed(&tree, &native), vec![row("files", 1)]);
}

#[gpui_kit::test]
fn a_composite_focused_again_in_a_new_tree_claims_on_its_first_frame(
    cx: &mut gpui_kit::TestAppContext,
) {
    struct Host(Entity<ViewTree>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, cx| {
        Host(cx.new(|_| ViewTree::new(two_composites(None))))
    });
    let host = window.root(cx).unwrap();
    let tree = host.read_with(cx, |host, _| host.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    focus_on(&tree, &mut native, vec![key("screen"), key("files")]);
    let fresh = native.update(|window, cx| {
        let saved = tree.read_with(cx, |tree, cx| tree.presentation(window, cx));
        let fresh = cx.new(|_| ViewTree::new(two_composites(None)).with_presentation(saved));
        host.update(cx, |host, cx| {
            host.0 = fresh.clone();
            cx.notify();
        });
        crate::render::draw_frame(window, cx);
        fresh
    });
    assert_eq!(
        claimed(&fresh, &native),
        vec![row("files", 1)],
        "the new tree's first frame gives files the keys back and lowers its claim"
    );
}

#[gpui_kit::test]
fn a_focused_box_around_two_composites_lets_neither_claim(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(two_composites(Some("Board")))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
    });
    focus_on(&tree, &mut native, vec![key("screen")]);
    assert_eq!(
        claimed(&tree, &native),
        Vec::<AuthoredPath>::new(),
        "the box holds the keys, not a composite inside it"
    );
    focus_on(&tree, &mut native, vec![key("screen"), key("files")]);
    assert_eq!(claimed(&tree, &native), vec![row("files", 1)]);
}

#[gpui_kit::test]
fn a_cached_view_lowers_its_claim_again_as_the_keys_move_between_composites(
    cx: &mut gpui_kit::TestAppContext,
) {
    struct Cached(Entity<ViewTree>);
    impl Render for Cached {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                gpui_kit::AnyView::from(self.0.clone())
                    .cached(gpui_kit::StyleRefinement::default().size_full()),
            )
        }
    }
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, cx| {
        Cached(cx.new(|_| ViewTree::new(two_composites(None))))
    });
    let tree = window
        .root(cx)
        .unwrap()
        .read_with(cx, |cached, _| cached.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    focus_on(&tree, &mut native, vec![key("screen"), key("files")]);
    native.run_until_parked();
    assert_eq!(claimed(&tree, &native), vec![row("files", 1)]);
    let tabs = tree.read_with(&native, |tree, _| {
        tree.focus_targets[&vec![key("screen"), key("tabs")]]
            .1
            .clone()
    });
    native.update(|window, cx| {
        window.focus(&tabs, cx);
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
    native.update(crate::render::draw_frame);
    assert_eq!(
        claimed(&tree, &native),
        vec![row("tabs", 1)],
        "the keys moved to tabs with no write from the view: the claim moved with them"
    );
}

mod phase_two {
    use super::*;

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

    fn built(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> Option<accesskit::Node> {
        let root = sanitized(root);
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            crate::render::draw_frame(window, cx);
            let update = window.a11y_tree().expect("an a11y tree once activated");
            update
                .nodes
                .iter()
                .find(|(_, node)| node.author_id() == Some("child"))
                .map(|(_, node)| node.clone())
        })
    }

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
        native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            window.focus_next(cx);
        });
        native.simulate_keystrokes("left");
        native.update(|window, cx| {
            crate::render::draw_frame(window, cx);
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
                style: crate::render::test_style(
                    div().w(px(width)).h_full().flex_shrink_0().style().clone(),
                ),
                interactivity: Some(Box::new(named(Role::Group, "List"))),
                children: Vec::new(),
            });
            let divider = wire::Node::ResizeHandle {
                id: key("list-resize"),
                style: crate::render::test_style(div().w(px(1.)).h_full().style().clone()),
                interactivity: Some(Box::new(wire::Interactivity {
                    focusable: true,
                    tab_stop: Some(true),
                    on_key_down: Some(ROUTE),
                    aria: wire::Aria {
                        orientation: Some(accesskit::Orientation::Vertical),
                        ..named(Role::Splitter, "Resize the list").aria
                    },
                    ..named(Role::Splitter, "Resize the list")
                })),
                on_press: None,
                on_release: None,
                on_drag: None,
                cursor: None,
                content: Box::new(wire::Node::Container(view_wire::ContainerNode {
                    id: Some(key("list-rule")),
                    style: crate::render::test_style(div().w(px(1.)).h_full().style().clone()),
                    interactivity: Default::default(),
                    children: Vec::new(),
                })),
            };
            sanitized(wire::Node::Container(view_wire::ContainerNode {
                id: Some(key("panes")),
                style: crate::render::test_style(
                    div().flex().w(px(600.)).h(px(200.)).style().clone(),
                ),
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
                crate::render::draw_frame(window, cx);
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
        native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            window.focus_next(cx);
        });
        let mut width = 200.;
        assert_eq!(wide(&mut native), (200, Some("Splitter".into())));
        for (keystroke, moved) in [("right", 208), ("shift-right", 240), ("left", 232)] {
            native.simulate_keystrokes(keystroke);
            for event in events.borrow_mut().drain(..) {
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

    #[gpui_kit::test]
    fn a_view_focus_handle_on_a_divider_or_a_list_survives_a_frame(
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
                crate::render::draw_frame(window, cx);
                let handle = tree.read(cx).view_focus_targets[&PARENT_FOCUS].clone();
                tree.update(cx, |tree, cx| tree.replace(again, &[], cx));
                crate::render::draw_frame(window, cx);
                assert!(
                    tree.read(cx).view_focus_targets[&PARENT_FOCUS] == handle,
                    "{site:?} made its handle anew"
                );
            });
        }
    }

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
            style: crate::render::test_style(div().h(px(20.)).style().clone()),
            interactivity: Some(Box::new(named(format!("inner-{n}")))),
            children: Vec::new(),
        });
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(key(&format!("row-{n}"))),
            style: crate::render::test_style(div().h(px(20.)).style().clone()),
            interactivity: Some(Box::new(wire::Interactivity {
                role,
                ..named(format!("row-{n}"))
            })),
            children: vec![inner],
        })
    }

    fn set_places(
        cx: &mut gpui_kit::TestAppContext,
        root: wire::Node,
    ) -> Vec<(String, Option<usize>, Option<usize>)> {
        let root = sanitized(root);
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            crate::render::draw_frame(window, cx);
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

    #[gpui_kit::test]
    fn a_list_row_says_where_it_is_in_the_list(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let mut told = row(2, Some(Role::ListBoxOption));
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut told {
            let interactivity = interactivity.get_or_insert_default();
            interactivity.aria.position_in_set = Some(9);
        }
        let uniform = wire::Node::UniformList {
            id: key("list"),
            path: vec![key("list")],
            route: 1,
            style: crate::render::test_style(boxed()),
            interactivity: Default::default(),
            count: 5,
            measure_index: 0,
            sizing: wire::list::UniformListSizing::Auto,
            horizontal_sizing: wire::list::UniformListHorizontalSizing::FitList,
            y_flipped: false,
            scroll_request: None,
            revision: 0,
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
            id: key("rows"),
            path: vec![key("rows")],
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
            style: crate::render::test_style(boxed()),
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
