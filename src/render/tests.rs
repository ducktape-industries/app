use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, Keystroke};
use std::{cell::RefCell, rc::Rc};

/// Resident memory, for the render measurements (`--ignored`).
pub(super) fn resident_mib() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
    let kib: f64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib / 1024.
}

thread_local! {
    /// The style table of the test on this thread: every style one of its
    /// nodes names, as the guest numbers them and the host holds them.
    static STYLES: RefCell<(wire::Interner, wire::Styles)> = Default::default();
}

/// The id `style` has in this test's table, which holds it from here on.
pub(crate) fn test_style(
    style: impl std::borrow::Borrow<gpui_kit::StyleRefinement>,
) -> wire::StyleId {
    STYLES.with_borrow_mut(|(guest, host)| {
        let id = guest.intern(style.borrow());
        host.extend(guest.unsent()).expect("a style the host takes");
        id
    })
}

/// The id of the style that sets nothing.
pub(crate) fn plain_style() -> wire::StyleId {
    test_style(gpui_kit::StyleRefinement::default())
}

/// This test's table, as far as it has named styles.
pub(crate) fn test_styles() -> wire::Styles {
    STYLES.with_borrow(|(_, host)| host.clone())
}

/// `frame` as a guest sends it: a whole one carries this test's table.
pub(crate) fn sent(mut frame: wire::Frame) -> wire::Frame {
    if frame.root.is_some() {
        let held = test_styles();
        frame.styles = (0..held.len() as u32)
            .map(|id| wire::Style::new(&held[wire::StyleId(id)]))
            .collect();
    }
    frame
}

/// A whole `frame` through the sanitizer, as the host takes one a guest
/// sent.
pub(crate) fn sanitize_whole(
    frame: &mut wire::Frame,
) -> Result<wire::SanitizeReport, wire::Refused> {
    *frame = sent(std::mem::take(frame));
    wire::sanitize(frame, &mut wire::Styles::default())
}

/// Tooltip answers as a seat hands them to its tree: each with the table
/// of the frame it came in, here this test's.
pub(crate) fn answered(
    responses: impl IntoIterator<Item = wire::TooltipResponse>,
) -> Vec<(wire::TooltipResponse, wire::Styles)> {
    // the answers first: building one may name a style
    let responses: Vec<_> = responses.into_iter().collect();
    let styles = test_styles();
    responses
        .into_iter()
        .map(|response| (response, styles.clone()))
        .collect()
}

/// A test's tree is drawn with the test's table.
impl From<wire::Node> for Tree {
    fn from(root: wire::Node) -> Self {
        Self {
            root,
            styles: test_styles(),
        }
    }
}

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
        style: crate::render::test_style(style),
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
        style: crate::render::test_style(sized_style(width, height)),
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
        style: crate::render::test_style(gpui_kit::StyleRefinement::default()),
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
        style: crate::render::test_style(element.style().clone()),
        interactivity: Default::default(),
        children: children.into_iter().collect(),
    })
}

/// A one-line field holding "hunter2", its caret at the end, as the guest's
/// `TextField::new` leaves one.
fn input(label: &str, secure: bool, disabled: bool) -> wire::Node {
    wire::Node::Field {
        options: Box::new(wire::InputOptions {
            label: label.into(),
            description: Some("Shown to members".into()),
            disabled,
            ..Default::default()
        }),
        id: wire::ElementIdWire::Name("i".into()),
        multiline: false,
        value: "hunter2".into(),
        cursor: wire::TextRange::caret("hunter2".len()),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        placeholder: "Type here".into(),
        secure,
        on_change: Some(1),
        on_key: None,
        on_submit: None,
        style: crate::render::plain_style(),
    }
}

/// A growing field holding `value`, its caret at the end: what a composer
/// or a comment box mounts. It hears changes on 1 and claimed keys on 2.
fn area(
    key: &str,
    label: Option<&str>,
    value: &str,
    style: gpui_kit::StyleRefinement,
) -> wire::Node {
    wire::Node::Field {
        id: named_id(key),
        multiline: true,
        value: value.into(),
        cursor: wire::TextRange::caret(value.len()),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        options: Box::new(wire::InputOptions {
            label: label.map(str::to_owned).unwrap_or_default(),
            ..Default::default()
        }),
        placeholder: String::new(),
        secure: false,
        on_change: Some(1),
        on_key: Some(2),
        on_submit: None,
        style: crate::render::test_style(style),
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
            style: crate::render::plain_style(),
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
            style: crate::render::plain_style(),
            interactivity: Default::default(),
        },
    ]
}

/// A focusable button Tab reaches.
fn button(key: &str, name: &str) -> wire::Node {
    let mut node = container_with_style(key, div().w(px(80.)).h(px(24.)).style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        let interactivity = interactivity.get_or_insert_default();
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
        style: crate::render::plain_style(),
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

mod accessibility;
mod dialog_focus;
mod element_cost;
#[cfg(target_os = "linux")]
mod faces;
mod gpui_activation;
mod gpui_clip;
mod grip;
mod host_ids;
mod inputs;
mod layout;
mod links;
#[cfg(target_os = "linux")]
pub(super) mod picture_pixels;
mod picture_presentation;
mod primitives;
mod rich_tooltip;
mod selection;
mod uniform_list;
mod variable_list;
