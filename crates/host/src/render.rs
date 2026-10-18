//! GPUI rendering of the existing WASM tree. Widget identities retain native
//! input state; interaction uses the same semantic events the views consume.

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
pub const GRAB: f32 = 5.;

mod accessibility;
mod anchored;
pub mod av;
pub mod call;
mod canvas;
mod commands;
pub mod deferred;
mod frame;
mod inputs;
mod interactivity;
mod layout;
mod page_motion;
mod picture_resources;
mod pictures;
mod sensors;
mod sheet;
mod sideways;
mod style;
mod surfaces;
mod svg_limits;
mod text;
mod tooltip_containment;
mod uniform;
mod variable_list;
pub mod web;

#[cfg(any(test, feature = "test-support"))]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(any(test, feature = "test-support"))]
pub use test_support::{answered, plain_style, sanitize_whole, sent, test_style, test_styles};

use accessibility::accessible;
pub use accessibility::{Accessible, announce};
use canvas::{canvas_svg, native_canvas_commands, paint_canvas_commands};
pub use commands::dialog_entry;
use inputs::Field;
pub use pictures::PictureBytes;
use pictures::SharedRasters;
use sensors::SensorState;
use style::{has_named_overlay, named_overlay, named_sheet, native_cursor};
use svg_limits::{SvgPaintSource, guarded_svg_paint, svg_data_allowed};
use uniform::UniformListHostState;
use variable_list::VariableList;

// the runtime's: a widget command names its target by it (`View::answer`)
pub use crate::runtime::{AuthoredPath, enter_scope};

/// The gpui id for a sanitized wire id. Sanitize refuses every id that
/// cannot lower, so a frame that reached the renderer holds none.
pub fn native_id(id: &wire::ElementIdWire) -> ElementId {
    id.to_gpui().expect("sanitized element ids lower to gpui")
}

/// The gpui id of an element the host draws beside a view's children, or
/// gives a node the view left id-less. Its base is a code location, which
/// `validate_host` refuses from a view, so no id a view can send equals it,
/// by construction; the name is what tells one host element from another,
/// and what the /tree door prints for it.
pub fn host_id(name: impl Into<SharedString>) -> ElementId {
    static BASE: std::sync::LazyLock<Arc<ElementId>> = std::sync::LazyLock::new(|| {
        Arc::new(ElementId::CodeLocation(*std::panic::Location::caller()))
    });
    ElementId::NamedChild(BASE.clone(), name.into())
}

/// Whether `id` is one [`host_id`] made: the shape no view id has.
pub fn is_host_id(id: &ElementId) -> bool {
    matches!(id, ElementId::NamedChild(base, _) if matches!(**base, ElementId::CodeLocation(_)))
}

/// The name of the host id a seat draws around its view's tree, before the
/// program (`Seat::ax_mark`): the AX door reads the program off it.
pub const VIEW_MARK: &str = "view/";

/// Whether a node of `role` is a control: what is pressed, toggled, picked
/// or slid. Its words are its label, and a label is not text to read: as a
/// browser's button and a native app's controls do, it stays out of the
/// window's text selection, so a press on it that drifts is its press and
/// a double-click on it selects no word. A link, a list's option, a row or
/// a cell holds content (an address, a name) and stays selectable.
fn is_control(role: gpui_kit::Role) -> bool {
    use gpui_kit::Role::*;
    matches!(
        role,
        Button
            | DefaultButton
            | CheckBox
            | RadioButton
            | Switch
            | Tab
            | MenuItem
            | MenuItemCheckBox
            | MenuItemRadio
            | MenuListOption
            | ComboBox
            | EditableComboBox
            | Slider
            | SpinButton
            | Splitter
            | ScrollBar
            | ColorWell
            | DisclosureTriangle
    )
}

/// Whether the words inside a node of `role` stay out of the window's text
/// selection: a control's label (`is_control`), and a list marker: the
/// bullet or number before an item, a code line's number, a diff line's
/// +/−. A marker is drawn beside the content and is not part of it, as a
/// browser leaves an ordered list's numbers out of a selection and its copy:
/// a drag down a file copies its code, not its line numbers.
fn keeps_words_unselected(role: gpui_kit::Role) -> bool {
    is_control(role) || role == gpui_kit::Role::ListMarker
}

