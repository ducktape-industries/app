//! The test door (#114): a loopback HTTP door over the SAME AccessKit tree
//! GPUI hands the OS, so a QA runner reads and drives the app the way a
//! screen reader does. There is no second tree: every answer is read off
//! `Window::a11y_tree`, every act goes through the adapter's own action path.
//!
//! Off unless the app is launched with `DUCKTAPE_AX_DOOR=<port|0>` (0 picks a
//! free port). It binds 127.0.0.1 only — [`bind`] takes a port, never a host
//! — and writes `{port, token}` to [`door_file`] mode 0600; a request without
//! that token is refused. `ducktape-app ax …` ([`cli`]) is its client.
//!
//! Ids are `<window>:<element id>`: a native node's own string ElementId, a
//! view node's `<module>/<wire key>`, widened with its ancestors' ids
//! (`rail.view:chat`) only while another node in the window shares it. Entity,
//! focus-handle and the kit's type-path segments never count: they change per
//! run or say nothing. Never an AccessKit NodeId, never a position. A password field's value and a node marked
//! [`gpui_notion::editor::ui::AX_PRIVATE`] (the recovery-phrase words) are
//! masked here, before anything leaves the process.
use futures::StreamExt as _;
use gpui_kit::accesskit::{Action, ActionData, ActionRequest, NodeId, Role, Toggled, TreeId};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, ElementId, Window};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The element id a module view's host draws around the view's tree
/// (`module_view.rs`): what follows it in a path is that module's.
pub(crate) const VIEW_MARK: &str = "view/";
const MASK: &str = "•••";
const TEXT_MAX: usize = 120;
const POLL: Duration = Duration::from_millis(50);

/// One visible node, as the door reports it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AxNode {
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
    pub(crate) state: Vec<&'static str>,
    pub(crate) actions: Vec<&'static str>,
    /// `<window>` or `<window>/<module>`.
    #[serde(rename = "in")]
    pub(crate) scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bounds: Option<[i32; 4]>,
    #[serde(skip)]
    node: NodeId,
}

/// The visible nodes of `window`'s last tree, in tree order; empty before
/// the window has built one.
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
    push_children(&mut stack, root, &format!("{name}:"), name, &[]);
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
        let private = node.class_name() == Some(gpui_notion::editor::ui::AX_PRIVATE);
        let secret = private || role == Role::PasswordInput;
        let name = match (private, node.label()) {
            (true, Some(_)) => MASK.to_owned(),
            (_, label) => truncate(label.unwrap_or_default()),
        };
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

