use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, Keystroke};

fn named_id(key: &str) -> wire::ElementIdWire {
    wire::ElementIdWire::Name(key.into())
}

fn sized_style(
    width: Option<wire::Length>,
    height: Option<wire::Length>,
) -> gpui_kit::StyleRefinement {
    dimensions(div(), width, height).style().clone()
}

fn container(key: &str, children: impl IntoIterator<Item = wire::Node>) -> wire::Node {
    container_with_style(key, gpui_kit::StyleRefinement::default(), children)
}

fn container_with_style(
    key: &str,
    style: gpui_kit::StyleRefinement,
    children: impl IntoIterator<Item = wire::Node>,
) -> wire::Node {
    wire::Node::Container {
        id: Some(named_id(key)),
        style,
        interactivity: Default::default(),
        children: children.into_iter().collect(),
    }
}

fn sized(
    key: &str,
    child: wire::Node,
    width: Option<wire::Length>,
    height: Option<wire::Length>,
) -> wire::Node {
    wire::Node::Container {
        id: Some(named_id(key)),
        style: sized_style(width, height),
        interactivity: Default::default(),
        children: vec![child],
    }
}

fn text(key: &str, content: impl Into<String>) -> wire::Node {
    wire::Node::Text {
        id: Some(named_id(key)),
        style: gpui_kit::StyleRefinement::default(),
        content: content.into(),
        heading: None,
        live: None,
    }
}

fn axis_container(
    key: &str,
    axis: wire::Axis,
    children: impl IntoIterator<Item = wire::Node>,
) -> wire::Node {
    let mut element = div().flex().w_full().min_w_0().gap(px(8.));
    element = match axis {
        wire::Axis::Column => element.flex_col(),
        wire::Axis::Row => element.flex_row(),
    };
    wire::Node::Container {
        id: Some(named_id(key)),
        style: element.style().clone(),
        interactivity: Default::default(),
        children: children.into_iter().collect(),
    }
}

include!("tests/part_1.rs");
include!("tests/part_2.rs");
include!("tests/part_3.rs");
include!("tests/part_4.rs");
include!("tests/gpui_clip.rs");

include!("tests/gpui_activation.rs");
include!("tests/part_5.rs");
include!("tests/variable_list.rs");

include!("tests/picture_presentation.rs");