/// Native widget state worth carrying into a fresh `ViewTree` when a view's
/// view is re-instantiated: plain data only, no entity or handler id, since
/// the new view's handler ids mean different things. Taken by
/// `ViewTree::presentation`, consumed by the new tree's first frame.
#[derive(Default)]
pub struct NativePresentation {
    /// The seat's rasters, the cache itself: the new tree draws on in it.
    images: Option<SharedRasters>,
    focused_container: Option<(AuthoredPath, std::mem::Discriminant<wire::Node>)>,
    inputs: HashMap<AuthoredPath, InputPresentation>,
}

/// A text field as its user left it: restored only onto a field the new
/// view gives the same value and secrecy.
struct InputPresentation {
    value: String,
    secure: bool,
    selection: std::ops::Range<usize>,
    focused: bool,
}

/// A tree to draw and the table its nodes name their styles in: what a
/// seat hands its [`ViewTree`] with each frame, and what a tooltip's
/// content is kept as. An id means a style only beside its table.
pub struct Tree {
    pub root: wire::Node,
    pub styles: wire::Styles,
}

/// The style `id` names in `styles`, as the app draws it: a view names the
/// design crate's faces, and the app's stand in for them
/// ([`crate::fonts::app_family`]). Every node's style is read through here.
fn drawn_style(styles: &wire::Styles, id: wire::StyleId) -> Arc<gpui_kit::StyleRefinement> {
    let style = styles.share(id);
    let Some(family) = &style.text.font_family else {
        return style;
    };
    let face = crate::fonts::app_family(family);
    if face == *family {
        return style;
    }
    let mut style = Arc::unwrap_or_clone(style);
    style.text.font_family = Some(face);
    Arc::new(style)
}

/// The gpui entity that draws one view's wire tree and keeps the
/// native state that outlives a frame, keyed by [`AuthoredPath`]. `replace`
/// adopts each new root and retains every map to the paths it still mounts.
pub struct ViewTree {
    // The frame being drawn.
    root: wire::Node,
    /// The table `root`'s nodes name their styles in.
    styles: wire::Styles,
    /// The path of the node `node()` is drawing; pushed and popped on the way.
    authored_path: AuthoredPath,
    /// Counts the nodes of a render the view gave no id, each by its place
    /// under its nearest identified node (`node` restarts the count inside
    /// each one): the number of the host id one takes (`container-N`,
    /// `text-N`, `rich-N`, `primitive-N`). A container that makes no id
    /// (`container`) is counted too, so a number never depends on what an
    /// earlier node sets.
    render_index: u64,
    /// Where the row a virtualized list draws next sits in its set
    /// (1-based position, set size); `node` hands it to that row alone.
    next_row: Option<(usize, usize)>,
    /// The drawn node's place in its list's set, for `view_aria`: set on
    /// entering a list's row, cleared on entering any other node.
    row: Option<(usize, usize)>,
    /// The document order the next rich paragraph registers for selection.
    selection_order: std::rc::Rc<std::cell::Cell<u64>>,
    /// Whether the node being drawn sits inside a control or a list marker
    /// (`keeps_words_unselected`): its words are a label or a marker, not
    /// text to read, and stay out of the window's text selection.
    unselected: bool,

