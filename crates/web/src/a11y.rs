//! The page's own accessibility tree, under the page's node in the app's.
//!
//! While the app's accessibility is active (an assistive tool is attached),
//! Chromium builds the page's tree ([`Accessibility::follow`]) and sends it as
//! serialized `AXTreeUpdate`s, a CEF value this file reads as JSON. A
//! [`PageTree`] keeps it, CEF-free: one tree per document (the page's, and
//! an iframe's own), changed node by node as Chromium's own applier changes
//! it. [`PageTree::build`] hands it to gpui as the synthetic children of
//! the page's node: roles, names, values and states as Chromium has them,
//! bounds in window pixels, the page's focus as that node's active
//! descendant (accesskit's adapters report it as the focused node), and a
//! reader's Click on a node with a default action as a click at its centre.
//! The rest of a reader's actions have no path through CEF (see the
//! browser-poc2 a11y report).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::accesskit::{self, Action, NodeId, Role, Toggled};
use gpui_kit::{A11ySubtreeBuilder, FocusHandle, Window};
use serde::Deserialize;

use crate::page::Shared;

/// What the page's node holds of the page's tree, and whether Chromium is
/// asked to send it.
#[derive(Default)]
pub(crate) struct Accessibility {
    pub(crate) tree: RefCell<PageTree>,
    on: Cell<bool>,
    /// Each node a reader may Click and the page point it clicks, as the
    /// last build made them.
    clicks: RefCell<Vec<(NodeId, (f64, f64))>>,
}

impl Accessibility {
    /// Chromium builds the page's tree only while the app's own is
    /// `active`: it costs the renderer memory and time on every change. Off,
    /// the tree goes. Applied once the browser is there.
    pub(crate) fn follow(&self, host: Option<cef::BrowserHost>, active: bool) {
        if self.on.get() == active {
            return;
        }
        let Some(host) = host else { return };
        cef_side::set(&host, active);
        self.on.set(active);
        if !active {
            self.tree.replace(PageTree::default());
        }
    }
}

/// The page's tree as the synthetic children of the page's node (`tree`'s
/// parent), in the window's pixels at `shared`'s scale.
pub(crate) fn children(shared: &Shared, tree: &mut A11ySubtreeBuilder) {
    let clicks = shared.a11y.tree.borrow().build(tree, shared.scale.get());
    shared.a11y.clicks.replace(clicks);
}

/// A reader's Click on a page node: the page takes the keys, as a pointer's
/// press gives them, and a click at the node's centre. gpui keeps a node's
/// listeners for one frame, so each paint registers them again.
pub(crate) fn actions(shared: &Rc<Shared>, focus: &FocusHandle, window: &mut Window) {
    if !window.is_a11y_active() {
        return;
    }
    for &(id, at) in shared.a11y.clicks.borrow().iter() {
        let (shared, focus) = (shared.clone(), focus.clone());
        window.on_a11y_action(id, Action::Click, move |_, window, cx| {
            window.focus(&focus, cx);
            if let Some(host) = shared.host() {
                cef_side::click(&host, at);
            }
        });
    }
}

pub(crate) fn handler(shared: Rc<Shared>) -> cef::AccessibilityHandler {
    cef_side::handler(shared)
}

/// One Chromium accessibility notification: the updates of one document's
/// tree (`ax_tree_id`).
#[derive(Deserialize)]
struct TreeChange {
    ax_tree_id: String,
    #[serde(default)]
    updates: Vec<Update>,
}

/// Chromium's `AXTreeUpdate`: the nodes it sends replace the ones it held
/// (their children lists too); a node no longer reached from the root is
/// gone.
#[derive(Deserialize)]
struct Update {
    #[serde(default)]
    root_id: i32,
    #[serde(default)]
    node_id_to_clear: i32,
    tree_data: Option<TreeData>,
    #[serde(default)]
    nodes: Vec<Data>,
}

#[derive(Deserialize)]
struct TreeData {
    /// An iframe's document has its parent's; the page's top one has none.
    parent_tree_id: Option<String>,
    #[serde(default)]
    focus_id: i32,
    /// Set on the top document: the document holding the focus.
    focused_tree_id: Option<String>,
}

/// One node as Chromium serializes it (`AXNodeData`), with the attributes
/// this file reads; its location is relative to its offset container's.
#[derive(Clone, Deserialize)]
struct Data {
    id: i32,
    role: String,
    #[serde(default)]
    attributes: Attributes,
    #[serde(default)]
    state: Vec<String>,
    #[serde(default)]
    child_ids: Vec<i32>,
    #[serde(default)]
    location: Location,
    #[serde(default)]
    offset_container_id: i32,
}

