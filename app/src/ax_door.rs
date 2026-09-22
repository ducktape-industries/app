//! The test door (#114): a loopback HTTP door over the SAME AccessKit tree
//! GPUI hands the OS, so a QA runner reads and drives the app the way a
//! screen reader does. There is no second tree: every answer is read off
//! `Window::a11y_tree`, every act goes through the adapter's own action path.
//!
//! Off unless the app is launched with `DUCKTAPE_AX_DOOR=<port|0>` (0 picks a
//! free port). It binds 127.0.0.1 only — `http::bind` takes a port, never a host
//! — and writes `{port, token}` to `http::door_file` mode 0600; a request without
//! that token is refused. `ducktape-app ax …` ([`cli`]) is its client.
//!
//! Ids are `<window>:<element id>`: a native node's own string ElementId, a
//! view node's `<module>/<wire key>`, widened with its ancestors' ids
//! (`rail.view:chat`) only while another node in the window shares it. Entity,
//! focus-handle and the kit's type-path segments never count: they change per
//! run or say nothing. Never an AccessKit NodeId, never a position. A password field's value and a node marked
//! [`crate::a11y::AX_PRIVATE`] (the recovery-phrase words) are
//! masked here, before anything leaves the process. Only a rig that also sets
//! `DUCKTAPE_AX_DOOR_PRIVATE=1` may ask for one private node's text ([`reveal`]).
//! A keyboard-only walk sends keys through the window's own key dispatch
//! ([`press_keys`]) and reads the bindings it can reach ([`shortcuts`]); a
//! pointer drag goes through its mouse dispatch ([`drag_by_id`]).
//! Every read draws the window it reads, and every answer carries
//! `X-Ax-Revision` ([`Seen`]): unchanged while the trees it read are.
use futures::StreamExt as _;
use gpui_kit::accesskit::{
    Action, ActionData, ActionRequest, NodeId, Role, Toggled, TreeId, TreeUpdate,
};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, ElementId, Window};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod actions;
mod http;
mod tree;

