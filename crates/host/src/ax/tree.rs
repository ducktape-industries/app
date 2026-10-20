//! The door's view of one window: `Window::a11y_tree` turned into the node
//! list every read serves. [`snapshot`] walks the tree in paint order under
//! the topmost modal, drops what is hidden, zero-sized or off the viewport,
//! masks what is secret, and words the states and actions; [`door_ids`]
//! gives each node its stable id. The rest are the shapes of answers:
//! [`compact`], [`offers`], [`delta`], [`nearest`].
use super::*;
use crate::render::VIEW_MARK;
use gpui_kit::accesskit::{HasPopup, Invalid, Live, Orientation};

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

const MASK: &str = "•••";
const TEXT_MAX: usize = 120;

/// One visible node, as the door reports it.
#[derive(Clone, Debug, Serialize)]
pub struct AxNode {
    pub id: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub state: Vec<&'static str>,
    pub actions: Vec<&'static str>,
    #[serde(flatten)]
    pub more: Properties,
    #[serde(rename = "in")]
    pub(super) scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) bounds: Option<[i32; 4]>,
    #[serde(skip)]
    pub(super) node: NodeId,
    #[serde(skip)]
    pub(super) synthetic: bool,
}

/// What a node says beyond its role, name, value, states and actions: each
/// key only when the node has it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Properties {
    /// `polite` or `assertive`: how a change inside it is announced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub modal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard_shortcut: Option<String>,
    /// The door id of the focused node inside this composite: gpui moves
    /// the tree's focus to the row that claims it, and sets the AccessKit
    /// property only on a web page's node, for the page's own focus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_descendant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_in_set: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_of_set: Option<usize>,
    /// `true`, `grammar` or `spelling`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// A field whose text is read and selected, never changed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// `menu`, `listbox`, `tree`, `grid` or `dialog`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_popup: Option<&'static str>,
    /// `horizontal` or `vertical`, on any node that has one; AX-107's probe
    /// presses a composite's arrows by it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<&'static str>,
    /// The door id of the nearest ancestor the snapshot has; none at its root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
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
            orientation: node.orientation().map(|orientation| match orientation {
                Orientation::Horizontal => "horizontal",
                Orientation::Vertical => "vertical",
            }),
            ..Default::default()
        }
    }
}

/// The visible nodes of `window`'s last tree, in tree order; empty before
/// the window has built one. An active modal returns only its reachable
/// subtree, matching what the door promises a screen reader can reach.
pub fn snapshot(name: &str, window: &Window, bounds: bool) -> Vec<AxNode> {
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
    let focused = nodes
        .get(&update.focus)
        .and_then(|node| node.active_descendant())
        .filter(|id| nodes.contains_key(id))
        .unwrap_or(update.focus);
    let modal = last_reachable_painted_modal(root, &nodes);
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
                (Some(program), path) => (
                    format!("{name}:{program}/"),
                    format!("{name}/{program}"),
                    path,
                ),
                (None, path) => (format!("{name}:"), name.to_owned(), path),
            },
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
        let ancestor = if shown { Some(out.len()) } else { parent };
        push_children(&mut stack, id, &prefix, &scope, &path, ancestor);
        if !shown {
            continue;
        }
        let role = node.role();
        let private = node.class_name() == Some(crate::a11y::AX_PRIVATE);
        let secret = private || role == Role::PasswordInput;
        let label = match role {
            Role::Label => node.value().or(node.label()),
            _ => node.label(),
        };
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
        if focused == id {
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
            if node.supports_action(Action::Focus) && is_text_input(role) && !node.is_read_only() {
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

pub(super) fn modal_active(window: &Window) -> bool {
    window.a11y_tree().is_some_and(|update| {
        let nodes = update.nodes.iter().map(|(id, node)| (*id, node)).collect();
        let root = update.tree.as_ref().map_or(NodeId(0), |tree| tree.root);
        last_reachable_painted_modal(root, &nodes).is_some()
    })
}

fn last_reachable_painted_modal(
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

pub(super) fn scope_of(name: &str, element: &gpui_kit::GlobalElementId) -> String {
    match element_path(element.iter()).0 {
        Some(program) => format!("{name}/{program}"),
        None => name.to_owned(),
    }
}

fn element_path<'a>(ids: impl Iterator<Item = &'a ElementId>) -> (Option<String>, Vec<String>) {
    let mut program = None;
    let mut path = Vec::new();
    for id in ids {
        match id {
            ElementId::NamedChild(_, name) if crate::render::is_host_id(id) => {
                match name.strip_prefix(VIEW_MARK) {
                    Some(mark) => {
                        program = Some(mark.to_owned());
                        path.clear();
                    }
                    None => path.push(name.to_string()),
                }
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
    (program, path)
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
    use super::*;
    use crate::a11y::Patch;
    use gpui_kit::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
    };

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
                .aria_orientation(Orientation::Vertical)
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

    #[gpui_kit::test]
    fn the_door_says_what_else_a_node_carries(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(300.), px(300.)), |_, _| Keys);
        let mut native = VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            window.focus_next(cx);
            crate::render::draw_frame(window, cx);
            crate::render::draw_frame(window, cx);
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
                "active_descendant": "t:row", "orientation": "vertical", "parent": "t:dialog",
                "in": "t",
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
mod mark_tests {
    use super::*;
    use gpui_kit::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
    };

    struct Forged;

    impl Render for Forged {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let approve = div()
                .id("approve")
                .size(px(20.))
                .role(Role::Button)
                .aria_label("Approve");
            div()
                .id(crate::render::host_id(format!("{VIEW_MARK}chat")))
                .size(px(200.))
                .child(div().id("view/forge").size(px(100.)).child(approve))
        }
    }

    #[gpui_kit::test]
    fn a_view_cannot_name_its_nodes_another_programs(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(300.), px(300.)), |_, _| Forged);
        let mut native = VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            snapshot("t", window, false)
        });
        let approve = nodes
            .iter()
            .find(|node| node.name == "Approve")
            .expect("the button");
        assert_eq!(
            (approve.scope.as_str(), approve.id.as_str()),
            ("t/chat", "t:chat/approve")
        );
    }
}