    // Native widgets, one per mounted node.
    fields: HashMap<AuthoredPath, Field>,
    /// The one keystroke interceptor the fields share, once one is mounted:
    /// a view's claim on a key runs before the engine's own bindings.
    keystrokes: Option<Subscription>,
    /// A rich text with links: its Tab stop and the link its arrows picked.
    links: HashMap<AuthoredPath, text::links::Links>,
    sensors: HashMap<AuthoredPath, SensorState>,
    /// The web pieces' pages (`web.rs`).
    webs: web::Pages,
    /// The camera and microphone pieces' devices (`av.rs`).
    media: av::Media,
    /// The pieces that publish or play a live track (`call.rs`), and the
    /// live session they open tracks on (none for a tree no view seats).
    calls: call::Calls,
    live: Option<crate::runtime::HostLive>,
    /// A resize handle's press position while its drag lasts.
    drags: HashMap<AuthoredPath, Point<Pixels>>,
    /// The page the frame shows on top of the view's stack, and the change
    /// from the page before it while that moves (`page_motion.rs`).
    page: Option<page_motion::Shown>,
    page_motion: Option<page_motion::PageMotion>,
    /// Under touch, each page a deeper one was pushed over, by its own
    /// depth, as it was last drawn: what the edge swipe uncovers under the
    /// page on top; and the swipe dragging that page off.
    kept: std::collections::BTreeMap<u32, Entity<ViewTree>>,
    finger: Option<page_motion::Finger>,
    /// A drawn sheet's detent and drag.
    sheets: HashMap<AuthoredPath, sheet::SheetState>,
    /// The sheet this render built, laid over the root (`sheet.rs`).
    sheet_layer: Option<AnyElement>,
    /// Under touch, a sheet the view dropped, going down as it was drawn;
    /// and while it is walked, how far in it is, its height, whether it
    /// slides (`sheet.rs`).
    leaving: Option<sheet::Leaving>,
    leaving_at: Option<(f32, Pixels, bool)>,
    /// The body the view is laid out in, as its seat last told the view
    /// (`Event::Viewport`): its width class docks a sheet or centres it.
    /// `None` before a seat tells it (a tree drawn alone): the window's.
    body: Option<Size<Pixels>>,

    // Scrolling.
    /// Identified containers with `overflow.y: scroll`; the scroll widget commands' targets.
    scrolls: HashMap<AuthoredPath, ScrollHandle>,
    /// Under touch, what takes a sideways drag itself (`sideways.rs`).
    sideways: sideways::Sideways,
    /// The identified nodes this render claims as a composite's active
    /// descendant, and those already scrolled into view: a claim that moves
    /// onto a node scrolls the plain scroller around it to it, once.
    claiming: std::collections::HashSet<AuthoredPath>,
    revealed: std::collections::HashSet<AuthoredPath>,
    uniform_lists: HashMap<AuthoredPath, UniformListHostState>,
    variable_lists: HashMap<AuthoredPath, VariableList>,

    // Focus: two systems. A container enters the native focus path only on
    // an explicit Focus widget command (keyed by path, with the node kind
    // it was granted as); a view `Interactivity::focus_handle` is a handle
    // the view names by number.
    focus_targets: HashMap<AuthoredPath, (std::mem::Discriminant<wire::Node>, FocusHandle)>,
    view_focus_targets: HashMap<u64, FocusHandle>,
    /// The overlays showing a dialog: where focus enters each, and what
    /// held focus as it opened.
    dialogs: HashMap<AuthoredPath, (FocusHandle, Option<WeakFocusHandle>)>,
    /// A dialog that has just closed: where focus entered it, and what
    /// held focus as it opened, which gets focus back if focus went with
    /// the dialog.
    opener: Option<(FocusHandle, WeakFocusHandle)>,
    /// The keys a node heard go down and has not heard come up, each told
    /// it came up if the keyboard leaves the node first (`interactivity`).
    held_keys: Vec<interactivity::HeldKey>,

    // Pictures by the view's content hash: the seat's bytes, which a node
    // that names a hash alone draws from, and the rasters decoded from them.
    pictures: Arc<PictureBytes>,
    images: SharedRasters,
    /// What the view built for the tree's tooltip routes, by route
    /// (`wire::TooltipResponse`): beside the tree, never written into it,
    /// kept while the tree holds the route (`tooltip_responses`).
    tooltips: HashMap<u32, TooltipContent>,

    // Plumbing.
    /// State carried over from the previous view instance's tree; emptied by the first render.
    presentation: NativePresentation,
    /// When the host last received a real press or key aimed at this tree
    /// (`activate`): the view's transient activation, which the seat takes
    /// to its view on the next turn.
    activation: std::cell::Cell<Option<std::time::Instant>>,
    /// Whether the view whose tree this is may move the keys
    /// (`Seat::keys_free`, mirrored here): a dialog that opens in it takes
    /// the keyboard only then.
    keys_grant: bool,
    /// The seat this tree draws for, once a widget owns it: its renders,
    /// their time and gpui's passes over what they build (`deferred::Layer`)
    /// are counted there (docs/perf.md).
    perf_key: Option<crate::perf::Key>,
    #[cfg(test)]
    renders: u64,
}

