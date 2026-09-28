//! The door's view of one window: `Window::a11y_tree` turned into the node
//! list every read serves. [`snapshot`] walks the tree in paint order under
//! the topmost modal, drops what is hidden, zero-sized or off the viewport,
//! masks what is secret, and words the states and actions; [`door_ids`]
//! gives each node its stable id. The rest are the shapes of answers:
//! [`compact`], [`offers`], [`delta`], [`nearest`].
use super::*;
use gpui_kit::accesskit::{HasPopup, Invalid, Live};

/// The actions the door offers, each with its word: a node supporting one
/// is offered it, and `/act` performs each word (`actions::perform`).
pub(super) const ACTIONS: [(Action, &str); 9] = [
    (Action::Click, "press"),
    (Action::Focus, "focus"),
    (Action::SetValue, "set_value"),
    (Action::Increment, "increment"),
    (Action::Decrement, "decrement"),
    (Action::Expand, "expand"),
    (Action::Collapse, "collapse"),
    (Action::ShowContextMenu, "context_menu"),
    (Action::ScrollIntoView, "scroll_into_view"),
];

/// A composite whose rows the arrows pick: the focused row inside one is
/// its active descendant.
pub(super) const COMPOSITES: [&str; 8] = [
    "Tree",
    "ListBox",
    "Menu",
    "MenuBar",
    "Grid",
    "EditableComboBox",
    "RadioGroup",
    "TabList",
];

/// The element id a module view's host draws around the view's tree
/// (`runtime.rs`): what follows it in a path is that module's.
pub(crate) const VIEW_MARK: &str = "view/";
const MASK: &str = "•••";
const TEXT_MAX: usize = 120;

/// One visible node, as the door reports it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AxNode {
    pub(super) id: String,
    pub(crate) role: String,
    pub(super) name: String,
    /// what a screen reader reads after the name: why a control is
    /// disabled, a field's placeholder
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
    pub(crate) state: Vec<&'static str>,
    pub(crate) actions: Vec<&'static str>,
    #[serde(flatten)]
    pub(crate) more: Properties,
    /// `<window>` or `<window>/<module>`.
    #[serde(rename = "in")]
    pub(super) scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) bounds: Option<[i32; 4]>,
    #[serde(skip)]
    pub(super) node: NodeId,
    /// A node no element draws: one an element's `a11y_synthetic_children`
    /// pushed (a RichText's clickable range). gpui gives such a node no
    /// focus.
    #[serde(skip)]
    pub(super) synthetic: bool,
}

/// What a node says beyond its role, name, value, states and actions: each
/// key only when the node has it.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct Properties {
    /// `polite` or `assertive`: how a change inside it is announced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) live: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) modal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) level: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) keyboard_shortcut: Option<String>,
    /// The door id of the focused node inside this composite: gpui moves
    /// the tree's focus to the row that claims it, and never sets the
    /// AccessKit property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_descendant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position_in_set: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) size_of_set: Option<usize>,
    /// `true`, `grammar` or `spelling`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) invalid: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) required: bool,
    /// A field whose text is read and selected, never changed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) read_only: bool,
    /// `menu`, `listbox`, `tree`, `grid` or `dialog`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) has_popup: Option<&'static str>,
    /// The door id of the nearest ancestor the snapshot has; none at its root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
}

impl Properties {
    fn of(node: &gpui_kit::accesskit::Node) -> Self {
        Self {
            live: node.live().and_then(|live| match live {
                Live::Off => None,
                Live::Polite => Some("polite"),
                Live::Assertive => Some("assertive"),
            }),
            modal: node.is_modal(),
            level: node.level(),
            placeholder: node.placeholder().map(truncate),
            keyboard_shortcut: node.keyboard_shortcut().map(str::to_owned),
            position_in_set: node.position_in_set(),
            size_of_set: node.size_of_set(),
            invalid: node.invalid().map(|invalid| match invalid {
                Invalid::True => "true",
                Invalid::Grammar => "grammar",
                Invalid::Spelling => "spelling",
            }),
            required: node.is_required(),
            read_only: node.is_read_only(),
            has_popup: node.has_popup().map(|popup| match popup {
                HasPopup::Menu => "menu",
                HasPopup::Listbox => "listbox",
                HasPopup::Tree => "tree",
                HasPopup::Grid => "grid",
                HasPopup::Dialog => "dialog",
            }),
            ..Default::default()
        }
    }
}