use actions::{drag_by_id, perform_by_id, press_keys, read, reveal, shortcuts};
pub(crate) use http::{cli, open};
pub(crate) use tree::{AxNode, VIEW_MARK, snapshot};
use tree::{compact, delta, nearest, offers};

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
    fn step(&self, nodes: &[AxNode], expired: bool) -> Option<Reply> {
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

/// `POST /key`: what a keyboard sends to one window — `keys`, keystrokes as
/// GPUI parses them (`tab`, `shift-tab`, `enter`, `ctrl-k`, space-separated),
/// then `text`, one key per character, to whatever holds focus.
#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Key {
    #[serde(default)]
    keys: String,
    #[serde(default)]
    text: String,
    /// the window that gets them; else the one holding focus, else the first
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

/// `POST /drag`: what a mouse sends to one window — a left press at `from`,
/// `steps` moves with the button held, a release at `to`. Logical px; with
/// `id`, local to that node's painted bounds, else window coordinates.
#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Drag {
    #[serde(default)]
    id: Option<String>,
    from: [f32; 2],
    to: [f32; 2],
    /// moves between press and release, 4 unless given, never 0
    #[serde(default)]
    steps: Option<u32>,
    /// without `id`: the window that gets it; else as `/key` picks one
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

impl Drag {
    /// The move count once the request is checked: 400 for a coordinate
    /// that is not a finite position on the window, or zero steps.
    fn checked(&self) -> Result<u32, Reply> {
        let refuse = |error: &str| Reply::new(400, json!({ "error": error }));
        if self
            .from
            .iter()
            .chain(&self.to)
            .any(|edge| !edge.is_finite() || *edge < 0.)
        {
            return Err(refuse("coordinates are finite logical px, 0 or more"));
        }
        match self.steps {
            Some(0) => Err(refuse("steps is 1 or more")),
            Some(steps) => Ok(steps),
            None => Ok(4),
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
    Reveal(Reveal),
    Key(Key),
    Keys(Filter),
    Drag(Drag),
}

#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Reveal {
    id: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Reply {
    status: u16,
    body: String,
    /// `X-Ax-Revision`
    revision: Option<u64>,
}

impl Reply {
    fn new(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            body: body.to_string(),
            revision: None,
        }
    }

    fn ok(body: serde_json::Value) -> Self {
        Self::new(200, body)
    }

    /// The reply, answered off the trees of `revision` ([`Seen`]).
    fn revised(self, revision: u64) -> Self {
        Self {
            revision: Some(revision),
            ..self
        }
    }
}

/// The tree the door last read of each window, and the revision: how many
/// reads found a window's tree unlike the one before, so a caller sees
/// whether the tree advanced between two answers.
#[derive(Default)]
pub(crate) struct Seen {
    trees: HashMap<String, TreeUpdate>,
    revision: u64,
}

impl Seen {
    fn saw(&mut self, name: &str, tree: &TreeUpdate) {
        if self.trees.get(name) != Some(tree) {
            self.trees.insert(name.to_owned(), tree.clone());
            self.revision += 1;
        }
    }
}

/// A request and where its answer goes.
pub(crate) type Call = (Request, std::sync::mpsc::Sender<Reply>);

const POLL: Duration = Duration::from_millis(50);

/// Answers the door's calls on the app's thread until the door is gone.
/// `windows` lists the windows it serves, by name.
pub(crate) async fn serve(
    mut calls: futures::channel::mpsc::UnboundedReceiver<Call>,
    windows: impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    cx: &mut AsyncApp,
) {
    let mut seen = Seen::default();
    while let Some((request, reply)) = calls.next().await {
        let answer = answer(request, &windows, &mut seen, cx).await;
        let _ = reply.send(answer.revised(seen.revision));
    }
}

async fn answer(
    request: Request,
    windows: &impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    seen: &mut Seen,
    cx: &mut AsyncApp,
) -> Reply {
    let all = Filter::default();
    match request {
        Request::Tree {
            filter,
            compact: small,
            bounds,
        } => {
            let nodes = read(windows, &filter, bounds, seen, cx);
            match small {
                true => Reply::ok(json!(compact(&nodes))),
                false => Reply::ok(json!(nodes)),
            }
        }
        Request::Actions(filter) => {
            Reply::ok(json!(offers(&read(windows, &filter, false, seen, cx))))
        }
        Request::Act(act) => {
            let before = read(windows, &all, false, seen, cx);
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
            let after = settle(&before, act.deadline_ms, windows, seen, cx).await;
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Key(key) => {
            let before = read(windows, &all, false, seen, cx);
            let Some(handle) = keyboard_window(key.window.as_deref(), &before, windows, cx) else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let pressed = handle
                .update(cx, |_, window, cx| {
                    press_keys(window, cx, &key.keys, &key.text)
                })
                .unwrap_or_else(|_| Err("the window is gone".into()));
            if let Err(error) = pressed {
                return Reply::new(400, json!({ "error": error }));
            }
            let after = settle(&before, key.deadline_ms, windows, seen, cx).await;
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Drag(drag) => {
            if let Err(reply) = drag.checked() {
                return reply;
            }
            let before = read(windows, &all, false, seen, cx);
            let handle = match &drag.id {
                Some(id) => {
                    if !before.iter().any(|node| node.id == *id) {
                        return Reply::new(
                            404,
                            json!({ "error": "no such node", "nearest": nearest(id, &before) }),
                        );
                    }
                    keyboard_window(
                        id.split_once(':').map(|(name, _)| name),
                        &before,
                        windows,
                        cx,
                    )
                }
                None => keyboard_window(drag.window.as_deref(), &before, windows, cx),
            };
            let Some(handle) = handle else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let name = cx
                .update(|cx| windows(cx))
                .into_iter()
                .find_map(|(name, other)| (other == handle).then_some(name))
                .unwrap_or_default();
            let sent = handle
                .update(cx, |_, window, cx| drag_by_id(&name, window, cx, &drag))
                .unwrap_or_else(|_| Reply::new(404, json!({ "error": "no such window" })));
            if sent.status != 200 {
                return sent;
            }
            settle(&before, drag.deadline_ms, windows, seen, cx).await;
            sent
        }
        Request::Keys(filter) => {
            let before = read(windows, &all, false, seen, cx);
            let Some(handle) = keyboard_window(filter.window.as_deref(), &before, windows, cx)
            else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            handle
                .update(cx, |_, window, cx| Reply::ok(json!(shortcuts(window, cx))))
                .unwrap_or_else(|_| Reply::new(404, json!({ "error": "no such window" })))
        }
        Request::Wait(wait) => {
            let deadline = Instant::now() + Duration::from_millis(wait.deadline_ms.min(60_000));
            loop {
                let nodes = read(windows, &all, false, seen, cx);
                if let Some(reply) = wait.step(&nodes, Instant::now() >= deadline) {
                    return reply;
                }
                cx.background_executor().timer(POLL).await;
            }
        }
        Request::Reveal(Reveal { id }) => {
            // a window's tree is switched on by the read
            let _ = read(windows, &all, false, seen, cx);
            let name = id.split_once(':').map_or("", |(name, _)| name).to_owned();
            cx.update(|cx| windows(cx))
                .into_iter()
                .find_map(|(window, handle)| (window == name).then_some(handle))
                .and_then(|handle| {
                    handle
                        .update(cx, |_, window, _| reveal(&name, window, &id))
                        .ok()
                })
                .unwrap_or_else(|| Reply::new(404, json!({ "error": "no such window" })))
        }
    }
}

/// The next settled frame after an input: the tree unchanged for three polls
/// (or `deadline_ms`, 2 s unless given).
async fn settle(
    before: &[AxNode],
    deadline_ms: Option<u64>,
    windows: &impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    seen: &mut Seen,
    cx: &mut AsyncApp,
) -> Vec<AxNode> {
    let deadline = Instant::now() + Duration::from_millis(deadline_ms.unwrap_or(2000));
    let mut after = before.to_vec();
    let mut last = serde_json::to_string(&after).unwrap_or_default();
    let mut quiet = 0;
    while quiet < 3 && Instant::now() < deadline {
        cx.background_executor().timer(POLL).await;
        let now = read(windows, &Filter::default(), false, seen, cx);
        let key = serde_json::to_string(&now).unwrap_or_default();
        match key == last {
            true => quiet += 1,
            false => (quiet, last, after) = (0, key, now),
        }
    }
    after
}

/// The window a keyboard types into: `named`, else the one whose tree
/// holds focus, else the first served.
fn keyboard_window(
    named: Option<&str>,
    nodes: &[AxNode],
    windows: &impl Fn(&App) -> Vec<(String, AnyWindowHandle)>,
    cx: &mut AsyncApp,
) -> Option<AnyWindowHandle> {
    let list = cx.update(|cx| windows(cx));
    let find = |want: &str| {
        list.iter()
            .find_map(|(name, handle)| (name == want).then_some(*handle))
    };
    match named {
        Some(named) => find(named),
        None => nodes
            .iter()
            .find(|node| node.state.contains(&"focused"))
            .and_then(|node| find(node.scope.split('/').next().unwrap_or_default()))
            .or_else(|| list.first().map(|(_, handle)| *handle)),
    }
}