impl EventEmitter<wire::Event> for ViewTree {}

/// Files dropped on a node that listens for them (`on_file_drop`): its
/// route, and the paths, which the seat opens and holds for the view
/// before it tells it (`Event::FileDrop`).
pub struct Dropped {
    pub handler: u32,
    pub paths: Vec<std::path::PathBuf>,
}

impl EventEmitter<Dropped> for ViewTree {}

/// The content a tooltip route's response brought with the table it came
/// with, and for a rich text's tooltip the character it was built for.
struct TooltipContent {
    character_index: Option<u32>,
    content: Arc<wire::Node>,
    styles: wire::Styles,
}

/// The tree has drawn: every render says so, and the seat that ticked
/// waits for it before its next turn (one tick per draw).
pub struct Drawn;

impl EventEmitter<Drawn> for ViewTree {}

impl ViewTree {
    pub fn new(tree: impl Into<Tree>) -> Self {
        let Tree { root, styles } = tree.into();
        let page = page_motion::shown(&root);
        Self {
            activation: Default::default(),
            keys_grant: false,
            root,
            styles,
            focus_targets: HashMap::new(),
            view_focus_targets: HashMap::new(),
            fields: HashMap::new(),
            keystrokes: None,
            links: HashMap::new(),
            authored_path: Vec::new(),
            scrolls: HashMap::new(),
            sideways: Default::default(),
            claiming: Default::default(),
            revealed: Default::default(),
            uniform_lists: HashMap::new(),
            variable_lists: HashMap::new(),
            drags: HashMap::new(),
            sheets: HashMap::new(),
            sheet_layer: None,
            page,
            page_motion: None,
            kept: Default::default(),
            finger: None,
            leaving: None,
            leaving_at: None,
            body: None,
            dialogs: HashMap::new(),
            opener: None,
            held_keys: Vec::new(),
            sensors: HashMap::new(),
            webs: Default::default(),
            media: Default::default(),
            calls: Default::default(),
            live: None,
            pictures: Default::default(),
            images: Default::default(),
            tooltips: HashMap::new(),
            presentation: NativePresentation::default(),
            render_index: 0,
            next_row: None,
            row: None,
            selection_order: Default::default(),
            unselected: false,
            perf_key: None,
            #[cfg(test)]
            renders: 0,
        }
    }

    /// The body the view is laid out in, as its seat told the view: a
    /// sheet docks or centres by its width class.
    pub fn set_body(&mut self, body: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let body = body.map(|(width, height)| size(px(width), px(height)));
        if self.body != body {
            self.body = body;
            cx.notify();
        }
    }

    /// The perf key this tree's renders count under.
    pub fn with_perf_key(mut self, key: crate::perf::Key) -> Self {
        self.perf_key = Some(key);
        self
    }

    /// The live session the tree's publishing and remote pieces open their
    /// tracks on (`call.rs`).
    pub fn with_live(mut self, live: crate::runtime::HostLive) -> Self {
        self.live = Some(live);
        self
    }

    /// The first one-line field this tree mounted, for a test to focus and blur.
    #[cfg(any(test, feature = "test-support"))]
    pub fn first_input_for_test(&self) -> Option<Entity<InputState>> {
        self.fields.values().find_map(|field| match &field.engine {
            inputs::Engine::Line(state) => Some(state.clone()),
            inputs::Engine::Area(_) => None,
        })
    }

    /// The native handle behind a view focus handle, for a test to ask who has the keys.
    #[cfg(any(test, feature = "test-support"))]
    pub fn view_focus_for_test(&self, handle: u64) -> Option<FocusHandle> {
        self.view_focus_targets.get(&handle).cloned()
    }

