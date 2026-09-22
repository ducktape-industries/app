use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, Keystroke};

fn named_id(key: &str) -> wire::ElementIdWire {
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
    style.size.width = width.clone();
    style.size.height = height.clone();
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
    width: Option<gpui_kit::Length>,
    height: Option<gpui_kit::Length>,
) -> wire::Node {
    wire::Node::Container {
        id: Some(named_id(key)),
        style: sized_style(width, height),
        interactivity: Default::default(),
        children: vec![child],
    }
}

fn rule(key: &str, axis: wire::Axis) -> wire::Node {
    let mut element = div().bg(gpui_kit::Rgba {
        r: 0.4,
        g: 0.4,
        b: 0.4,
        a: 1.0,
    });
    element = match axis {
        wire::Axis::Row => element.h(px(1.)),
        wire::Axis::Column => element.w(px(1.)),
    };
    wire::Node::Rule {
        id: named_id(key),
        axis,
        style: element.style().clone(),
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