/// Each `(prefix, path)`'s id: its last segment, widened by its ancestors'
/// only while another node in the window shares it.
pub(crate) fn door_ids(paths: &[(String, Vec<String>)]) -> Vec<String> {
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

/// `compact=1`: the nodes that carry something — a name, a value, a state
/// or an action. Pure structure is dropped.
pub(crate) fn compact(nodes: &[AxNode]) -> Vec<&AxNode> {
    nodes
        .iter()
        .filter(|node| {
            !node.name.is_empty()
                || node.value.is_some()
                || !node.state.is_empty()
                || !node.actions.is_empty()
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Offer {
    id: String,
    action: &'static str,
    label: String,
}

/// Everything that can be done now: one entry per node and action.
pub(crate) fn offers(nodes: &[AxNode]) -> Vec<Offer> {
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
pub(crate) struct Delta {
    appeared: Vec<AxNode>,
    disappeared: Vec<String>,
    changed: Vec<AxNode>,
}

/// What an act did to the tree, by id: `changed` holds a node's new self.
pub(crate) fn delta(before: &[AxNode], after: &[AxNode]) -> Delta {
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
pub(crate) fn nearest(id: &str, nodes: &[AxNode]) -> Vec<String> {
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

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Filter {
    window: Option<String>,
    view: Option<String>,
}

impl Filter {
    fn keeps(&self, node: &AxNode) -> bool {
        let (window, view) = match node.scope.split_once('/') {
            Some((window, view)) => (window, Some(view)),
            None => (node.scope.as_str(), None),
        };
        self.window.as_deref().is_none_or(|want| want == window)
            && self.view.as_deref().is_none_or(|want| Some(want) == view)
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Act {
    id: String,
    action: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Wait {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, rename = "in")]
    scope: Option<String>,
    #[serde(default)]
    gone: bool,
    deadline_ms: u64,
}

impl Wait {
    fn matches(&self, node: &AxNode) -> bool {
        self.role
            .as_deref()
            .is_none_or(|role| role.eq_ignore_ascii_case(&node.role))
            && self
                .name
                .as_deref()
                .is_none_or(|name| node.name.to_lowercase().contains(&name.to_lowercase()))
            && self
                .state
                .as_deref()
                .is_none_or(|state| node.state.contains(&state))
            && self.scope.as_deref().is_none_or(|scope| {
                node.scope == scope || node.scope.starts_with(&format!("{scope}/"))
            })
    }

    /// The answer once the wait is decided: the match (or its absence, for
    /// `gone`), or 408 with the compact tree when `expired`.
    pub(crate) fn step(&self, nodes: &[AxNode], expired: bool) -> Option<Reply> {
        let found = nodes.iter().find(|node| self.matches(node));
        match (found, self.gone) {
            (Some(node), false) => Some(Reply::ok(json!({ "node": node }))),
            (None, true) => Some(Reply::ok(json!({ "gone": true }))),
            _ if expired => Some(Reply::new(
                408,
                json!({ "error": "deadline passed", "tree": compact(nodes) }),
            )),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Request {
    Tree {
        filter: Filter,
        compact: bool,
        bounds: bool,
    },
    Actions(Filter),
    Act(Act),
    Wait(Wait),
}

#[derive(Debug, PartialEq)]
pub(crate) struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    pub(crate) fn new(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            body: body.to_string(),
        }
    }

    fn ok(body: serde_json::Value) -> Self {
        Self::new(200, body)
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> &str {
        &self.body
    }
}

/// A request and where its answer goes.
pub(crate) type Call = (Request, std::sync::mpsc::Sender<Reply>);

/// Answers the door's calls on the app's thread until the door is gone.
/// `windows` lists the windows it serves, by name.
pub(crate) async fn serve(
    mut calls: futures::channel::mpsc::UnboundedReceiver<Call>,
    windows: impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    cx: &mut AsyncApp,
) {
    while let Some((request, reply)) = calls.next().await {
        let answer = answer(request, &windows, cx).await;
        let _ = reply.send(answer);
    }
}

async fn answer(
    request: Request,
    windows: &impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    cx: &mut AsyncApp,
) -> Reply {
    let all = Filter::default();
    match request {
        Request::Tree {
            filter,
            compact: small,
            bounds,
        } => {
            let nodes = read(windows, &filter, bounds, cx).await;
            match small {
                true => Reply::ok(json!(compact(&nodes))),
                false => Reply::ok(json!(nodes)),
            }
        }
        Request::Actions(filter) => {
            Reply::ok(json!(offers(&read(windows, &filter, false, cx).await)))
        }
        Request::Act(act) => {
            let before = read(windows, &all, false, cx).await;
            let Some(target) = before.iter().find(|node| node.id == act.id) else {
                return Reply::new(
                    404,
                    json!({ "error": "no such node", "nearest": nearest(&act.id, &before) }),
                );
            };
            if !target.actions.contains(&act.action.as_str()) {
                return Reply::new(
                    400,
                    json!({ "error": "not an action of this node", "actions": target.actions }),
                );
            }
            let name = target
                .scope
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            let handle = cx
                .update(|cx| windows(cx))
                .into_iter()
                .find_map(|(window, handle)| (window == name).then_some(handle));
            let value = act.value.unwrap_or_default();
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, cx| {
                    perform_by_id(&name, window, cx, &act.id, &act.action, &value)
                });
            }
            // the next settled frame: the tree unchanged for three polls
            let deadline = Instant::now() + Duration::from_millis(act.deadline_ms.unwrap_or(2000));
            let mut after = before.clone();
            let mut last = serde_json::to_string(&after).unwrap_or_default();
            let mut quiet = 0;
            while quiet < 3 && Instant::now() < deadline {
                cx.background_executor().timer(POLL).await;
                let now = read(windows, &all, false, cx).await;
                let key = serde_json::to_string(&now).unwrap_or_default();
                match key == last {
                    true => quiet += 1,
                    false => (quiet, last, after) = (0, key, now),
                }
            }
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Wait(wait) => {
            let deadline = Instant::now() + Duration::from_millis(wait.deadline_ms.min(60_000));
            loop {
                let nodes = read(windows, &all, false, cx).await;
                if let Some(reply) = wait.step(&nodes, Instant::now() >= deadline) {
                    return reply;
                }
                cx.background_executor().timer(POLL).await;
            }
        }
    }
}

/// Every served window's visible nodes. A window with no tree yet is
/// switched on and given up to a second to draw one.
async fn read(
    windows: &impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    filter: &Filter,
    bounds: bool,
    cx: &mut AsyncApp,
) -> Vec<AxNode> {
    let list = cx.update(|cx| windows(cx));
    let mut out = Vec::new();
    for _ in 0..20 {
        out.clear();
        let mut pending = false;
        for (name, handle) in &list {
            if filter.window.as_deref().is_some_and(|want| want != name) {
                continue;
            }
            let nodes = handle.update(cx, |_, window, _| {
                if !window.is_a11y_active() || window.a11y_tree().is_none() {
                    window.activate_a11y();
                    return None;
                }
                Some(snapshot(name, window, bounds))
            });
            match nodes {
                Ok(Some(nodes)) => out.extend(nodes),
                Ok(None) => pending = true,
                Err(_) => {}
            }
        }
        if !pending {
            break;
        }
        cx.background_executor().timer(POLL).await;
    }
    out.retain(|node| filter.keeps(node));
    out
}

/// Performs `action` on the node `id` names in window `name`, through the
/// path an assistive technology's request takes; `type` focuses it and sends
/// each character as a key. False when no such node is showing.
pub(crate) fn perform_by_id(
    name: &str,
    window: &mut Window,
    cx: &mut App,
    id: &str,
    action: &str,
    value: &str,
) -> bool {
    let node = snapshot(name, window, false)
        .into_iter()
        .find_map(|node| (node.id == id).then_some(node.node));
    if let Some(node) = node {
        perform(window, cx, node, action, value);
    }
    node.is_some()
}

fn perform(window: &mut Window, cx: &mut App, node: NodeId, action: &str, value: &str) {
    let request = |action, data| ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: node,
        data,
    };
    match action {
        "press" => window.dispatch_a11y_action(request(Action::Click, None), cx),
        "focus" => window.dispatch_a11y_action(request(Action::Focus, None), cx),
        "scroll_into_view" => {
            window.dispatch_a11y_action(request(Action::ScrollIntoView, None), cx)
        }
        "set_value" => window.dispatch_a11y_action(
            request(Action::SetValue, Some(ActionData::Value(value.into()))),
            cx,
        ),
        "type" => {
            window.dispatch_a11y_action(request(Action::Focus, None), cx);
            for ch in value.chars() {
                let (key, text) = match ch {
                    '\n' => ("enter".to_owned(), None),
                    '\t' => ("tab".to_owned(), None),
                    ' ' => ("space".to_owned(), Some(" ".to_owned())),
                    ch => (ch.to_string(), Some(ch.to_string())),
                };
                window.dispatch_keystroke(
                    gpui_kit::Keystroke {
                        modifiers: Default::default(),
                        key,
                        key_char: text,
                    },
                    cx,
                );
            }
        }
        _ => {}
    }
}

/// `DUCKTAPE_AX_DOOR`: unset or empty is no door; a port (0 picks one) is a
/// door; anything else — an address included — is refused.
pub(crate) fn door_port(value: Option<&str>) -> Result<Option<u16>, String> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(port) => port.parse().map(Some).map_err(|_| {
            format!("DUCKTAPE_AX_DOOR={port}: a port number (0 picks one); the door binds 127.0.0.1 only")
        }),
    }
}

/// The only listener the door opens: 127.0.0.1, on `port`.
pub(crate) fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port))
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct DoorFile {
    pub(crate) port: u16,
    pub(crate) token: String,
}

/// `$XDG_RUNTIME_DIR/ducktape/ax-door.json`, else the app's state directory.
pub(crate) fn door_file() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("ducktape").join("ax-door.json")),
        None => Ok(crate::backend::state_dir()?.join("ax-door.json")),
    }
}

/// Writes the door's port and token, readable by this user only.
pub(crate) fn write_door_file(path: &Path, door: &DoorFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(serde_json::to_string(door).unwrap_or_default().as_bytes())
}

/// Opens the door when `DUCKTAPE_AX_DOOR` asks for it; the calls it takes
/// arrive on the returned channel for [`serve`].
pub(crate) fn open() -> Option<futures::channel::mpsc::UnboundedReceiver<Call>> {
    open_env(std::env::var("DUCKTAPE_AX_DOOR").ok().as_deref())
}

pub(crate) fn open_env(
    value: Option<&str>,
) -> Option<futures::channel::mpsc::UnboundedReceiver<Call>> {
    let port = match door_port(value) {
        Ok(port) => port?,
        Err(error) => {
            tracing::warn!(target: "ducktape::app", reason = "ax_door_refused", %error, "the test door stays shut");
            return None;
        }
    };
    let opened = bind(port).and_then(|listener| Ok((listener.local_addr()?.port(), listener)));
    let (port, listener) = opened
        .inspect_err(|error| tracing::warn!(target: "ducktape::app", reason = "ax_door_unbound", %error, "the test door stays shut"))
        .ok()?;
    let door = DoorFile {
        port,
        token: format!("{:032x}", rand::random::<u128>()),
    };
    let written = door_file()
        .and_then(|path| write_door_file(&path, &door).map_err(|error| error.to_string()));
    if let Err(error) = written {
        tracing::warn!(target: "ducktape::app", reason = "ax_door_file_unwritten", %error, "the test door stays shut");
        return None;
    }
    let (sender, calls) = futures::channel::mpsc::unbounded();
    let token = door.token;
    std::thread::Builder::new()
        .name("ax-door".into())
        .spawn(move || {
            accept(listener, &token, |request| {
                let (reply, answer) = std::sync::mpsc::channel();
                sender.unbounded_send((request, reply)).ok()?;
                answer.recv().ok()
            })
        })
        .ok()?;
    tracing::info!(target: "ducktape::app", port, "ax_door_open");
    Some(calls)
}

/// The door's HTTP/1.1 loop: one request per connection, answered in turn.
/// ponytail: one at a time, so a long `wait` holds the next caller; the one
/// consumer is a sequential runner.
pub(crate) fn accept(
    listener: TcpListener,
    token: &str,
    answer: impl Fn(Request) -> Option<Reply>,
) {
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let reply = match read_request(&stream) {
            Err(_) => Reply::new(400, json!({ "error": "not an HTTP/1.1 request" })),
            Ok((_, _, auth, _)) if !same(auth.as_deref().unwrap_or_default(), token) => {
                Reply::new(401, json!({ "error": "the door's token is required" }))
            }
            Ok((method, target, _, body)) => match route(&method, &target, &body) {
                Ok(request) => answer(request)
                    .unwrap_or_else(|| Reply::new(503, json!({ "error": "the app is closing" }))),
                Err(reply) => reply,
            },
        };
        let reason = match reply.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            408 => "Request Timeout",
            _ => "Service Unavailable",
        };
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            reply.status,
            reply.body.len(),
            reply.body
        );
    }
}

