use super::*;

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
    /// `<window>` or `<window>/<module>`.
    #[serde(rename = "in")]
    pub(super) scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) bounds: Option<[i32; 4]>,
    #[serde(skip)]
    pub(super) node: NodeId,
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
    // and path segments; the id itself is resolved once all are known
    let mut out = Vec::new();
    let mut paths: Vec<(String, Vec<String>)> = Vec::new();
    type Frame = (NodeId, String, String, Vec<String>);
    let mut stack: Vec<Frame> = Vec::new();
    let root = update.tree.as_ref().map_or(NodeId(0), |tree| tree.root);
    let push_children =
        |stack: &mut Vec<Frame>, id: NodeId, prefix: &str, scope: &str, path: &[String]| {
            if let Some(node) = nodes.get(&id) {
                for child in node.children().iter().rev() {
                    stack.push((*child, prefix.to_owned(), scope.to_owned(), path.to_vec()));
                }
            }
        };
    // Children are pushed in reverse so this is a deterministic pre-order
    // walk in paint order; the last visible modal is the nested/tree-topmost
    // boundary, just as the last painted sibling is visually on top.
    let modal = topmost_modal(root, &nodes);
    match modal {
        Some(id) => stack.push((id, format!("{name}:"), name.to_owned(), Vec::new())),
        None => push_children(&mut stack, root, &format!("{name}:"), name, &[]),
    }
    while let Some((id, prefix, scope, mut path)) = stack.pop() {
        let Some(node) = nodes.get(&id) else { continue };
        if node.is_hidden() {
            continue;
        }
        let (prefix, scope, path) = match window.a11y_element_id(id) {
            Some(element) => match element_path(element.iter()) {
                (Some(module), path) => (
                    format!("{name}:{module}/"),
                    format!("{name}/{module}"),
                    path,
                ),
                (None, path) => (format!("{name}:"), name.to_owned(), path),
            },
            // a synthetic child: its element's path and its role
            None => {
                path.push(format!("{:?}", node.role()));
                (prefix, scope, path)
            }
        };
        push_children(&mut stack, id, &prefix, &scope, &path);
        let rect = node.bounds();
        let shown = rect.is_none_or(|r| {
            r.width() > 0.
                && r.height() > 0.
                && r.x1 > 0.
                && r.y1 > 0.
                && r.x0 < width
                && r.y0 < height
        });
        if !shown {
            continue;
        }
        let role = node.role();
        let private = node.class_name() == Some(crate::a11y::AX_PRIVATE);
        let secret = private || role == Role::PasswordInput;
        let name = match (private, node.label()) {
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
        if node.is_selected() == Some(true) {
            state.push("selected");
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
            for (action, word) in [
                (Action::Click, "press"),
                (Action::Focus, "focus"),
                (Action::SetValue, "set_value"),
                (Action::ScrollIntoView, "scroll_into_view"),
            ] {
                if node.supports_action(action) {
                    actions.push(word);
                }
            }
            if node.supports_action(Action::Focus) && is_text_input(role) {
                actions.push("type");
            }
        }
        paths.push((prefix, path));
        out.push(AxNode {
            id: String::new(),
            role: format!("{role:?}"),
            name,
            description,
            value,
            state,
            actions,
            scope,
            bounds: bounds
                .then_some(())
                .and(rect)
                .map(|r| [r.x0, r.y0, r.x1, r.y1].map(|edge| (edge / scale).round() as i32)),
            node: id,
        });
    }
    for (node, id) in out.iter_mut().zip(door_ids(&paths)) {
        node.id = id;
    }
    out
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

        assert_eq!(topmost_modal(root_id, &nodes), Some(visible_id));
    }
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
