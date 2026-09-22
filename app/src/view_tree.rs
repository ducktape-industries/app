//! GPUI rendering of the existing WASM tree. Widget identities retain native
//! input state; interaction uses the same semantic events the guests consume.

use gpui_base::StyledExt as _;
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
    FollowMode, FontWeight, GlobalElementId, HitboxBehavior, Hsla, Image, ImageFormat,
    InspectorElementId, InteractiveElement as _, InteractiveText, IntoElement, KeyDownEvent,
    LayoutId, ListAlignment, ListSizingBehavior, ListState, MouseButton, MouseDownEvent,
    MouseMoveEvent, ObjectFit, ParentElement as _, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, SharedString, Size, Stateful, StatefulInteractiveElement as _,
    Styled, StyledImage as _, StyledText, Subscription, Task, TextLayout, Transformation, Window,
    canvas, div, fill, img, point, px, radians, relative, rgb, size, svg,
};
use std::collections::HashMap;
use std::sync::Arc;
use view_wire as wire;

mod accessibility;
mod canvas;
mod commands;
mod deferred;
mod inputs;
mod interactivity;
mod layout;
mod pickers;
mod picture_resources;
mod pictures;
mod scroll;
mod sensors;
mod style;
mod surfaces;
mod svg_limits;
mod text;
mod tooltip_containment;
mod uniform;
mod variable_list;

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
use picture_resources::decode_image;
use pictures::{ViewerState, qr};
use scroll::{ScrollRequest, VirtualScroll};
use sensors::SensorState;
use style::{
    button_style, content_dimensions, cross_align, decoration, dimensions, has_named_overlay,
    named_overlay, native_cursor, object_fit, pad, rgba, shadows,
};
use svg_limits::{guarded_svg_paint, svg_data_allowed};
use uniform::UniformListHostState;
use variable_list::{VariableList, VariableListKey};

pub(crate) type AuthoredPath = Vec<wire::ElementIdWire>;

