//! GPUI rendering of the existing WASM tree. Widget identities retain native
//! input state; interaction uses the same semantic events the guests consume.

use gpui_kit::MouseUpEvent;
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{
    Disableable, Selectable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputContentType, InputEvent, InputState},
};
use gpui_kit::component::{
    IndexPath,
    searchable_list::{SearchableListDelegate, SearchableListItem},
    select::{SearchableVec, Select, SelectEvent, SelectState},
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, BoxShadow, Context, CursorStyle, Div, Element,
    ElementId, Entity, EntityInputHandler as _, EventEmitter, FocusHandle, Focusable as _,
    FollowMode, FontWeight, GlobalElementId, HighlightStyle, HitboxBehavior, Hsla, Image,
    ImageFormat, InspectorElementId, InteractiveElement as _, IntoElement, KeyDownEvent, LayoutId,
    ListAlignment, ListSizingBehavior, ListState, MouseButton, MouseDownEvent, MouseMoveEvent,
    ObjectFit, ParentElement as _, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Size, Stateful, StatefulInteractiveElement as _,
    StrikethroughStyle, Styled, StyledImage as _, StyledText, Subscription, Task, TextLayout,
    UnderlineStyle, Window, canvas, div, fill, img, point, px, relative, rgb, size, svg,
};
use std::collections::HashMap;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;
use view_wire as wire;

mod accessibility;
mod canvas;
mod commands;
mod deferred;
mod inputs;
mod layout;
mod pickers;
mod pictures;
mod scroll;
mod sensors;
mod style;
mod surfaces;
mod text;

#[cfg(test)]
mod tests;

pub(crate) use accessibility::{Accessible, accessible, announce};
#[cfg(test)]
use canvas::{append_arc, append_arc_to};
use canvas::{canvas_svg, native_canvas_commands, paint_canvas_commands};
pub(crate) use commands::dialog_entry;
use inputs::{EditorMount, Field, RangeControl};
use pickers::Picker;
#[cfg(test)]
use pictures::decode_image;
use pictures::{ViewerState, qr};
use scroll::{ScrollRequest, VirtualScroll};
use sensors::SensorState;
use style::{
    button_style, content_dimensions, cross_align, decoration, dimensions, font_weight,
    has_named_overlay, horizontal_align, named_overlay, native_cursor, object_fit, pad, rgba,
    shadows, text_options,
};
use text::RichSelection;

#[derive(Default)]
pub(crate) struct NativePresentation {
    focused_container: Option<(String, std::mem::Discriminant<wire::Node>)>,
    inputs: HashMap<String, InputPresentation>,
    editors: HashMap<String, wire::editor_document::EditorDocumentRef>,
    scrolls: HashMap<String, ScrollPresentation>,
}

struct ScrollPresentation {
    direction: wire::ScrollDirection,
    anchors: (wire::ScrollAnchor, wire::ScrollAnchor),
    offset: Point<Pixels>,
    rows: Option<Vec<String>>,
}

struct InputPresentation {
    value: String,
    secure: bool,
    selection: std::ops::Range<usize>,
    focused: bool,
}

pub struct ViewTree {
    user_activation: std::cell::Cell<Option<u32>>,
    root: wire::Node,
    // Structural nodes enter the native focus path only on an explicit Focus request.
    focus_targets: HashMap<String, (std::mem::Discriminant<wire::Node>, FocusHandle)>,
    fields: HashMap<String, Field>,
    rich_selections: HashMap<String, RichSelection>,
    scrolls: HashMap<String, ScrollHandle>,
    lists: HashMap<String, VirtualScroll>,
    scroll_positions: HashMap<String, (Point<Pixels>, Point<Pixels>)>,
    pickers: HashMap<String, Picker>,
    drags: HashMap<String, Point<Pixels>>,
    /// The overlays showing a dialog, and where focus enters each.
    dialogs: HashMap<String, FocusHandle>,
    containers: HashMap<String, [f64; 2]>,
    bounds: HashMap<String, Bounds<Pixels>>,
    sensors: HashMap<String, SensorState>,
    hovered: std::collections::HashSet<String>,
    ranges: HashMap<String, RangeControl>,
    images: HashMap<u64, Arc<RenderImage>>,
    viewers: HashMap<String, ViewerState>,
    vectors: HashMap<u64, Arc<[u8]>>,
    editor_store: Option<crate::editor::wire::EditorStore>,
    editors: HashMap<String, EditorMount>,
    mounted: std::collections::HashSet<String>,
    presentation: NativePresentation,
    render_index: u64,
    #[cfg(test)]
    renders: u64,
}

impl EventEmitter<wire::Event> for ViewTree {}

