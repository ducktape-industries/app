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
mod editor_mount;
mod frame;
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

pub(crate) use accessibility::{Accessible, announce};
use accessibility::{accessible, descendant_text};
use canvas::{canvas_svg, native_canvas_commands, paint_canvas_commands};
pub(crate) use commands::dialog_entry;
use editor_mount::EditorMount;
use inputs::Field;
pub(crate) use pictures::qr;
use sensors::SensorState;
use style::{has_named_overlay, named_overlay, native_cursor};
use svg_limits::{SvgPaintSource, guarded_svg_paint, svg_data_allowed};
use uniform::UniformListHostState;
use variable_list::{VariableList, VariableListKey};

/// The ids of a node and of its identified ancestors, root first: the key
/// every piece of retained native state is stored under. Only a node with an
/// id (`Node::identity`) adds a segment; one without shares its nearest
/// identified ancestor's path, so only identified nodes may write a map
/// keyed by it.
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

/// Native widget state worth carrying into a fresh `ViewTree` when a view's
/// guest is re-instantiated: plain data only, no entity or handler id, since
/// the new guest's handler ids mean different things. Taken by
/// `ViewTree::presentation`, consumed by the new tree's first render.
#[derive(Default)]
pub(crate) struct NativePresentation {
    images: HashMap<u64, Arc<RenderImage>>,
    vectors: HashMap<u64, Arc<[u8]>>,
    focused_container: Option<(AuthoredPath, std::mem::Discriminant<wire::Node>)>,
    inputs: HashMap<AuthoredPath, InputPresentation>,
    /// The focused editors' documents; the field named takes the caret back.
    editors: HashMap<AuthoredPath, wire::editor_document::EditorDocumentRef>,
}

/// A text field as its user left it: restored only onto a field the new
/// guest gives the same value and secrecy.
struct InputPresentation {
    value: String,
    secure: bool,
    selection: std::ops::Range<usize>,
    focused: bool,
}

/// The gpui entity that draws one guest view's wire tree and keeps the
/// native state that outlives a frame, keyed by [`AuthoredPath`]. `replace`
/// adopts each new root and retains every map to the paths it still mounts.
pub struct ViewTree {
    // The frame being drawn.
    root: wire::Node,
    /// The path of the node `node()` is drawing; pushed and popped on the way.
    authored_path: AuthoredPath,
    /// Every identified path this render passed: what a widget command may target.
    mounted: std::collections::HashSet<AuthoredPath>,
    /// Counts the anonymous elements of a render, for ids of their own.
    render_index: u64,
    /// The document order the next rich paragraph registers for selection.
    selection_order: std::rc::Rc<std::cell::Cell<u64>>,

    // Native widgets, one per mounted node.
    fields: HashMap<AuthoredPath, Field>,
    editors: HashMap<AuthoredPath, EditorMount>,
    /// The guest's editor documents; handed over by the runtime, not built here.
    editor_store: Option<crate::editor::wire::EditorStore>,
    sensors: HashMap<AuthoredPath, SensorState>,
    /// A resize handle's press position while its drag lasts.
    drags: HashMap<AuthoredPath, Point<Pixels>>,

    // Scrolling.
    /// Identified containers with `overflow.y: scroll`; the scroll widget commands' targets.
    scrolls: HashMap<AuthoredPath, ScrollHandle>,
    uniform_lists: HashMap<AuthoredPath, UniformListHostState>,
    variable_lists: HashMap<VariableListKey, VariableList>,

    // Focus: two systems. A container enters the native focus path only on
    // an explicit Focus widget command (keyed by path, with the node kind
    // it was granted as); a guest `Interactivity::focus_handle` is a handle
    // the guest names by number.
    focus_targets: HashMap<AuthoredPath, (std::mem::Discriminant<wire::Node>, FocusHandle)>,
    guest_focus_targets: HashMap<u64, FocusHandle>,
    /// The overlays showing a dialog, and where focus enters each.
    dialogs: HashMap<AuthoredPath, FocusHandle>,

    // Measured geometry of identified containers and editor mounts (`measure`).
    bounds: HashMap<AuthoredPath, Bounds<Pixels>>,

    // Decoded pictures by content hash; a frame resends bytes only for a
    // hash the host has not remembered.
    images: HashMap<u64, Arc<RenderImage>>,
    vectors: HashMap<u64, Arc<[u8]>>,

    // Plumbing.
    /// State carried over from the previous guest instance's tree; emptied by the first render.
    presentation: NativePresentation,
    /// The handler a real gesture last pressed; `take_user_activation` spends it once.
    user_activation: std::cell::Cell<Option<u32>>,
    /// The pane box this tree is clipped to, for its tooltip windows.
    slot_mask: tooltip_containment::SlotMask,
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
        // One arm, one method (Space, a bare div, is the one exception): this
        // dispatcher is on the recursion chain for every nesting level the
        // wire allows (`wire::MAX_DEPTH`), and an unoptimised build gives a
        // function the stack of ALL its arms at once — inlined bodies here
        // once cost 570 KiB a level and overflowed the main thread at a depth
        // of fourteen.
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
        // Paragraph selection order starts at a base unique to this view (its
        // entity id in the high 32 bits) and restarts there every render, so
        // paragraphs keep stable numbers and two views never interleave.
        self.selection_order
            .set((cx.entity_id().as_u64() & u64::from(u32::MAX)) << 32);
        let node = self.node(&self.root.clone(), window, cx);
        // Carried-over state is for the first render of a new tree only:
        // whatever it did not claim is dropped.
        self.presentation = NativePresentation::default();
        let slot_mask = self.slot_mask.clone();
        // Host-owned clip box around the guest root (guest style never
        // reaches it). The zero-size canvas records this box's content mask
        // into `slot_mask`, which tooltip windows (`tooltip_containment::
        // build`) use to stay inside this pane.
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
