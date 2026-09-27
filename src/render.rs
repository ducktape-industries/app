//! GPUI rendering of the existing WASM tree. Widget identities retain native
//! input state; interaction uses the same semantic events the guests consume.

use gpui_base::StyledExt as _;
use gpui_kit::MouseUpEvent;
use gpui_kit::base::FocusTrapElement as _;
use gpui_kit::component::input::{Input, InputContentType, InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, Context, CursorStyle, Div, Element, ElementId,
    Entity, EventEmitter, FocusHandle, Focusable as _, FollowMode, GlobalElementId, HitboxBehavior,
    Hsla, Image, ImageFormat, InspectorElementId, InteractiveElement as _, InteractiveText,
    IntoElement, KeyDownEvent, LayoutId, ListAlignment, ListSizingBehavior, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, ObjectFit, ParentElement as _, Pixels, Point, Render,
    RenderImage, ScrollHandle, SharedString, Size, Stateful, StatefulInteractiveElement as _,
    Styled, StyledImage as _, StyledText, Subscription, TextLayout, Transformation, Window, canvas,
    div, fill, img, point, px, radians, rgb, size, svg,
};
use std::collections::HashMap;
use std::sync::Arc;
use view_wire as wire;

/// How far past its border a thing that sizes can still be taken hold of:
/// a desk window's edges, a view's divider. Its grip reaches this much out
/// into the gap around it.
pub(crate) const GRAB: f32 = 5.;

mod accessibility;
mod anchored;
mod canvas;
mod commands;
mod deferred;
mod inputs;
mod interactivity;
mod layout;
mod picture_resources;
mod pictures;
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

pub(crate) use accessibility::{Accessible, accessible, announce, descendant_text};
#[cfg(test)]
use canvas::{append_arc, append_arc_to};
use canvas::{canvas_svg, native_canvas_commands, paint_canvas_commands};
pub(crate) use commands::dialog_entry;
use inputs::{EditorMount, Field};
#[cfg(test)]
use picture_resources::decode_image;
pub(crate) use pictures::qr;
use sensors::SensorState;
use style::{has_named_overlay, named_overlay, native_cursor};
use svg_limits::{SvgPaintSource, guarded_svg_paint, svg_data_allowed};
use uniform::UniformListHostState;
use variable_list::{VariableList, VariableListKey};

pub(crate) type AuthoredPath = Vec<wire::ElementIdWire>;

/// The gpui id for a sanitized wire id. Sanitize refuses every id that
/// cannot lower, so a frame that reached the renderer holds none.
pub(crate) fn native_id(id: &wire::ElementIdWire) -> ElementId {
    id.to_gpui().expect("sanitized element ids lower to gpui")
}

/// Pushes the node's identity onto `path` when it has one; the caller pops
/// on the way out when this answers `true`.
pub(crate) fn enter_scope(node: &wire::Node, path: &mut AuthoredPath) -> bool {
    match node.identity() {
        Some(id) => {
            path.push(id.clone());
            true
        }
        None => false,
    }
}

#[derive(Default)]
pub(crate) struct NativePresentation {
    images: HashMap<u64, Arc<RenderImage>>,
    vectors: HashMap<u64, Arc<[u8]>>,
    focused_container: Option<(AuthoredPath, std::mem::Discriminant<wire::Node>)>,
    inputs: HashMap<AuthoredPath, InputPresentation>,
    editors: HashMap<AuthoredPath, wire::editor_document::EditorDocumentRef>,
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
    drags: HashMap<AuthoredPath, Point<Pixels>>,
    /// The overlays showing a dialog, and where focus enters each.
    dialogs: HashMap<AuthoredPath, FocusHandle>,
    bounds: HashMap<AuthoredPath, Bounds<Pixels>>,
    sensors: HashMap<AuthoredPath, SensorState>,
    guest_focus_targets: HashMap<u64, FocusHandle>,
    uniform_lists: HashMap<AuthoredPath, UniformListHostState>,
    variable_lists: HashMap<VariableListKey, VariableList>,
    images: HashMap<u64, Arc<RenderImage>>,
    vectors: HashMap<u64, Arc<[u8]>>,
    editor_store: Option<crate::editor::wire::EditorStore>,
    editors: HashMap<AuthoredPath, EditorMount>,
    mounted: std::collections::HashSet<AuthoredPath>,
    presentation: NativePresentation,
    render_index: u64,
    /// The document order the next rich paragraph registers for selection.
    selection_order: std::rc::Rc<std::cell::Cell<u64>>,
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
            uniform_lists: HashMap::new(),
            variable_lists: HashMap::new(),
            drags: HashMap::new(),
            dialogs: HashMap::new(),
            bounds: HashMap::new(),
            sensors: HashMap::new(),
            images: HashMap::new(),
            vectors: HashMap::new(),
            editor_store: None,
            editors: HashMap::new(),
            mounted: Default::default(),
            presentation: NativePresentation::default(),
            render_index: 0,
            selection_order: Default::default(),
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
        let entered_scope = enter_scope(node, &mut self.authored_path);
        if entered_scope {
            self.mounted.insert(self.authored_path.clone());
        }
        use wire::Node;
        let element = match node {
            Node::Text(view_wire::TextNode { .. }) => self.text(node, cx),
            Node::Space { style } => div().refine_style(style).into_any_element(),
            Node::UniformList { .. } => self.uniform_list(node, window, cx),
            Node::List { .. } => self.variable_list(node, cx),
            Node::Container(view_wire::ContainerNode { .. }) => self.container(node, window, cx),
            Node::Input { .. } => self.input(node, window, cx),
            Node::Deferred { .. } => self.deferred(node, window, cx),
            Node::ResizeHandle { .. } => self.resize_handle(node, window, cx),
            Node::Sensor { .. } => self.sensor(node, window, cx),
            Node::RichText { .. } => self.rich_text(node, window, cx),
            Node::Image { .. } => self.picture(node, window, cx),
            Node::Svg { .. } => self.vector(node, window, cx),
            Node::Canvas { .. } => self.drawing(node, cx),
            // the host registers no surface: the slot says so where it would be
            Node::Surface {
                id, name, style, ..
            } => div()
                .id(native_id(id))
                .refine_style(style)
                .child(format!("Unavailable host surface: {name}"))
                .into_any_element(),
            Node::Overlay { .. } => self.overlay(node, window, cx),
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
        // Each view counts its paragraphs from its own base, so two views'
        // orders never interleave and a frame repeats the last one's.
        self.selection_order
            .set((cx.entity_id().as_u64() & u64::from(u32::MAX)) << 32);
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
