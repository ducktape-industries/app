use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, Keystroke};
use std::{cell::RefCell, rc::Rc};

pub(super) fn named_id(key: &str) -> wire::ElementIdWire {
    wire::ElementIdWire::Name(key.into())
}

fn fill() -> gpui_kit::Length {
    gpui_kit::Length::Definite(gpui_kit::DefiniteLength::Fraction(1.0))
}

fn fixed(value: f32) -> gpui_kit::Length {
    gpui_kit::Length::Definite(gpui_kit::DefiniteLength::Absolute(
        gpui_kit::AbsoluteLength::Pixels(px(value)),
    ))
}

fn sized_style(
    width: Option<gpui_kit::Length>,
    height: Option<gpui_kit::Length>,
) -> gpui_kit::StyleRefinement {
    let mut style = gpui_kit::StyleRefinement::default();
    style.size.width = width;
    style.size.height = height;
    if let Some(width) = width {
        style.min_size.width = Some(match width {
            gpui_kit::Length::Definite(gpui_kit::DefiniteLength::Fraction(_)) => px(0.).into(),
            width => width,
        });
    }
    if let Some(height) = height {
        style.min_size.height = Some(match height {
            gpui_kit::Length::Definite(gpui_kit::DefiniteLength::Fraction(_)) => px(0.).into(),
            height => height,
        });
    }
    style
}

fn container(key: &str, children: impl IntoIterator<Item = wire::Node>) -> wire::Node {
    container_with_style(key, gpui_kit::StyleRefinement::default(), children)
}

fn container_with_style(
    key: &str,
    style: gpui_kit::StyleRefinement,
    children: impl IntoIterator<Item = wire::Node>,
) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style,
        interactivity: Default::default(),
        children: children.into_iter().collect(),
    })
}

fn sized(
    key: &str,
    child: wire::Node,
    width: Option<gpui_kit::Length>,
    height: Option<gpui_kit::Length>,
) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: sized_style(width, height),
        interactivity: Default::default(),
        children: vec![child],
    })
}

/// Which way a test's flex container or rule runs.
#[derive(Clone, Copy)]
enum Axis {
    Column,
    Row,
}

/// A one-pixel line across `axis`: a sized, filled container with nothing in it.
fn rule(key: &str, axis: Axis) -> wire::Node {
    let mut element = div().bg(gpui_kit::Rgba {
        r: 0.4,
        g: 0.4,
        b: 0.4,
        a: 1.0,
    });
    element = match axis {
        Axis::Row => element.h(px(1.)),
        Axis::Column => element.w(px(1.)),
    };
    container_with_style(key, element.style().clone(), [])
}

fn text(key: &str, content: impl Into<String>) -> wire::Node {
    wire::Node::Text(view_wire::TextNode {
        id: Some(named_id(key)),
        style: gpui_kit::StyleRefinement::default(),
        content: content.into(),
    })
}

fn axis_container(
    key: &str,
    axis: Axis,
    children: impl IntoIterator<Item = wire::Node>,
) -> wire::Node {
    let mut element = div().flex().w_full().min_w_0().gap(px(8.));
    element = match axis {
        Axis::Column => element.flex_col(),
        Axis::Row => element.flex_row(),
    };
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: element.style().clone(),
        interactivity: Default::default(),
        children: children.into_iter().collect(),
    })
}

fn input(label: &str, secure: bool, disabled: bool) -> wire::Node {
    wire::Node::Input {
        options: wire::InputOptions {
            label: label.into(),
            description: Some("Shown to members".into()),
            disabled,
            ..Default::default()
        },
        id: wire::ElementIdWire::Name("i".into()),
        placeholder: "Type here".into(),
        value: "hunter2".into(),
        on_input: Some(1),
        on_submit: None,
        secure,
        style: Default::default(),
    }
}

fn picture(label: Option<&str>) -> [wire::Node; 2] {
    let label = label.map(str::to_owned);
    [
        wire::Node::Image {
            id: Some(wire::ElementIdWire::Name("img".into())),
            hash: 1,
            data: None,
            label: label.clone(),
            image_style: wire::ImageStyle {
                grayscale: false,
                object_fit: wire::ImageObjectFit::Contain,
            },
            loading: false,
            fallback: false,
            state_children: vec![],
            style: Default::default(),
            interactivity: Default::default(),
        },
        wire::Node::Svg {
            id: Some(wire::ElementIdWire::Name("svg".into())),
            source: wire::SvgSource::Data {
                hash: 1,
                bytes: None,
            },
            transformation: wire::SvgTransformation {
                scale: [1., 1.],
                translate: [0., 0.],
                rotate: 0.,
            },
            label,
            style: Default::default(),
            interactivity: Default::default(),
        },
    ]
}

/// A focusable button Tab reaches.
fn button(key: &str, name: &str) -> wire::Node {
    let mut node = container_with_style(key, div().w(px(80.)).h(px(24.)).style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.aria.label = Some(name.into());
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
    }
    node
}

/// "Read the docs or the code", its two links ("the docs", "the code")
/// pressing handler 72.
fn rich() -> wire::Node {
    wire::Node::RichText {
        id: Some(named_id("rich")),
        style: Default::default(),
        text: "Read the docs or the code".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: vec![5..13, 17..25],
        on_click: Some(72),
        on_hover: None,
        tooltip: None,
    }
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

/// `root` in a window, its editors on a store of their own; with `text`,
/// the document they ask for holds it.
fn with_editors(
    root: wire::Node,
    text: Option<&str>,
    cx: &mut gpui_kit::TestAppContext,
) -> (Entity<ViewTree>, gpui_kit::VisualTestContext) {
    let store = crate::editor::wire::EditorStore::new(91);
    store.replace(&root).unwrap();
    if let Some(text) = text {
        seed_editor_text(&store, text);
    }
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        let mut tree = ViewTree::new(root);
        tree.set_editor_store(store, cx);
        tree
    });
    let tree = window.root(cx).unwrap();
    (
        tree,
        gpui_kit::VisualTestContext::from_window(window.into(), cx),
    )
}

/// A view mounted the way a module seat mounts a guest: a cached view under
/// a full-size div, not as the window root (which gpui stretches).
pub(super) struct Seat(pub(super) Entity<ViewTree>);

impl Render for Seat {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.0
                .clone()
                .cached(gpui_kit::StyleRefinement::default().size_full()),
        )
    }
}

/// Every event `tree` emits while the subscription lives.
pub(super) fn emitted(
    tree: &Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
) -> (Rc<RefCell<Vec<wire::Event>>>, Subscription) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    let subscription = native.update(|_, cx| {
        cx.subscribe(tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone());
        })
    });
    (events, subscription)
}

use crate::editor::wire::seed_editor_text;

mod accessibility;
mod dialog_focus;
mod gpui_activation;
mod gpui_clip;
mod grip;
mod host_ids;
mod inputs;
mod layout;
mod links;
mod picture_presentation;
mod primitives;
mod rich_tooltip;
mod selection;
mod uniform_list;
mod variable_list;