impl Data {
    fn has(&self, state: &str) -> bool {
        self.state.iter().any(|held| held == state)
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attributes {
    name: Option<String>,
    value: Option<String>,
    description: Option<String>,
    placeholder: Option<String>,
    hierarchical_level: Option<usize>,
    checked_state: Option<String>,
    restriction: Option<String>,
    input_type: Option<String>,
    default_action_verb: Option<String>,
    url: Option<String>,
    selected: Option<bool>,
    scroll_x: Option<f64>,
    scroll_y: Option<f64>,
    child_tree_id: Option<String>,
}

#[derive(Clone, Copy, Default, Deserialize)]
struct Location {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// A node that moved without its tree changing.
#[derive(Deserialize)]
struct Moved {
    ax_tree_id: String,
    id: i32,
    new_location: NewLocation,
}

#[derive(Deserialize)]
struct NewLocation {
    bounds: Location,
    #[serde(default)]
    offset_container_id: i32,
}

/// One document's tree, as Chromium last sent it.
#[derive(Default)]
struct Tree {
    root: i32,
    nodes: HashMap<i32, Data>,
    focus: i32,
    focused_tree: Option<String>,
}

impl Tree {
    fn apply(&mut self, update: Update) {
        match update.node_id_to_clear {
            0 => {}
            clear if clear == self.root => self.nodes.clear(),
            clear => {
                if let Some(node) = self.nodes.get_mut(&clear) {
                    node.child_ids.clear();
                }
            }
        }
        if update.root_id != 0 {
            self.root = update.root_id;
        }
        if let Some(data) = update.tree_data {
            self.focus = data.focus_id;
            self.focused_tree = data.focused_tree_id;
        }
        for node in update.nodes {
            self.nodes.insert(node.id, node);
        }
    }

    /// Drops every node the root no longer reaches.
    fn prune(&mut self) {
        let mut reached = HashSet::new();
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if reached.insert(id)
                && let Some(node) = self.nodes.get(&id)
            {
                stack.extend(&node.child_ids);
            }
        }
        self.nodes.retain(|id, _| reached.contains(id));
    }
}

/// The page's accessibility tree: its top document's, and each iframe's
/// document's, by Chromium's tree id.
#[derive(Default)]
pub(crate) struct PageTree {
    top: Option<String>,
    trees: HashMap<String, Tree>,
    /// The top document's scroll offset. CEF passes Chromium's scroll
    /// updates on as an empty location change, so it comes from the render
    /// handler's `on_scroll_offset_changed`.
    scroll: (f64, f64),
}

impl PageTree {
    /// Chromium's `OnAccessibilityTreeChange`, as JSON.
    pub(crate) fn changed(&mut self, json: &str) {
        let Ok(change) = serde_json::from_str::<TreeChange>(json) else {
            return;
        };
        for update in change.updates {
            let top = update
                .tree_data
                .as_ref()
                .is_some_and(|data| data.parent_tree_id.is_none());
            // a new top document (a navigation): what came before goes
            if top && self.top.as_ref() != Some(&change.ax_tree_id) {
                self.trees.clear();
                self.top = Some(change.ax_tree_id.clone());
            }
            // ponytail: an iframe's tree stays until the next top document,
            // even once its iframe is gone; prune by `childTreeId` if pages
            // that churn iframes grow it
            self.trees
                .entry(change.ax_tree_id.clone())
                .or_default()
                .apply(update);
        }
        if let Some(tree) = self.trees.get_mut(&change.ax_tree_id) {
            tree.prune();
        }
    }

    /// Chromium's `OnAccessibilityLocationChange`, as JSON.
    pub(crate) fn moved(&mut self, json: &str) {
        let Ok(moves) = serde_json::from_str::<Vec<Moved>>(json) else {
            return;
        };
        for moved in moves {
            let node = self
                .trees
                .get_mut(&moved.ax_tree_id)
                .and_then(|tree| tree.nodes.get_mut(&moved.id));
            if let Some(node) = node {
                node.location = moved.new_location.bounds;
                node.offset_container_id = moved.new_location.offset_container_id;
            }
        }
    }

    /// The top document scrolled to `x`, `y`.
    pub(crate) fn scrolled(&mut self, x: f64, y: f64) {
        self.scroll = (x, y);
    }

    /// Pushes the page's nodes as `tree`'s synthetic children: the top
    /// document's root becomes the parent's only child, and the page's
    /// focus its active descendant. Page pixels are `scale` window pixels.
    /// Returns each node a reader may Click, with the page point it clicks.
    pub(crate) fn build(
        &self,
        tree: &mut A11ySubtreeBuilder,
        scale: f32,
    ) -> Vec<(NodeId, (f64, f64))> {
        let Some(top) = self.top.as_deref() else {
            return Vec::new();
        };
        let Some(piece) = tree.parent_node().bounds() else {
            return Vec::new();
        };
        let mut build = Build {
            page: self,
            piece,
            scale: f64::from(scale),
            nodes: Vec::new(),
            seen: HashSet::new(),
            clicks: Vec::new(),
        };
        let roots = self
            .trees
            .get(top)
            .map(|document| build.node(tree, top, document.root, (0., 0.)))
            .unwrap_or_default();
        let focus = self.trees.get(top).map(|document| {
            let held = document.focused_tree.as_deref().unwrap_or(top);
            let focus = self.trees.get(held).map_or(0, |held| held.focus);
            id(tree, held, focus)
        });
        for (id, node) in build.nodes {
            tree.push_child(id, node);
        }
        // `push_child` lists every node under the page's node; only the
        // root is its child
        let parent = tree.parent_node();
        parent.set_children(roots);
        if let Some(focus) = focus.filter(|focus| build.seen.contains(focus)) {
            parent.set_active_descendant(focus);
        }
        build.clicks
    }
}

/// The id of node `node` of document `tree` under the page's node.
fn id(builder: &A11ySubtreeBuilder, tree: &str, node: i32) -> NodeId {
    builder.synthetic_node_id((tree, node))
}

/// One build of a page's nodes.
struct Build<'a> {
    page: &'a PageTree,
    /// The page's node, in window pixels.
    piece: accesskit::Rect,
    scale: f64,
    nodes: Vec<(NodeId, accesskit::Node)>,
    seen: HashSet<NodeId>,
    clicks: Vec<(NodeId, (f64, f64))>,
}

impl Build<'_> {
    /// The nodes that stand for node `node` of document `tree`: itself, or,
    /// for a node Chromium marks ignored, its kept descendants. `origin` is
    /// the document's place on the page (an iframe's, for its document).
    fn node(
        &mut self,
        builder: &A11ySubtreeBuilder,
        tree: &str,
        node: i32,
        origin: (f64, f64),
    ) -> Vec<NodeId> {
        let Some(document) = self.page.trees.get(tree) else {
            return Vec::new();
        };
        let Some(data) = document.nodes.get(&node) else {
            return Vec::new();
        };
        let id = id(builder, tree, node);
        // a node reached twice would be listed under two parents
        if data.role == "inlineTextBox" || !self.seen.insert(id) {
            return Vec::new();
        }
        let top = self.page.top.as_deref() == Some(tree);
        let place = self.place(document, data, top, origin);
        let mut children: Vec<NodeId> = data
            .child_ids
            .iter()
            .flat_map(|&child| self.node(builder, tree, child, origin))
            .collect();
        if let Some(inner) = &data.attributes.child_tree_id
            && let Some(document) = self.page.trees.get(inner)
        {
            let inner = inner.clone();
            children.extend(self.node(builder, &inner, document.root, (place.x, place.y)));
        }
        if data.has("ignored") || data.has("invisible") {
            return children;
        }
        let mut converted = convert(data);
        converted.set_children(children);
        let shown = self.bounds(place);
        // a default action is a click on the node, while it is on the page
        if data.attributes.default_action_verb.is_some() && shown.area() > 0. {
            converted.add_action(Action::Click);
            let centre = (place.x + place.width / 2., place.y + place.height / 2.);
            self.clicks.push((id, centre));
        }
        converted.set_bounds(shown);
        self.nodes.push((id, converted));
        vec![id]
    }

