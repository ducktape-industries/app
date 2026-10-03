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
    Styled, StyledImage as _, StyledText, Subscription, TextLayout, Transformation,
    WeakFocusHandle, Window, canvas, div, fill, img, point, px, radians, rgb, size, svg,
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
pub(crate) mod deferred;
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

use accessibility::accessible;
pub(crate) use accessibility::{Accessible, announce};
use canvas::{canvas_svg, native_canvas_commands, paint_canvas_commands};
pub(crate) use commands::dialog_entry;
use editor_mount::EditorMount;
use inputs::Field;
use pictures::SharedRasters;
pub(crate) use pictures::{PictureBytes, qr};
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

/// The gpui id of an element the host draws beside a view's children, or
/// gives a node the view left id-less. Its base is a code location, which
/// `validate_host` refuses from a view, so no id a view can send equals it,
/// by construction; the name is what tells one host element from another,
/// and what the /tree door prints for it.
pub(crate) fn host_id(name: impl Into<SharedString>) -> ElementId {
    static BASE: std::sync::LazyLock<Arc<ElementId>> = std::sync::LazyLock::new(|| {
        Arc::new(ElementId::CodeLocation(*std::panic::Location::caller()))
    });
    ElementId::NamedChild(BASE.clone(), name.into())
}

/// Whether `id` is one [`host_id`] made: the shape no view id has.
pub(crate) fn is_host_id(id: &ElementId) -> bool {
    matches!(id, ElementId::NamedChild(base, _) if matches!(**base, ElementId::CodeLocation(_)))
}

/// The name of the host id a seat draws around its view's tree, before the
/// module (`Seat::ax_mark`): the AX door reads the module off it.
pub(crate) const VIEW_MARK: &str = "view/";

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
    /// The seat's rasters, the cache itself: the new tree draws on in it.
    images: Option<SharedRasters>,
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
    /// Where the row a virtualized list draws next sits in its set
    /// (1-based position, set size); `node` hands it to that row alone.
    next_row: Option<(usize, usize)>,
    /// The drawn node's place in its list's set, for `guest_aria`: set on
    /// entering a list's row, cleared on entering any other node.
    row: Option<(usize, usize)>,
    /// The document order the next rich paragraph registers for selection.
    selection_order: std::rc::Rc<std::cell::Cell<u64>>,

    // Native widgets, one per mounted node.
    fields: HashMap<AuthoredPath, Field>,
    /// A rich text with links: its Tab stop and the link its arrows picked.
    links: HashMap<AuthoredPath, text::links::Links>,
    editors: HashMap<AuthoredPath, EditorMount>,
    /// The guest's editor documents; handed over by the runtime, not built here.
    editor_store: Option<crate::editor::wire::EditorStore>,
    sensors: HashMap<AuthoredPath, SensorState>,
    /// A resize handle's press position while its drag lasts.
    drags: HashMap<AuthoredPath, Point<Pixels>>,

    // Scrolling.
    /// Identified containers with `overflow.y: scroll`; the scroll widget commands' targets.
    scrolls: HashMap<AuthoredPath, ScrollHandle>,
    /// The identified nodes this render claims as a composite's active
    /// descendant, and those already scrolled into view: a claim that moves
    /// onto a node scrolls the plain scroller around it to it, once.
    claiming: std::collections::HashSet<AuthoredPath>,
    revealed: std::collections::HashSet<AuthoredPath>,
    uniform_lists: HashMap<AuthoredPath, UniformListHostState>,
    variable_lists: HashMap<VariableListKey, VariableList>,

    // Focus: two systems. A container enters the native focus path only on
    // an explicit Focus widget command (keyed by path, with the node kind
    // it was granted as); a guest `Interactivity::focus_handle` is a handle
    // the guest names by number.
    focus_targets: HashMap<AuthoredPath, (std::mem::Discriminant<wire::Node>, FocusHandle)>,
    guest_focus_targets: HashMap<u64, FocusHandle>,
    /// The overlays showing a dialog: where focus enters each, and what
    /// held focus as it opened.
    dialogs: HashMap<AuthoredPath, (FocusHandle, Option<WeakFocusHandle>)>,
    /// A dialog that has just closed: where focus entered it, and what
    /// held focus as it opened, which gets focus back if focus went with
    /// the dialog.
    opener: Option<(FocusHandle, WeakFocusHandle)>,

    // Measured geometry by path: what `measure` records for identified
    // containers, editor mounts and canvases, and the sensor canvas for sensors.
    bounds: HashMap<AuthoredPath, Bounds<Pixels>>,

    // Pictures by the guest's content hash: the seat's bytes, which a node
    // that names a hash alone draws from, and the rasters decoded from them.
    pictures: Arc<PictureBytes>,
    images: SharedRasters,

    // Plumbing.
    /// State carried over from the previous guest instance's tree; emptied by the first render.
    presentation: NativePresentation,
    /// When the host last received a real press or key aimed at this tree
    /// (`activate`): the view's transient activation, which the seat takes
    /// to its guest on the next turn.
    activation: std::cell::Cell<Option<std::time::Instant>>,
    /// Whether the guest whose tree this is may move the keys
    /// (`Seat::keys_free`, mirrored here): a dialog that opens in it takes
    /// the keyboard only then.
    keys_grant: bool,
    /// The seat this tree draws for, once a widget owns it: its renders and
    /// their time are counted there (docs/perf.md).
    perf_key: Option<crate::perf::Key>,
    #[cfg(test)]
    renders: u64,
}