/// The visible nodes of `window`'s last tree, in tree order; empty before
/// the window has built one. An active modal returns only its reachable
/// subtree, matching what the door promises a screen reader can reach.
pub(crate) fn snapshot(name: &str, window: &Window, bounds: bool) -> Vec<AxNode> {
    let Some(update) = window.a11y_tree() else {
        return Vec::new();
    };
    let nodes: HashMap<NodeId, &gpui_kit::accesskit::Node> =
        update.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let scale = f64::from(window.scale_factor());
    let viewport = window.viewport_size();
    let (width, height) = (
        f64::from(viewport.width) * scale,
        f64::from(viewport.height) * scale,
    );
    // each node's id prefix (`<window>:` or `<window>:<module>/`), scope
    // and path segments, and its parent's index in `out`; the ids
    // themselves are resolved once all are known. A synthetic child's own
    // segment is its role, and after the first of that role under one
    // node, its place among them: `Link`, `Link2`.
    let mut out: Vec<AxNode> = Vec::new();
    let mut paths: Vec<(String, Vec<String>)> = Vec::new();
    let mut parents: Vec<Option<usize>> = Vec::new();
    type Frame = (NodeId, String, String, Vec<String>, Option<usize>, String);
    let mut stack: Vec<Frame> = Vec::new();
    let root = update.tree.as_ref().map_or(NodeId(0), |tree| tree.root);
    let push_children = |stack: &mut Vec<Frame>,
                         id: NodeId,
                         prefix: &str,
                         scope: &str,
                         path: &[String],
                         parent: Option<usize>| {
        let Some(node) = nodes.get(&id) else { return };
        let mut seen: HashMap<Role, usize> = HashMap::new();
        let segments: Vec<String> = node
            .children()
            .iter()
            .map(
                |child| match (window.a11y_element_id(*child), nodes.get(child)) {
                    (None, Some(child)) => {
                        let nth = seen.entry(child.role()).or_default();
                        *nth += 1;
                        match *nth {
                            1 => format!("{:?}", child.role()),
                            nth => format!("{:?}{nth}", child.role()),
                        }
                    }
                    _ => String::new(),
                },
            )
            .collect();
        for (child, segment) in node.children().iter().zip(segments).rev() {
            stack.push((
                *child,
                prefix.to_owned(),
                scope.to_owned(),
                path.to_vec(),
                parent,
                segment,
            ));
        }
    };
    // Children are pushed in reverse so this is a deterministic pre-order
    // walk in paint order; the last visible modal is the nested/tree-topmost
    // boundary, just as the last painted sibling is visually on top.
    let modal = topmost_modal(root, &nodes);
    match modal {
        Some(id) => stack.push((
            id,
            format!("{name}:"),
            name.to_owned(),
            Vec::new(),
            None,
            String::new(),
        )),
        None => push_children(&mut stack, root, &format!("{name}:"), name, &[], None),
    }
    while let Some((id, prefix, scope, mut path, parent, segment)) = stack.pop() {
        let Some(node) = nodes.get(&id) else { continue };
        if node.is_hidden() {
            continue;
        }
        let synthetic = window.a11y_element_id(id).is_none();
        let (prefix, scope, path) = match window.a11y_element_id(id) {
            Some(element) => match element_path(element.iter()) {
                (Some(module), path) => (
                    format!("{name}:{module}/"),
                    format!("{name}/{module}"),
                    path,
                ),
                (None, path) => (format!("{name}:"), name.to_owned(), path),
            },
            // a synthetic child: its element's path and its own segment
            None => {
                path.push(segment);
                (prefix, scope, path)
            }
        };
        let rect = node.bounds();
        let shown = rect.is_none_or(|r| {
            r.width() > 0.
                && r.height() > 0.
                && r.x1 > 0.
                && r.y1 > 0.
                && r.x0 < width
                && r.y0 < height
        });
        // a node the snapshot drops is no one's parent: its children's is
        // the nearest one it keeps
        let ancestor = if shown { Some(out.len()) } else { parent };
        push_children(&mut stack, id, &prefix, &scope, &path, ancestor);
        if !shown {
            continue;
        }
        let role = node.role();
        let private = node.class_name() == Some(crate::a11y::AX_PRIVATE);
        let secret = private || role == Role::PasswordInput;
        // a Label is named by its value first, as the adapters name it
        // (gpui's `Text` sets only the value)
        let label = match role {
            Role::Label => node.value().or(node.label()),
            _ => node.label(),
        };
        // a private text field's name is its label; its value is the secret
        let name = match (private && !is_text_input(role), label) {
            (true, Some(_)) => MASK.to_owned(),
            (_, label) if node.class_name() == Some(crate::a11y::AX_WHOLE) => {
                label.unwrap_or_default().to_owned()
            }
            (_, label) => truncate(label.unwrap_or_default()),
        };
        let description = node.description().map(|text| {
            if private {
                MASK.to_owned()
            } else {
                truncate(text)
            }
        });
        let value = node
            .value()
            .map(truncate)
            .or_else(|| node.numeric_value().map(|value| value.to_string()))
            .map(|value| if secret { MASK.to_owned() } else { value });
        let mut state = Vec::new();
        if update.focus == id {
            state.push("focused");
        }
        if node.is_disabled() {
            state.push("disabled");
        }
        match node.is_selected() {
            Some(true) => state.push("selected"),
            Some(false) => state.push("unselected"),
            None => {}
        }
        match node.toggled() {
            Some(Toggled::True) => state.push("checked"),
            Some(Toggled::False) => state.push("unchecked"),
            Some(Toggled::Mixed) => state.push("mixed"),
            None => {}
        }
        match node.is_expanded() {
            Some(true) => state.push("expanded"),
            Some(false) => state.push("collapsed"),
            None => {}
        }
        if node.is_busy() {
            state.push("busy");
        }
        let mut actions = Vec::new();
        if !node.is_disabled() {
            for (action, word) in ACTIONS {
                if node.supports_action(action) {
                    actions.push(word);
                }
            }
            if node.supports_action(Action::Focus) && is_text_input(role) {
                actions.push("type");
            }
        }
        paths.push((prefix, path));
        parents.push(parent);
        out.push(AxNode {
            id: String::new(),
            role: format!("{role:?}"),
            name,
            description,
            value,
            state,
            actions,
            more: Properties::of(node),
            scope,
            bounds: bounds
                .then_some(())
                .and(rect)
                .map(|r| [r.x0, r.y0, r.x1, r.y1].map(|edge| (edge / scale).round() as i32)),
            node: id,
            synthetic,
        });
    }
    for (node, id) in out.iter_mut().zip(door_ids(&paths)) {
        node.id = id;
    }
    for (index, parent) in parents.iter().enumerate() {
        out[index].more.parent = parent.map(|parent| out[parent].id.clone());
        if out[index].state.contains(&"focused") {
            let focused = out[index].id.clone();
            let mut above = *parent;
            while let Some(at) = above {
                if COMPOSITES.contains(&out[at].role.as_str()) {
                    out[at].more.active_descendant = Some(focused.clone());
                }
                above = parents[at];
            }
        }
    }
    out
}