    /// Where `data` is on the page, in page pixels: its location, plus
    /// each offset container's, less what each has scrolled.
    fn place(&self, document: &Tree, data: &Data, top: bool, origin: (f64, f64)) -> Location {
        let (mut x, mut y) = (data.location.x + origin.0, data.location.y + origin.1);
        let (mut at, mut container) = (data.id, data.offset_container_id);
        // ponytail: CSS transforms are not applied; a transformed box reads
        // at its untransformed place
        for _ in 0..64 {
            if container == at {
                break;
            }
            let Some(holder) = document.nodes.get(&container) else {
                break;
            };
            // ponytail: an inner scroller's offset is as of its last update;
            // only the top document's follows every scroll
            let (scroll_x, scroll_y) = match top && container == document.root {
                true => self.page.scroll,
                false => (
                    holder.attributes.scroll_x.unwrap_or(0.),
                    holder.attributes.scroll_y.unwrap_or(0.),
                ),
            };
            x += holder.location.x - scroll_x;
            y += holder.location.y - scroll_y;
            (at, container) = (container, holder.offset_container_id);
        }
        Location {
            x,
            y,
            ..data.location
        }
    }

    /// A page place in window pixels, clipped to the page's node: a node
    /// scrolled out of the page is an empty box at its edge.
    fn bounds(&self, place: Location) -> accesskit::Rect {
        let piece = self.piece;
        let clamp_x = |x: f64| (piece.x0 + x * self.scale).clamp(piece.x0, piece.x1);
        let clamp_y = |y: f64| (piece.y0 + y * self.scale).clamp(piece.y0, piece.y1);
        accesskit::Rect {
            x0: clamp_x(place.x),
            y0: clamp_y(place.y),
            x1: clamp_x(place.x + place.width),
            y1: clamp_y(place.y + place.height),
        }
    }
}

