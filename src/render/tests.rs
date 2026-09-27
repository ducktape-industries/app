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
        heading: None,
        live: None,
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

/// Seeds `text` into `store` by answering the document request the store
/// emits for a document it has no text for, with the Begin/Chunk/Complete
/// transfer a guest sends.
fn seed_editor_text(store: &crate::editor::wire::EditorStore, text: &str) {
    use wire::editor_document::{EditorDocumentMessage as Message, EditorTransfer};
    let asked = store.drain().into_iter().find_map(|event| match event {
        wire::Event::EditorDocument {
            message: Message::Request { id, target },
            ..
        } => Some((id, target)),
        _ => None,
    });
    let (id, target) = asked.expect("the store asks for a document it has no text for");
    store
        .frame(&wire::Frame {
            editor_documents: vec![
                Message::Transfer(EditorTransfer::Begin {
                    id: id.clone(),
                    target,
                }),
                Message::Transfer(EditorTransfer::Chunk {
                    id: id.clone(),
                    index: 0,
                    bytes: text.as_bytes().to_vec(),
                }),
                Message::Transfer(EditorTransfer::Complete { id }),
            ],
            ..Default::default()
        })
        .expect("the answer to the store's own request");
}

mod accessibility;
mod gpui_activation;
mod gpui_clip;
mod grip;
mod inputs;
mod layout;
mod picture_presentation;
mod primitives;
mod rich_tooltip;
mod selection;
mod uniform_list;
mod variable_list;