/// A modal scopes `window`'s snapshot now.
pub(super) fn modal_active(window: &Window) -> bool {
    window.a11y_tree().is_some_and(|update| {
        let nodes = update.nodes.iter().map(|(id, node)| (*id, node)).collect();
        let root = update.tree.as_ref().map_or(NodeId(0), |tree| tree.root);
        topmost_modal(root, &nodes).is_some()
    })
}

/// The last painted modal that is actually reachable. A hidden ancestor hides
/// its whole subtree, including modal descendants.
fn topmost_modal(
    root: NodeId,
    nodes: &HashMap<NodeId, &gpui_kit::accesskit::Node>,
) -> Option<NodeId> {
    let mut modal = None;
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = nodes.get(&id) else { continue };
        if node.is_hidden() {
            continue;
        }
        if node.is_modal() {
            modal = Some(id);
        }
        stack.extend(node.children().iter().rev().copied());
    }
    modal
}

/// Each `(prefix, path)`'s id: its last segment, widened by its ancestors'
/// only while another node in the window shares it.
pub(super) fn door_ids(paths: &[(String, Vec<String>)]) -> Vec<String> {
    let mut take = vec![1usize; paths.len()];
    let mut ids = loop {
        let ids: Vec<String> = paths
            .iter()
            .zip(&take)
            .map(|((prefix, path), take)| {
                format!(
                    "{prefix}{}",
                    path[path.len().saturating_sub(*take)..].join(".")
                )
            })
            .collect();
        let mut count: HashMap<&str, usize> = HashMap::new();
        for id in &ids {
            *count.entry(id).or_default() += 1;
        }
        let mut widened = false;
        for (index, id) in ids.iter().enumerate() {
            if count[id.as_str()] > 1 && take[index] < paths[index].1.len() {
                take[index] += 1;
                widened = true;
            }
        }
        if !widened {
            break ids;
        }
    };
    // ponytail: two elements whose paths differ only in dropped (per-run)
    // segments still share an id; tree order tells them apart. Give such an
    // element a name segment when one shows up.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for id in &mut ids {
        let count = seen.entry(id.clone()).or_default();
        *count += 1;
        if *count > 1 {
            *id = format!("{id}~{count}");
        }
    }
    ids
}