/// A Chromium node as an accesskit node: role, name, value and states.
fn convert(data: &Data) -> accesskit::Node {
    let attributes = &data.attributes;
    let role = role(data);
    let mut node = accesskit::Node::new(role);
    if let Some(name) = attributes.name.as_deref().filter(|name| !name.is_empty()) {
        node.set_label(name);
        // the adapters name a Label by its value
        if role == Role::Label {
            node.set_value(name);
        }
    }
    if let Some(value) = &attributes.value {
        node.set_value(value.as_str());
    }
    if let Some(description) = &attributes.description {
        node.set_description(description.as_str());
    }
    if let Some(placeholder) = &attributes.placeholder {
        node.set_placeholder(placeholder.as_str());
    }
    if let Some(level) = attributes.hierarchical_level {
        node.set_level(level);
    }
    if let Some(url) = &attributes.url {
        node.set_url(url.as_str());
    }
    if let Some(selected) = attributes.selected {
        node.set_selected(selected);
    }
    match attributes.checked_state.as_deref() {
        Some("true") => node.set_toggled(Toggled::True),
        Some("false") => node.set_toggled(Toggled::False),
        Some("mixed") => node.set_toggled(Toggled::Mixed),
        _ => {}
    }
    match attributes.restriction.as_deref() {
        Some("disabled") => node.set_disabled(),
        Some("readOnly") => node.set_read_only(),
        _ => {}
    }
    if data.has("required") {
        node.set_required();
    }
    if data.has("expanded") {
        node.set_expanded(true);
    } else if data.has("collapsed") {
        node.set_expanded(false);
    }
    node
}

/// Chromium's role as accesskit's: the same names (accesskit took
/// Chromium's), but for text and its fields; one accesskit lacks is a plain
/// container.
fn role(data: &Data) -> Role {
    match data.role.as_str() {
        "staticText" | "labelText" => Role::Label,
        "textField" => match data.attributes.input_type.as_deref() {
            Some("password") => Role::PasswordInput,
            Some("email") => Role::EmailInput,
            Some("tel") => Role::PhoneNumberInput,
            Some("url") => Role::UrlInput,
            _ if data.has("multiline") => Role::MultilineTextInput,
            _ => Role::TextInput,
        },
        "searchBox" => Role::SearchInput,
        "textFieldWithComboBox" => Role::EditableComboBox,
        "comboBoxSelect" | "comboBoxMenuButton" | "popUpButton" => Role::ComboBox,
        "toggleButton" => Role::Button,
        role => serde_json::from_value(serde_json::Value::String(role.to_owned()))
            .unwrap_or(Role::GenericContainer),
    }
}

/// Everything here that names CEF's own types (they clash with gpui's).
mod cef_side {
    use crate::page::Shared;
    use cef::*;
    use std::rc::Rc;

    pub(super) fn set(host: &BrowserHost, on: bool) {
        host.set_accessibility_state(if on { State::ENABLED } else { State::DISABLED });
    }

    /// A left click at page point `at`.
    pub(super) fn click(host: &BrowserHost, (x, y): (f64, f64)) {
        let event = MouseEvent {
            x: x as i32,
            y: y as i32,
            modifiers: 0,
        };
        host.send_mouse_move_event(Some(&event), 0);
        for up in [0, 1] {
            host.send_mouse_click_event(Some(&event), MouseButtonType::LEFT, up, 1);
        }
    }

    pub(super) fn handler(shared: Rc<Shared>) -> AccessibilityHandler {
        PageAccessibility::new(shared)
    }

    fn json(value: Option<&mut Value>) -> String {
        CefString::from(&write_json(value, JsonWriterOptions::DEFAULT)).to_string()
    }

    wrap_accessibility_handler! {
        struct PageAccessibility {
            shared: Rc<Shared>,
        }

        impl AccessibilityHandler {
            fn on_accessibility_tree_change(&self, value: Option<&mut Value>) {
                self.shared.a11y.tree.borrow_mut().changed(&json(value));
                self.shared.dirty.set(true);
            }

            fn on_accessibility_location_change(&self, value: Option<&mut Value>) {
                self.shared.a11y.tree.borrow_mut().moved(&json(value));
                self.shared.dirty.set(true);
            }
        }
    }
}