impl ViewTree {
    pub fn new(root: wire::Node) -> Self {
        Self {
            user_activation: Default::default(),
            root,
            focus_targets: HashMap::new(),
            fields: HashMap::new(),
            rich_selections: HashMap::new(),
            scrolls: HashMap::new(),
            lists: HashMap::new(),
            scroll_positions: HashMap::new(),
            pickers: HashMap::new(),
            drags: HashMap::new(),
            dialogs: HashMap::new(),
            containers: HashMap::new(),
            bounds: HashMap::new(),
            sensors: HashMap::new(),
            hovered: Default::default(),
            ranges: HashMap::new(),
            images: HashMap::new(),
            viewers: HashMap::new(),
            vectors: HashMap::new(),
            editor_store: None,
            editors: HashMap::new(),
            mounted: Default::default(),
            presentation: NativePresentation::default(),
            render_index: 0,
            #[cfg(test)]
            renders: 0,
        }
    }

    fn node(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(key) = node.key() {
            self.mounted.insert(key.to_owned());
        }
        // One arm, one method: this dispatcher is on the recursion chain for
        // every nesting level the wire allows (`wire::MAX_DEPTH`), and an
        // unoptimised build gives a function the stack of ALL its arms at
        // once — inlined bodies here once cost 570 KiB a level and overflowed
        // the main thread at a depth of fourteen.
        use wire::Node;
        match node {
            Node::Text { .. } => self.text(node, cx),
            Node::Space { width, height } => dimensions(div(), *width, *height).into_any_element(),
            Node::Linear { .. } => self.linear(node, window, cx),
            Node::KeyedColumn { .. } => self.keyed_column(node, window, cx),
            Node::Container { .. } => self.container(node, window, cx),
            Node::Scroll { .. } => self.scroll(node, window, cx),
            Node::Button { .. } => self.button(node, window, cx),
            Node::Input { .. } => self.input(node, window, cx),
            Node::PickList { .. } | Node::ComboBox { .. } => self.picker(node, window, cx),
            Node::Toggle { .. } => self.toggle(node, cx),
            Node::Radio {
                key,
                label,
                selected,
                on_select,
                ..
            } => {
                let message = *on_select;
                let radio = Radio::new(key.clone())
                    .label(label.clone())
                    .checked(*selected)
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(wire::Event::Message(message))),
                    );
                announce(radio, accessible(node)).into_any_element()
            }
            Node::Rule {
                axis,
                thickness,
                color,
                ..
            } => {
                let element = div().bg(color.map(rgba).unwrap_or_else(|| {
                    gpui_kit::component::Theme::global(cx).color_tokens().border
                }));
                match axis {
                    wire::Axis::Column => element.w(px(*thickness)).h_full().into_any_element(),
                    wire::Axis::Row => element.h(px(*thickness)).w_full().into_any_element(),
                }
            }
            Node::Lazy { content, .. } => self.node(content, window, cx),
            Node::ResizeHandle { .. } => self.resize_handle(node, window, cx),
            Node::Responsive { .. } => self.responsive(node, window, cx),
            Node::When { .. } => self.when(node, window, cx),
            Node::Sensor { .. } => self.sensor(node, window, cx),
            Node::MouseArea { .. } => self.mouse_area(node, window, cx),
            Node::Slider { .. } => self.slider(node, window, cx),
            Node::RichText { .. } => self.rich_text(node, window, cx),
            Node::Grid { .. } => self.grid(node, window, cx),
            Node::Hover { .. } => self.hover(node, window, cx),
            Node::Tooltip { .. } => self.tooltip(node, window, cx),
            Node::Float { .. } => self.float(node, window, cx),
            Node::Image { .. } => self.picture(node, cx),
            Node::ImageViewer { .. } => self.image_viewer(node, window, cx),
            Node::Svg { .. } => self.vector(node, window),
            Node::Canvas { .. } => self.drawing(node, cx),
            Node::Qr { key, code } => {
                announce(div().id(key.clone()).child(qr(code)), accessible(node)).into_any_element()
            }
            // the host registers no surface: the slot says so where it would be
            Node::Surface { name, .. } => div()
                .child(format!("Unavailable host surface: {name}"))
                .into_any_element(),
            Node::Stack { .. } => self.stack(node, window, cx),
            Node::Overlay { .. } => self.overlay(node, window, cx),
            Node::Progress { .. } => self.progress(node, cx),
            Node::Pin { .. } => self.pin(node, window, cx),
            Node::Editor { .. } => self.editor(node, window, cx),
        }
    }
}

impl Render for ViewTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        self.mounted.clear();
        self.render_index = 0;
        let node = self.node(&self.root.clone(), window, cx);
        // Only controls mounted by this replacement frame may recover focus.
        self.presentation = NativePresentation::default();
        // This boundary belongs to the host, never to guest style. It also
        // supplies the mask captured by deferred guest draws.
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(node)
    }
}