/// A token comparison that takes the same time wherever the first
/// difference is.
fn same(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Method, target, bearer token and body.
fn read_request(stream: &TcpStream) -> std::io::Result<(String, String, Option<String>, Vec<u8>)> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let (mut auth, mut length) = (None, 0usize);
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("authorization") {
                auth = value.strip_prefix("Bearer ").map(str::to_owned);
            } else if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().map_err(|_| invalid())?;
            }
        }
    }
    if length > 1 << 20 {
        return Err(invalid());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok((method, target, auth, body))
}

fn route(method: &str, target: &str, body: &[u8]) -> Result<Request, Reply> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .collect();
    let filter = Filter {
        window: params.get("window").map(|value| value.to_string()),
        view: params.get("view").map(|value| value.to_string()),
    };
    let flag = |name| params.get(name) == Some(&"1");
    match (method, path.trim_start_matches('/')) {
        ("GET", "tree") => Ok(Request::Tree {
            filter,
            compact: flag("compact"),
            bounds: flag("bounds"),
        }),
        ("GET", "actions") => Ok(Request::Actions(filter)),
        ("POST", "act") => parse(body).map(Request::Act),
        ("POST", "wait") => parse(body).map(Request::Wait),
        _ => Err(Reply::new(
            404,
            json!({ "error": "no such endpoint", "endpoints": ["GET /tree", "GET /actions", "POST /act", "POST /wait"] }),
        )),
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(body)
        .map_err(|error| Reply::new(400, json!({ "error": error.to_string() })))
}