/// A path's module (after [`VIEW_MARK`]) and its stable segments: the
/// names call sites pass, not entities, focus handles or the kit's own
/// type-path ids (`gpui_component::button::button::Button`).
fn element_path<'a>(ids: impl Iterator<Item = &'a ElementId>) -> (Option<String>, Vec<String>) {
    let mut module = None;
    let mut path = Vec::new();
    for id in ids {
        match id {
            ElementId::Name(name) if name.starts_with(VIEW_MARK) => {
                module = Some(name[VIEW_MARK.len()..].to_owned());
                path.clear();
            }
            ElementId::View(_)
            | ElementId::FocusHandle(_)
            | ElementId::Uuid(_)
            | ElementId::CodeLocation(_)
            | ElementId::OpaqueId(_) => {}
            ElementId::Name(name) if name.contains("::") => {}
            id => path.push(id.to_string()),
        }
    }
    (module, path)
}

fn is_text_input(role: Role) -> bool {
    matches!(
        role,
        Role::TextInput
            | Role::EditableComboBox
            | Role::MultilineTextInput
            | Role::SearchInput
            | Role::EmailInput
            | Role::NumberInput
            | Role::PasswordInput
            | Role::PhoneNumberInput
            | Role::UrlInput
    )
}

fn truncate(text: &str) -> String {
    match text.char_indices().nth(TEXT_MAX) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// `compact=1`: the nodes that carry something — a name, a description, a
/// value, a state or an action. Pure structure is dropped.
pub(super) fn compact(nodes: &[AxNode]) -> Vec<&AxNode> {
    nodes
        .iter()
        .filter(|node| {
            !node.name.is_empty()
                || node.description.is_some()
                || node.value.is_some()
                || !node.state.is_empty()
                || !node.actions.is_empty()
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub(super) struct Offer {
    id: String,
    action: &'static str,
    label: String,
}

/// Everything that can be done now: one entry per node and action.
pub(super) fn offers(nodes: &[AxNode]) -> Vec<Offer> {
    nodes
        .iter()
        .flat_map(|node| {
            let label = format!("{} {}", node.role, node.name).trim_end().to_owned();
            node.actions.iter().map(move |action| Offer {
                id: node.id.clone(),
                action,
                label: label.clone(),
            })
        })
        .collect()
}

#[derive(Debug, Default, Serialize)]
pub(super) struct Delta {
    appeared: Vec<AxNode>,
    disappeared: Vec<String>,
    changed: Vec<AxNode>,
}

/// What an act did to the tree, by id: `changed` holds a node's new self.
pub(super) fn delta(before: &[AxNode], after: &[AxNode]) -> Delta {
    let old: HashMap<&str, String> = before
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                serde_json::to_string(node).unwrap_or_default(),
            )
        })
        .collect();
    let new: HashMap<&str, ()> = after.iter().map(|node| (node.id.as_str(), ())).collect();
    let mut delta = Delta::default();
    for node in after {
        match old.get(node.id.as_str()) {
            None => delta.appeared.push(node.clone()),
            Some(was) if *was != serde_json::to_string(node).unwrap_or_default() => {
                delta.changed.push(node.clone())
            }
            Some(_) => {}
        }
    }
    delta.disappeared = before
        .iter()
        .filter(|node| !new.contains_key(node.id.as_str()))
        .map(|node| node.id.clone())
        .collect();
    delta
}

/// Up to five ids a caller most likely meant: the words of its id found in
/// a node's name or id. Never picks one for it.
pub(super) fn nearest(id: &str, nodes: &[AxNode]) -> Vec<String> {
    let tail = id
        .split_once(':')
        .map_or(id, |(_, path)| path)
        .to_lowercase();
    let words: Vec<&str> = tail
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let mut scored: Vec<(usize, &AxNode)> = nodes
        .iter()
        .map(|node| {
            let hay = format!("{} {}", node.name, node.id).to_lowercase();
            (
                words.iter().filter(|word| hay.contains(*word)).count(),
                node,
            )
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(5)
        .map(|(_, node)| node.id.clone())
        .collect()
}

#[cfg(test)]
mod key_tests {
    //! The keys a node carries beyond role, name, value, states and actions.
    use super::*;
    use crate::a11y::Patch;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
    };

    /// A modal dialog holding a list whose one row has every key.
    struct Keys;

    impl Render for Keys {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let row = Patch {
                live: Some(Live::Polite),
                invalid: Some(Invalid::True),
                required: true,
                read_only: true,
                has_popup: Some(HasPopup::Listbox),
                ..Default::default()
            }
            .on(div()
                .id("row")
                .size(px(20.))
                .focusable()
                .tab_stop(true)
                .role(Role::ListBoxOption)
                .aria_label("One")
                .aria_selected(false)
                .aria_level(2)
                .aria_placeholder("hint")
                .aria_keyshortcuts("Ctrl+1")
                .aria_position_in_set(1)
                .aria_size_of_set(3));
            let list = div()
                .id("list")
                .size(px(100.))
                .role(Role::ListBox)
                .aria_label("Rows")
                .child(row);
            crate::a11y::modal(
                div()
                    .id("dialog")
                    .size(px(200.))
                    .role(Role::Dialog)
                    .aria_label("Pick")
                    .child(list),
            )
        }
    }

    /// Every key the door adds, each from what the node carries; the parent
    /// from the walk; the composite's active descendant from where focus is.
    #[gpui_kit::test]
    fn the_door_says_what_else_a_node_carries(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(300.), px(300.)), |_, _| Keys);
        let mut native = VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.focus_next(cx);
            window.render_frame(cx);
            window.render_frame(cx);
            snapshot("t", window, false)
        });
        let json = |id: &str| {
            let node = nodes.iter().find(|node| node.id == id).expect(id);
            serde_json::to_value(node).unwrap()
        };
        assert_eq!(
            json("t:dialog"),
            serde_json::json!({
                "id": "t:dialog", "role": "Dialog", "name": "Pick", "state": [], "actions": [],
                "modal": true, "in": "t",
            })
        );
        assert_eq!(
            json("t:list"),
            serde_json::json!({
                "id": "t:list", "role": "ListBox", "name": "Rows", "state": [], "actions": [],
                "active_descendant": "t:row", "parent": "t:dialog", "in": "t",
            })
        );
        assert_eq!(
            json("t:row"),
            serde_json::json!({
                "id": "t:row", "role": "ListBoxOption", "name": "One",
                "state": ["focused", "unselected"], "actions": ["focus"],
                "live": "polite", "level": 2, "placeholder": "hint",
                "keyboard_shortcut": "Ctrl+1", "position_in_set": 1, "size_of_set": 3,
                "invalid": "true", "required": true, "read_only": true, "has_popup": "listbox",
                "parent": "t:list", "in": "t",
            })
        );
    }
}
#[cfg(test)]
mod modal_tests;
#[cfg(test)]
mod offer_tests;