    /// The style `id` names in the table of the tree being drawn, shared:
    /// the walk borrows the tree for everything else.
    fn style(&self, id: wire::StyleId) -> Arc<gpui_kit::StyleRefinement> {
        drawn_style(&self.styles, id)
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
        self.row = self.next_row.take();
        let index = self.row.map(|(position, _)| position - 1);
        let entered_scope = enter_scope(node, index, &mut self.authored_path);
        // The host numbers id-less nodes within the nearest id'd node, from
        // zero: gpui files them under that node's id, so a number only has
        // to be unique there, and so it does not move when something is
        // added before this node elsewhere, or when a list measures its
        // rows in another order. Element state keyed under the number (a
        // paragraph's place in the selection) survives both.
        let outer_index = entered_scope.then(|| std::mem::take(&mut self.render_index));
        let outer_unselected = self.unselected;
        self.unselected |= node
            .interactivity()
            .and_then(|interactivity| interactivity.role)
            .is_some_and(keeps_words_unselected);
        use wire::Node;
        let element = match node {
            Node::Text(view_wire::TextNode { .. }) => self.text(node),
            Node::Space => div().into_any_element(),
            Node::UniformList { .. } => self.uniform_list(node, window, cx),
            Node::List { .. } => self.variable_list(node, window, cx),
            Node::Container(view_wire::ContainerNode { .. }) => self.container(node, window, cx),
            Node::Field { .. } => self.field(node, window, cx),
            Node::Deferred { .. } => self.deferred(node, window, cx),
            Node::ResizeHandle { .. } => self.resize_handle(node, window, cx),
            Node::Sensor { .. } => self.sensor(node, window, cx),
            Node::RichText { .. } => self.rich_text(node, window, cx),
            Node::Image { .. } => self.picture(node, window, cx),
            Node::Svg { .. } => self.vector(node, window, cx),
            Node::Canvas { .. } => self.drawing(node, cx),
            Node::Overlay { .. } => self.overlay(node, window, cx),
            Node::Sheet { .. } => self.sheet(node, window, cx),
            Node::Anchored { .. } => self.anchored(node, window, cx),
            Node::Web { .. } => self.web(node, window, cx),
            Node::Camera { .. } => self.camera(node, cx),
            Node::Microphone { .. } => self.microphone(node, window, cx),
            Node::Remote { .. } => self.remote(node, cx),
        };
        if entered_scope {
            self.authored_path.pop();
        }
        if let Some(outer) = outer_index {
            self.render_index = outer;
        }
        self.unselected = outer_unselected;
        // a row with no id of its own is drawn under the index it is filed
        // under, so gpui files its ids (and their accessibility nodes and
        // element state) under the row too
        match index {
            Some(index) if node.identity().is_none() => Scope {
                id: ElementId::Integer(index as u64),
                element,
            }
            .into_any_element(),
            _ => element,
        }
    }
}

/// An element drawn under `id` without the box an id-carrying element lays
/// out: a list row with no id of its own under its index (what `.id(index)`
/// would do), a list under its own id (gpui's list element takes none).
struct Scope {
    id: ElementId,
    element: AnyElement,
}

impl IntoElement for Scope {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Scope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.element.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(window, cx);
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
        self.authored_path.clear();
        self.render_index = 0;
        self.sideways.clear();
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
        let sheet = match self.sheet_layer.take() {
            Some(sheet) => {
                self.leaving = None;
                Some(sheet)
            }
            None => self.leaving_sheet(window, cx),
        };
        // Carried-over state is for the first frame of a new tree only:
        // whatever that frame did not claim is dropped. The rows of a list
        // (variable or uniform) are built after this call returns, when gpui
        // lays the list out, so it lives to the end of the frame, not of
        // this call. A deferred node's content is built in this walk.
        if self.presentation.focused_container.is_some() || !self.presentation.inputs.is_empty() {
            let tree = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = tree.update(cx, |tree, _| tree.presentation = Default::default());
            });
        }
        // Host-owned clip box around the view root (view style never
        // reaches it): the pane's mask, which the view's layer (its
        // tooltips and deferred draws) is fitted and clipped to.
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(deferred::Layer(node, self.perf_key))
            // over the whole body, in the view's layer like the rest
            .children(sheet.map(|sheet| deferred::Layer(sheet, None)))
    }
}