#[cfg(test)]
mod modal_tests {
    use super::*;

    #[test]
    fn a_modal_below_a_hidden_ancestor_does_not_replace_the_visible_modal() {
        let (root_id, visible_id, hidden_id, hidden_modal_id) =
            (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
        let mut root = gpui_kit::accesskit::Node::new(Role::Window);
        root.set_children([visible_id, hidden_id]);
        let mut visible = gpui_kit::accesskit::Node::new(Role::Dialog);
        visible.set_modal();
        let mut hidden = gpui_kit::accesskit::Node::new(Role::GenericContainer);
        hidden.set_hidden();
        hidden.set_children([hidden_modal_id]);
        let mut hidden_modal = gpui_kit::accesskit::Node::new(Role::Dialog);
        hidden_modal.set_modal();
        let nodes = HashMap::from([
            (root_id, &root),
            (visible_id, &visible),
            (hidden_id, &hidden),
            (hidden_modal_id, &hidden_modal),
        ]);

        assert_eq!(
            last_reachable_painted_modal(root_id, &nodes),
            Some(visible_id)
        );
    }
}
#[cfg(test)]
mod offer_tests {
    use super::*;
    use gpui_kit::{
        AccessibleAction, Context, InteractiveElement as _, IntoElement, Render,
        StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
    };

    const MORE: [(AccessibleAction, &str); 6] = [
        (AccessibleAction::Increment, "increment"),
        (AccessibleAction::Decrement, "decrement"),
        (AccessibleAction::Expand, "expand"),
        (AccessibleAction::Collapse, "collapse"),
        (AccessibleAction::ShowContextMenu, "context_menu"),
        (AccessibleAction::ScrollIntoView, "scroll_into_view"),
    ];

    struct Handles(std::rc::Rc<std::cell::RefCell<Vec<AccessibleAction>>>);

    impl Render for Handles {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            MORE.iter().fold(
                div()
                    .id("stepper")
                    .size(px(100.))
                    .role(Role::SpinButton)
                    .aria_label("Count")
                    .on_a11y_action(AccessibleAction::ScrollDown, |_, _, _| {}),
                |element, (action, _)| {
                    let done = self.0.clone();
                    let action = *action;
                    element.on_a11y_action(action, move |_, _, _| done.borrow_mut().push(action))
                },
            )
        }
    }

    #[gpui_kit::test]
    fn the_door_offers_and_performs_each_action_a_node_handles(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let done = std::rc::Rc::default();
        let seen = std::rc::Rc::clone(&done);
        let window = cx.open_window(size(px(200.), px(200.)), move |_, _| Handles(seen));
        let mut native = VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.activate_a11y();
            crate::render::draw_frame(window, cx);
            crate::render::draw_frame(window, cx);
            let node = snapshot("t", window, false)
                .into_iter()
                .find(|node| node.role == "SpinButton")
                .expect("the stepper is in the tree");
            let words: Vec<_> = MORE.iter().map(|(_, word)| *word).collect();
            assert_eq!(node.actions, words);
            for word in &words {
                assert!(super::super::actions::perform_by_id(
                    "t", window, cx, &node.id, word, ""
                ));
                crate::render::draw_frame(window, cx);
            }
        });
        let performed: Vec<_> = MORE.iter().map(|(action, _)| *action).collect();
        assert_eq!(*done.borrow(), performed);
    }
}

#[cfg(test)]
mod active_descendant_tests {
    use super::*;
    use gpui_kit::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, Render,
        StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
    };

    struct Page(FocusHandle);

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("page")
                .size(px(100.))
                .role(Role::Group)
                .aria_label("Page")
                .track_focus(&self.0)
                .a11y_synthetic_children(|tree| {
                    let field = tree.synthetic_node_id("field");
                    let mut node = gpui_kit::accesskit::Node::new(Role::TextInput);
                    node.set_label("User name");
                    tree.push_child(field, node);
                    tree.parent_node().set_active_descendant(field);
                })
        }
    }

    #[gpui_kit::test]
    fn the_focused_nodes_active_descendant_is_focused(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let handle = cx.update(|cx| cx.focus_handle());
        let tracked = handle.clone();
        let window = cx.open_window(size(px(200.), px(200.)), move |_, _| Page(tracked));
        let mut native = VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            window.focus(&handle, cx);
            crate::render::draw_frame(window, cx);
            crate::render::draw_frame(window, cx);
            snapshot("t", window, false)
        });
        let state = |name: &str| {
            let node = nodes.iter().find(|node| node.name == name).expect(name);
            node.state.clone()
        };
        assert_eq!(state("User name"), ["focused"]);
        assert!(state("Page").is_empty());
    }
}