#[derive(Default)]
pub(crate) struct NativePresentation {
    images: HashMap<u64, Arc<RenderImage>>,
    vectors: HashMap<u64, Arc<[u8]>>,
    focused_container: Option<(AuthoredPath, std::mem::Discriminant<wire::Node>)>,
    inputs: HashMap<AuthoredPath, InputPresentation>,
    editors: HashMap<AuthoredPath, wire::editor_document::EditorDocumentRef>,
    scrolls: HashMap<AuthoredPath, ScrollPresentation>,
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
    slot_mask: tooltip_containment::SlotMask,
    root: wire::Node,
    // Structural nodes enter the native focus path only on an explicit Focus request.
    focus_targets: HashMap<AuthoredPath, (std::mem::Discriminant<wire::Node>, FocusHandle)>,
    fields: HashMap<AuthoredPath, Field>,
    authored_path: AuthoredPath,
    scrolls: HashMap<AuthoredPath, ScrollHandle>,
    lists: HashMap<AuthoredPath, VirtualScroll>,
    scroll_positions: HashMap<AuthoredPath, (Point<Pixels>, Point<Pixels>)>,
    pickers: HashMap<AuthoredPath, Picker>,
    drags: HashMap<AuthoredPath, Point<Pixels>>,
    /// The overlays showing a dialog, and where focus enters each.
    dialogs: HashMap<AuthoredPath, FocusHandle>,
    containers: HashMap<AuthoredPath, [f64; 2]>,
    bounds: HashMap<AuthoredPath, Bounds<Pixels>>,
    sensors: HashMap<AuthoredPath, SensorState>,
    ranges: HashMap<AuthoredPath, RangeControl>,
    guest_focus_targets: HashMap<u64, FocusHandle>,
    uniform_lists: HashMap<AuthoredPath, UniformListHostState>,
    variable_lists: HashMap<VariableListKey, VariableList>,
    images: HashMap<u64, Arc<RenderImage>>,
    viewers: HashMap<AuthoredPath, ViewerState>,
    vectors: HashMap<u64, Arc<[u8]>>,
    editor_store: Option<crate::editor::wire::EditorStore>,
    editors: HashMap<AuthoredPath, EditorMount>,
    mounted: std::collections::HashSet<AuthoredPath>,
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
            slot_mask: Default::default(),
            root,
            focus_targets: HashMap::new(),
            guest_focus_targets: HashMap::new(),
            fields: HashMap::new(),
            authored_path: Vec::new(),
            scrolls: HashMap::new(),
            lists: HashMap::new(),
            uniform_lists: HashMap::new(),
            variable_lists: HashMap::new(),
            scroll_positions: HashMap::new(),
            pickers: HashMap::new(),
            drags: HashMap::new(),
            dialogs: HashMap::new(),
            containers: HashMap::new(),
            bounds: HashMap::new(),
            sensors: HashMap::new(),
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
        // One arm, one method: this dispatcher is on the recursion chain for
        // every nesting level the wire allows (`wire::MAX_DEPTH`), and an
        // unoptimised build gives a function the stack of ALL its arms at
        // once — inlined bodies here once cost 570 KiB a level and overflowed
        // the main thread at a depth of fourteen.
        let entered_scope = match node.identity() {
            Some(wire::IdentityKeyRef::Element(id)) => {
                self.authored_path.push(id.clone());
                true
            }
            _ => false,
        };
        if entered_scope {
            self.mounted.insert(self.authored_path.clone());
        }
        use wire::Node;
        let element = match node {
            Node::Text { .. } => self.text(node, cx),
            Node::Space { width, height } => dimensions(div(), *width, *height).into_any_element(),
            Node::UniformList { .. } => self.uniform_list(node, window, cx),
            Node::List { .. } => self.variable_list(node, cx),
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
            Node::Deferred { .. } => self.deferred(node, window, cx),
            Node::ResizeHandle { .. } => self.resize_handle(node, window, cx),
            Node::Responsive { .. } => self.responsive(node, window, cx),
            Node::When { .. } => self.when(node, window, cx),
            Node::Sensor { .. } => self.sensor(node, window, cx),
            Node::MouseArea { .. } => self.mouse_area(node, window, cx),
            Node::Slider { .. } => self.slider(node, window, cx),
            Node::RichText { .. } => self.rich_text(node, window, cx),
            Node::Tooltip { .. } => self.tooltip(node, window, cx),
            Node::Float { .. } => self.float(node, window, cx),
            Node::Image { .. } => self.picture(node, window, cx),
            Node::ImageViewer { .. } => self.image_viewer(node, window, cx),
            Node::Svg { .. } => self.vector(node, window, cx),
            Node::Canvas { .. } => self.drawing(node, cx),
            Node::Qr { key, code } => {
                announce(div().id(key.clone()).child(qr(code)), accessible(node)).into_any_element()
            }
            // the host registers no surface: the slot says so where it would be
            Node::Surface { id, name, .. } => div()
                .id(id.to_gpui().expect("sanitized surface identity"))
                .child(format!("Unavailable host surface: {name}"))
                .into_any_element(),
            Node::Overlay { .. } => self.overlay(node, window, cx),
            Node::Progress { .. } => self.progress(node, cx),
            Node::Anchored { .. } => self.anchored(node, window, cx),
            Node::Editor { .. } => self.editor(node, window, cx),
        };
        if entered_scope {
            self.authored_path.pop();
        }
        element
    }
}

impl Render for ViewTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        self.mounted.clear();
        self.authored_path.clear();
        self.render_index = 0;
        let node = self.node(&self.root.clone(), window, cx);
        // Only controls mounted by this replacement frame may recover focus.
        self.presentation = NativePresentation::default();
        let slot_mask = self.slot_mask.clone();
        // This boundary belongs to the host, never to guest style. It also
        // supplies the mask captured by deferred guest draws.
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(
                canvas(
                    move |_, window, _| slot_mask.set(window.content_mask()),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
            .child(node)
    }
}