/// One call to the door: its status and body.
pub(crate) fn call(
    door: &DoorFile,
    method: &str,
    target: &str,
    body: &str,
) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, door.port))?;
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        door.token,
        body.len()
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    let body = response.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    Ok((status, body.to_owned()))
}

const USAGE: &str = "usage: ducktape-app ax tree [--window W] [--view V] [--compact] [--bounds]
       ducktape-app ax actions [--window W] [--view V]
       ducktape-app ax act <id> <press|focus|set_value|type|scroll_into_view> [value]
       ducktape-app ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]";

/// `ducktape-app ax …`: prints the door's JSON. Exit 0 answered, 1 not
/// found, refused or timed out, 2 the door is not open.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut words = Vec::new();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.strip_prefix("--") {
            Some(name @ ("compact" | "bounds" | "gone")) => {
                flags.insert(name, "1");
            }
            Some(name) => {
                flags.insert(name, rest.next().map_or("", String::as_str));
            }
            None => words.push(arg.as_str()),
        }
    }
    let query = |names: &[&str]| {
        names
            .iter()
            .filter_map(|name| flags.get(name).map(|value| format!("{name}={value}")))
            .collect::<Vec<_>>()
            .join("&")
    };
    let (method, target, body) = match (args.first().map(String::as_str), &words[..]) {
        (Some("tree"), []) => ("GET", format!("/tree?{}", query(&["window", "view", "compact", "bounds"])), String::new()),
        (Some("actions"), []) => ("GET", format!("/actions?{}", query(&["window", "view"])), String::new()),
        (Some("act"), [id, action, value @ ..]) if value.len() <= 1 => (
            "POST",
            "/act".to_owned(),
            json!({ "id": id, "action": action, "value": value.first() }).to_string(),
        ),
        (Some("wait"), []) => (
            "POST",
            "/wait".to_owned(),
            json!({
                "role": flags.get("role"),
                "name": flags.get("name"),
                "state": flags.get("state"),
                "in": flags.get("in"),
                "gone": flags.contains_key("gone"),
                "deadline_ms": flags.get("deadline-ms").and_then(|ms| ms.parse::<u64>().ok()).unwrap_or(5000),
            })
            .to_string(),
        ),
        _ => {
            eprintln!("{USAGE}");
            return 1;
        }
    };
    let door = door_file().and_then(|path| {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_str::<DoorFile>(&text).map_err(|error| error.to_string())
    });
    let shut = "the door is not open: launch the app with DUCKTAPE_AX_DOOR=<port|0>";
    let door = match door {
        Ok(door) => door,
        Err(error) => {
            eprintln!("ax: {shut} ({error})");
            return 2;
        }
    };
    match call(&door, method, &target, &body) {
        Ok((status, body)) => {
            println!("{body}");
            i32::from(status != 200)
        }
        Err(error) => {
            eprintln!("ax: {shut} (127.0.0.1:{}: {error})", door.port);
            2
        }
    }
}