impl EventEmitter<wire::Event> for ViewTree {}

/// The tree has drawn: every render says so, and the seat that ticked
/// waits for it before its next turn (one tick per draw).
pub(crate) struct Drawn;

impl EventEmitter<Drawn> for ViewTree {}

impl ViewTree {
    pub fn new(root: wire::Node) -> Self {
        Self {
            activation: Default::default(),
            keys_grant: false,
            root,
            focus_targets: HashMap::new(),
            guest_focus_targets: HashMap::new(),
            fields: HashMap::new(),
            links: HashMap::new(),
            authored_path: Vec::new(),
            scrolls: HashMap::new(),
            claiming: Default::default(),
            revealed: Default::default(),
            uniform_lists: HashMap::new(),
            variable_lists: HashMap::new(),
            drags: HashMap::new(),
            dialogs: HashMap::new(),
            opener: None,
            bounds: HashMap::new(),
            sensors: HashMap::new(),
            pictures: Default::default(),
            images: Default::default(),
            editor_store: None,
            editors: HashMap::new(),
            mounted: Default::default(),
            presentation: NativePresentation::default(),
            render_index: 0,
            next_row: None,
            row: None,
            selection_order: Default::default(),
            perf_key: None,
            #[cfg(test)]
            renders: 0,
        }
    }

    /// The perf key this tree's renders count under.
    pub(crate) fn with_perf_key(mut self, key: crate::perf::Key) -> Self {
        self.perf_key = Some(key);
        self
    }

    /// The first one-line field this tree mounted, for a test to focus and blur.
    #[cfg(test)]
    pub(crate) fn first_input_for_test(&self) -> Option<Entity<InputState>> {
        self.fields.values().next().map(|field| field.state.clone())
    }

    /// The native handle behind a guest focus handle, for a test to ask who has the keys.
    #[cfg(test)]
    pub(crate) fn guest_focus_for_test(&self, handle: u64) -> Option<FocusHandle> {
        self.guest_focus_targets.get(&handle).cloned()
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
        self.row = self.next_row.take();
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
        let _timed = self.perf_key.and_then(|key| {
            crate::perf::count(key, "renders", 1);
            crate::perf::time(key, "render")
        });
        self.mounted.clear();
        self.authored_path.clear();
        self.render_index = 0;
        cx.emit(Drawn);
        // a linked text the last render did not draw is gone
        self.links
            .retain(|_, links| std::mem::take(&mut links.drawn));
        // a claim the last render did not make is revealed again if it comes back
        let claimed = std::mem::take(&mut self.claiming);
        self.revealed.retain(|path| claimed.contains(path));
        // Paragraph selection order starts at a base unique to this view (its
        // entity id in the high 32 bits) and restarts there every render, so
        // paragraphs keep stable numbers and two views never interleave.
        self.selection_order
            .set((cx.entity_id().as_u64() & u64::from(u32::MAX)) << 32);
        if let Some((entry, opener)) = self.opener.take() {
            commands::dialog_exit(&entry, opener, window, cx);
        }
        // drawn in place: nothing in the walk reads `self.root`
        let root = std::mem::replace(&mut self.root, wire::Node::empty());
        let node = self.node(&root, window, cx);
        self.root = root;
        // Carried-over state is for the first render of a new tree only:
        // whatever it did not claim is dropped.
        self.presentation = NativePresentation::default();
        // Host-owned clip box around the guest root (guest style never
        // reaches it): the pane's mask, which the guest's layer (its
        // tooltips and deferred draws) is fitted and clipped to.
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(deferred::Layer(node))
    }
}
