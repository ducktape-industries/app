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
//! Ids are `<window>:<path>`: a native node's own string ElementId
//! (`console:network-switcher`), a view node's `<module>/<wire key>`
//! (`console:chat/send`), widened with its ancestors' ids (`console:row.send`)
//! only while another node in the window shares it, and `~2`, `~3` … when
//! even the whole path is shared ([`tree::door_ids`]). A synthetic child with
//! no element of its own ends in its role, and after the first of its role
//! under one node in its place among them (`Link2`: a RichText's second
//! range). Entity, focus-handle and the kit's
//! type-path segments never count: they change per run or say nothing. Never
//! an AccessKit NodeId, never a position. A password field's value and a node
//! marked [`crate::a11y::AX_PRIVATE`] (the recovery-phrase words) are masked
//! here, before anything leaves the process. Only a rig that also sets
//! `DUCKTAPE_AX_DOOR_PRIVATE=1` may ask for one private node's text ([`reveal`]).
//! A keyboard-only walk sends keys through the window's own key dispatch
//! ([`press_keys`]) and reads the bindings it can reach ([`shortcuts`]); a
//! pointer drag goes through its mouse dispatch ([`drag_by_id`]).
//! `GET /audit?window&view&walk=1&launcher=1` runs the rules of
//! `docs/ax.md` ([`audit`]) over one window — the named one, else the one
//! holding focus, else the first — with the Tab walk when `walk`, a key per
//! update as `POST /key` presses it; `launcher` is the caller's word that
//! the shell screen is not the desk (AX-018), and the shell says which
//! controls its Help lists with a chord (AX-114).
//! Every read draws the window it reads, and every answer carries
//! `X-Ax-Revision` ([`Seen`]): unchanged while the trees it read are.
//!
//! Compiled with the `ax-door` feature (the kit and qa builds) and nowhere
//! else: a release build has no door to open and no client for one. The
//! tests compile it without the feature for the trees it reads, so its
//! server half has no caller there.
use futures::StreamExt as _;
use gpui_kit::accesskit::{
    Action, ActionData, ActionRequest, NodeId, Role, Toggled, TreeId, TreeUpdate,
};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, ElementId, Window};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod actions;
pub(crate) mod audit;
mod http;
mod tree;

use actions::{current, drag_by_id, perform_by_id, press_keys, read, reveal, shortcuts};
#[cfg(feature = "ax-door")]
pub(crate) use http::{cli, open};
pub(crate) use tree::{AxNode, snapshot};
use tree::{compact, delta, nearest, offers};

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Filter {
    window: Option<String>,
    view: Option<String>,
}

impl Filter {
    /// A node's `in`, or the scope of the element the fork refused.
    fn keeps(&self, scope: &str) -> bool {
        let (window, view) = match scope.split_once('/') {
            Some((window, view)) => (window, Some(view)),
            None => (scope, None),
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
///
/// With `"delta": false` the press reads nothing (docs/perf.md §4.3): no
/// window's tree before or after, so it neither switches a11y on nor draws,
/// and the answer is `{}` instead of the delta (`deadline_ms`, which bounds
/// the settle, goes unused). `window` then resolves from the door's window
/// list alone, without a tree to find the focused node in: the named
/// window, else the first one served.
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
    /// answer with the tree's delta, as ever; false presses without reading
    #[serde(default = "yes")]
    delta: bool,
}

fn yes() -> bool {
    true
}

/// Most characters in `keys` or `text` (`POST /key`) or `value` (`POST
/// /act`): the door dispatches a key for each, all in one update.
const MAX_TEXT: usize = 4_096;

/// Most moves in one `POST /drag`, all dispatched in one update.
const MAX_STEPS: u32 = 1_000;

/// `POST /drag`: what a mouse sends to one window — a left press at `from`,
/// `steps` moves with the button held, a release at `to`. Logical px; with
/// `id`, local to that node's painted bounds, else window coordinates.
#[derive(Debug, Deserialize, PartialEq)]
pub(crate) struct Drag {
    #[serde(default)]
    id: Option<String>,
    from: [f32; 2],
    to: [f32; 2],
    /// moves between press and release, 4 unless given, never 0, never
    /// more than [`MAX_STEPS`]
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
            Some(steps) if steps > MAX_STEPS => {
                Err(refuse(&format!("steps is {MAX_STEPS} or fewer")))
            }
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
    Audit {
        filter: Filter,
        walk: bool,
        launcher: bool,
    },
    /// `GET /perf[?by=instance]`: the perf registry (docs/perf.md), by
    /// module unless asked by instance. Answered without a window read.
    Perf {
        by_instance: bool,
    },
    /// `POST /perf/reset`: the registry's counters back to zero.
    PerfReset,
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
    #[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
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

/// One window the door serves: its door name (`console`, `console2`, …,
/// kept for the window's life: `Windows::served`), the shell's key for it,
/// and its handle.
pub(crate) type Served = (String, crate::runtime::WindowKey, AnyWindowHandle);

/// What the door serves: the windows, by name, and the seats behind them.
pub(crate) struct Door<'a> {
    /// The windows it serves, by name.
    pub(crate) windows: &'a dyn Fn(&App) -> Vec<Served>,
    /// Turns every seat, called before each read: a reply the guest has
    /// answered is in the tree the read draws, as the draw itself took it
    /// in before the seat left the draw path (`Seats::settle`).
    pub(crate) settle: &'a dyn Fn(&mut App),
}

const POLL: Duration = Duration::from_millis(50);

/// Longest the audit's arrow probe waits for a view to move its active
/// row after one key: a composite whose arrows do nothing costs it once
/// for each pair it tries and once more (`audit::arrow_pairs`).
const ARROW_WAIT: Duration = Duration::from_millis(500);

/// Longest a call may wait on the tree. The client picks its deadline, so
/// an absurd one is honoured only this far.
const MAX_DEADLINE: Duration = Duration::from_secs(60);

/// A call's deadline, `ms` from now, capped at [`MAX_DEADLINE`].
fn deadline(ms: u64) -> Instant {
    Instant::now() + bounded(ms)
}

/// `ms` as a Duration, no longer than [`MAX_DEADLINE`].
fn bounded(ms: u64) -> Duration {
    Duration::from_millis(ms).min(MAX_DEADLINE)
}

/// Answers the door's calls on the app's thread until the door is gone.
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
pub(crate) async fn serve(
    mut calls: futures::channel::mpsc::UnboundedReceiver<Call>,
    door: Door<'_>,
    cx: &mut AsyncApp,
) {
    let mut seen = Seen::default();
    while let Some((request, reply)) = calls.next().await {
        let answer = answer(request, &door, &mut seen, cx).await;
        let _ = reply.send(answer.revised(seen.revision));
    }
}

async fn answer(request: Request, door: &Door<'_>, seen: &mut Seen, cx: &mut AsyncApp) -> Reply {
    let all = Filter::default();
    match request {
        Request::Tree {
            filter,
            compact: small,
            bounds,
        } => {
            let nodes = read(door, &filter, bounds, seen, cx);
            match small {
                true => Reply::ok(json!(compact(&nodes))),
                false => Reply::ok(json!(nodes)),
            }
        }
        Request::Actions(filter) => Reply::ok(json!(offers(&read(door, &filter, false, seen, cx)))),
        Request::Act(act) => {
            let before = read(door, &all, false, seen, cx);
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
                .update(|cx| (door.windows)(cx))
                .into_iter()
                .find_map(|(window, _, handle)| (window == name).then_some(handle));
            let value = act.value.unwrap_or_default();
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, cx| {
                    perform_by_id(&name, window, cx, &act.id, &act.action, &value)
                });
            }
            let after = settle(&before, act.deadline_ms, door, seen, cx).await;
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Key(key) => {
            // no tree to read: the window comes from the list alone
            let before = match key.delta {
                true => read(door, &all, false, seen, cx),
                false => Vec::new(),
            };
            let Some(handle) = keyboard_window(key.window.as_deref(), &before, door, cx) else {
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
            if !key.delta {
                return Reply::ok(json!({}));
            }
            let after = settle(&before, key.deadline_ms, door, seen, cx).await;
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Drag(drag) => {
            if let Err(reply) = drag.checked() {
                return reply;
            }
            let before = read(door, &all, false, seen, cx);
            let handle = match &drag.id {
                Some(id) => {
                    if !before.iter().any(|node| node.id == *id) {
                        return Reply::new(
                            404,
                            json!({ "error": "no such node", "nearest": nearest(id, &before) }),
                        );
                    }
                    keyboard_window(id.split_once(':').map(|(name, _)| name), &before, door, cx)
                }
                None => keyboard_window(drag.window.as_deref(), &before, door, cx),
            };
            let Some(handle) = handle else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let name = cx
                .update(|cx| (door.windows)(cx))
                .into_iter()
                .find_map(|(name, _, other)| (other == handle).then_some(name))
                .unwrap_or_default();
            let sent = handle
                .update(cx, |_, window, cx| drag_by_id(&name, window, cx, &drag))
                .unwrap_or_else(|_| Reply::new(404, json!({ "error": "no such window" })));
            if sent.status != 200 {
                return sent;
            }
            settle(&before, drag.deadline_ms, door, seen, cx).await;
            sent
        }
        Request::Keys(filter) => {
            let before = read(door, &all, false, seen, cx);
            let Some(handle) = keyboard_window(filter.window.as_deref(), &before, door, cx) else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            handle
                .update(cx, |_, window, cx| Reply::ok(json!(shortcuts(window, cx))))
                .unwrap_or_else(|_| Reply::new(404, json!({ "error": "no such window" })))
        }
        Request::Audit {
            filter,
            walk,
            launcher,
        } => {
            let before = read(door, &all, false, seen, cx);
            let Some(handle) = keyboard_window(filter.window.as_deref(), &before, door, cx) else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let name = cx
                .update(|cx| (door.windows)(cx))
                .into_iter()
                .find_map(|(name, _, other)| (other == handle).then_some(name))
                .unwrap_or_default();
            let gone = || Reply::new(404, json!({ "error": "no such window" }));
            let opened = handle.update(cx, |_, window, cx| {
                // the probe's arrows pick Settings' choices, and a pick
                // saves: what they change goes back once the walk ends,
                // however it ends
                let kept = walk.then(|| crate::shell::Kept::of(window, cx)).flatten();
                let observer = audit::Observer::open(
                    window,
                    cx,
                    &name,
                    walk,
                    |scope| filter.keeps(scope),
                    |window, cx| current(&name, window, cx, true, seen),
                );
                (kept, observer)
            });
            let Ok((kept, mut observer)) = opened else {
                return gone();
            };
            let reply = async {
                // a key per update, as `POST /key` presses it: a view hears
                // a key once the update that pressed it ends, and answers
                // on the draw a read makes; the probe's arrows wait for
                // that answer
                loop {
                    match handle.update(cx, |_, window, cx| observer.press(window, cx)) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(_) => return gone(),
                    }
                    let deadline = Instant::now() + ARROW_WAIT;
                    loop {
                        cx.update(|cx| (door.settle)(cx));
                        let moved = handle
                            .update(cx, |_, window, cx| observer.read(window, cx))
                            .unwrap_or(true);
                        if moved || Instant::now() >= deadline {
                            break;
                        }
                        cx.background_executor().timer(POLL).await;
                    }
                }
                cx.update(|cx| (door.settle)(cx));
                handle
                    .update(cx, |_, window, cx| {
                        let mut reading = observer.finish(window, cx);
                        reading.chords = crate::shell::chords();
                        Reply::ok(json!(audit::audit(&reading, launcher)))
                    })
                    .unwrap_or_else(|_| gone())
            }
            .await;
            if let Some(kept) = kept {
                cx.update(|cx| kept.restore(cx));
            }
            reply
        }
        Request::Wait(wait) => {
            let deadline = deadline(wait.deadline_ms);
            loop {
                let nodes = read(door, &all, false, seen, cx);
                if let Some(reply) = wait.step(&nodes, Instant::now() >= deadline) {
                    return reply;
                }
                cx.background_executor().timer(POLL).await;
            }
        }
        Request::Perf { by_instance } => {
            let served = cx.update(|cx| (door.windows)(cx));
            let mut windows = Vec::with_capacity(served.len());
            for (name, key, handle) in served {
                // a11y read, never a draw: the cached path must stay cached
                let read = handle.update(cx, |_, window, _| {
                    (window.is_a11y_active(), gpui_perf(window))
                });
                let Ok((a11y, gpui)) = read else {
                    continue;
                };
                windows.push(ServedPerf {
                    name,
                    key,
                    a11y,
                    gpui,
                });
            }
            perf_reply(by_instance, &windows)
        }
        Request::PerfReset => perf_reset_reply(),
        Request::Reveal(Reveal { id }) => {
            // a window's tree is switched on by the read
            let _ = read(door, &all, false, seen, cx);
            let name = id.split_once(':').map_or("", |(name, _)| name).to_owned();
            cx.update(|cx| (door.windows)(cx))
                .into_iter()
                .find_map(|(window, _, handle)| (window == name).then_some(handle))
                .and_then(|handle| {
                    handle
                        .update(cx, |_, window, _| reveal(&name, window, &id))
                        .ok()
                })
                .unwrap_or_else(|| Reply::new(404, json!({ "error": "no such window" })))
        }
    }
}

/// What `/perf` reads of one served window without drawing it.
struct ServedPerf {
    name: String,
    key: crate::runtime::WindowKey,
    /// The window's a11y tree is on: a door read has drawn in it.
    a11y: bool,
    /// gpui's own histograms, with the `perf-deep` feature; else `Null`.
    gpui: serde_json::Value,
}

fn perf_off() -> Reply {
    Reply::new(
        409,
        json!({ "error": "perf is off: launch with DUCKTAPE_PERF=1" }),
    )
}

/// `POST /perf/reset`: the counters and samples back to zero; 409 while off.
fn perf_reset_reply() -> Reply {
    if !crate::perf::on() {
        return perf_off();
    }
    crate::perf::reset();
    Reply::ok(json!({ "reset": true }))
}

/// The registry as the door gives it: 409 while off, so a gate fails
/// loudly instead of passing on empty data; `cache_on` says no served
/// window's tree is on, so no door read drew in any (the layers draw
/// cached either way).
fn perf_reply(by_instance: bool, windows: &[ServedPerf]) -> Reply {
    if !crate::perf::on() {
        return perf_off();
    }
    let mut snapshot = crate::perf::snapshot(by_instance);
    snapshot["cache_on"] = json!(windows.iter().all(|window| !window.a11y));
    for window in windows {
        let entry = &mut snapshot["windows"][window.key.0.to_string()];
        if entry.is_null() {
            *entry = json!({});
        }
        entry["name"] = json!(window.name);
        if !window.gpui.is_null() {
            entry["gpui"] = window.gpui.clone();
        }
    }
    Reply::ok(snapshot)
}

/// gpui's frame and input histograms for one window, `perf-deep` only: the
/// end-to-end frame time (dirty to present), the draw, the present
/// interval and the input-to-frame latency the registry cannot see. They
/// cover the window's life: `POST /perf/reset` does not reach them.
#[cfg(feature = "perf-deep")]
fn gpui_perf(window: &Window) -> serde_json::Value {
    // the histograms hold nanoseconds; gpui does not re-export their type
    macro_rules! us {
        ($histogram:expr) => {
            json!({
                "n": $histogram.len(),
                "p50": $histogram.value_at_quantile(0.5) / 1000,
                "p95": $histogram.value_at_quantile(0.95) / 1000,
                "max": $histogram.max() / 1000,
            })
        };
    }
    let frames = window.frame_duration_snapshot();
    let input = window.input_latency_snapshot();
    json!({
        "us": {
            "dirty_to_present": us!(frames.dirty_to_present_histogram),
            "draw": us!(frames.draw_duration_histogram),
            "present_interval": us!(frames.present_interval_histogram),
            "input_latency": us!(input.latency_histogram),
        },
    })
}

#[cfg(not(feature = "perf-deep"))]
fn gpui_perf(_: &Window) -> serde_json::Value {
    serde_json::Value::Null
}

/// The next settled frame after an input: the tree unchanged for three polls
/// (or `deadline_ms`, 2 s unless given).
async fn settle(
    before: &[AxNode],
    deadline_ms: Option<u64>,
    door: &Door<'_>,
    seen: &mut Seen,
    cx: &mut AsyncApp,
) -> Vec<AxNode> {
    let deadline = deadline(deadline_ms.unwrap_or(2000));
    let mut after = before.to_vec();
    let mut last = serde_json::to_string(&after).unwrap_or_default();
    let mut quiet = 0;
    while quiet < 3 && Instant::now() < deadline {
        cx.background_executor().timer(POLL).await;
        let now = read(door, &Filter::default(), false, seen, cx);
        let key = serde_json::to_string(&now).unwrap_or_default();
        match key == last {
            true => quiet += 1,
            false => (quiet, last, after) = (0, key, now),
        }
    }
    after
}

/// The window a keyboard types into: `named`, else the one whose tree
/// holds focus, else the first served. With no `nodes` (a press that reads
/// no tree, `POST /key` with `"delta": false`) there is no focus to find,
/// so the unnamed window is the first served.
fn keyboard_window(
    named: Option<&str>,
    nodes: &[AxNode],
    door: &Door<'_>,
    cx: &mut AsyncApp,
) -> Option<AnyWindowHandle> {
    let list = cx.update(|cx| (door.windows)(cx));
    let find = |want: &str| {
        list.iter()
            .find_map(|(name, _, handle)| (name == want).then_some(*handle))
    };
    match named {
        Some(named) => find(named),
        None => nodes
            .iter()
            .find(|node| node.state.contains(&"focused"))
            .and_then(|node| find(node.scope.split('/').next().unwrap_or_default()))
            .or_else(|| list.first().map(|(_, _, handle)| *handle)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client names the deadline; u64::MAX must not leave a call waiting
    /// past the cap.
    #[test]
    fn a_deadline_is_capped() {
        assert_eq!(bounded(u64::MAX), MAX_DEADLINE);
        assert_eq!(bounded(2000), Duration::from_secs(2));
    }

    /// A drag of more than [`MAX_STEPS`] moves is 400, not an update that
    /// dispatches them all.
    #[test]
    fn drag_steps_are_capped() {
        let drag = |steps| Drag {
            id: None,
            from: [0., 0.],
            to: [10., 10.],
            steps: Some(steps),
            window: None,
            deadline_ms: None,
        };
        assert_eq!(drag(MAX_STEPS).checked(), Ok(MAX_STEPS));
        assert_eq!(
            drag(MAX_STEPS + 1).checked().map_err(|reply| reply.status),
            Err(400)
        );
        assert_eq!(
            drag(u32::MAX).checked().map_err(|reply| reply.status),
            Err(400)
        );
    }

    /// A served window under a key no window of a test running beside
    /// this one can have.
    fn served(name: &str, a11y: bool) -> ServedPerf {
        ServedPerf {
            name: name.to_owned(),
            key: crate::runtime::WindowKey::unique(),
            a11y,
            gpui: serde_json::Value::Null,
        }
    }

    /// Off, `/perf` is a 409 rather than empty numbers; on, the registry as
    /// JSON with each served window named and `cache_on` from the a11y state.
    #[test]
    fn perf_is_refused_off_and_json_on() {
        let off = {
            let _off = crate::perf::off_for_test();
            perf_reply(false, &[])
        };
        assert_eq!(off.status, 409);
        assert!(off.body.contains("DUCKTAPE_PERF=1"));

        let _on = crate::perf::on_for_test();
        let (console, console2) = (served("console", false), served("console2", true));
        let (first, second) = (console.key.0.to_string(), console2.key.0.to_string());
        crate::perf::count(crate::perf::Key::Window(console.key), "renders", 3);
        let reply = perf_reply(false, &[console, console2]);
        assert_eq!(reply.status, 200);
        let body: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(body["on"], true);
        assert_eq!(body["cache_on"], false, "one served window has a11y on");
        assert_eq!(body["windows"][&first]["name"], "console");
        assert_eq!(body["windows"][&first]["renders"], 3);
        assert_eq!(body["windows"][&second]["name"], "console2");
        let cached = perf_reply(false, &[served("console", false)]);
        let body: serde_json::Value = serde_json::from_str(&cached.body).unwrap();
        assert_eq!(body["cache_on"], true);
    }

    /// A reset is refused off; on, it zeroes what `/perf` then shows.
    #[test]
    fn perf_reset_is_refused_off_and_clears_on() {
        let off = {
            let _off = crate::perf::off_for_test();
            perf_reset_reply()
        };
        assert_eq!(off.status, 409);

        let _on = crate::perf::on_for_test();
        let key = crate::runtime::WindowKey::unique();
        crate::perf::count(crate::perf::Key::Window(key), "renders", 3);
        let reset = perf_reset_reply();
        assert_eq!(
            (reset.status, reset.body.as_str()),
            (200, r#"{"reset":true}"#)
        );
        let body: serde_json::Value = serde_json::from_str(&perf_reply(false, &[]).body).unwrap();
        assert!(
            body["windows"][&key.0.to_string()].is_null(),
            "the count was cleared"
        );
    }

    /// The door's answer to `request`, from a fresh window it serves as
    /// `console`, and whether that window's a11y is on afterwards.
    async fn served_answer(
        request: impl FnOnce() -> Request + 'static,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Reply, Reply, bool) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(400.), gpui_kit::px(300.)),
            |_, _| crate::render::ViewTree::new(view_wire::Node::empty()),
        );
        let handle: AnyWindowHandle = window.into();
        let key = crate::runtime::WindowKey::unique();
        let served = move |_: &App| vec![("console".to_owned(), key, handle)];
        let (answered, perf) = cx
            .spawn(async move |mut cx| {
                let door = Door {
                    windows: &served,
                    settle: &|_| {},
                };
                let mut seen = Seen::default();
                let answered = answer(request(), &door, &mut seen, &mut cx).await;
                let perf = answer(
                    Request::Perf { by_instance: false },
                    &door,
                    &mut seen,
                    &mut cx,
                )
                .await;
                (answered, perf)
            })
            .await;
        let active = window
            .update(cx, |_, window, _| window.is_a11y_active())
            .unwrap();
        (answered, perf, active)
    }

    /// A key press asked to read nothing still reaches the window, and
    /// leaves its a11y off, so no read drew in it and `/perf` says
    /// `cache_on`; the answer is `{}`, and an unnamed window is the first
    /// served. Any read (here a tree) switches a11y on and `cache_on` off:
    /// the check can tell them apart.
    #[gpui_kit::test]
    async fn a_press_without_a_read_keeps_the_cache_on(cx: &mut gpui_kit::TestAppContext) {
        let _on = crate::perf::on_for_test();
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let _heard = cx.update(|cx| {
            let heard = heard.clone();
            cx.observe_keystrokes(move |event, _, _| {
                heard.borrow_mut().push(event.keystroke.key.clone())
            })
        });
        let press = || {
            Request::Key(Key {
                keys: "tab".into(),
                text: String::new(),
                window: None,
                deadline_ms: None,
                delta: false,
            })
        };
        let (answered, perf, active) = served_answer(press, cx).await;
        assert_eq!((answered.status, answered.body.as_str()), (200, "{}"));
        assert_eq!(*heard.borrow(), ["tab"], "the press reached the window");
        assert!(
            !active,
            "a press that reads nothing does not switch a11y on"
        );
        let perf: serde_json::Value = serde_json::from_str(&perf.body).unwrap();
        assert_eq!(perf["cache_on"], true);

        let read = || Request::Tree {
            filter: Filter::default(),
            compact: true,
            bounds: false,
        };
        let (_, perf, active) = served_answer(read, cx).await;
        assert!(active, "a tree read switches a11y on");
        let perf: serde_json::Value = serde_json::from_str(&perf.body).unwrap();
        assert_eq!(perf["cache_on"], false);
    }

    /// A read turns the seats first: what a guest has answered since its
    /// last turn is in the tree the read draws, as the draw itself took it
    /// in while the guest was stepped on the draw path. Here the seat's
    /// guest holds an intent its next turn hands over; the read delivers
    /// it, the same request with the seats left alone does not.
    #[gpui_kit::test]
    async fn a_read_turns_the_seats_first(cx: &mut gpui_kit::TestAppContext) {
        const MODULE: &str = "door-read-turns-view";
        crate::runtime::seat_for_test(MODULE, 200);
        cx.update(gpui_kit::init);
        let seat = cx.update(|cx| {
            use gpui_kit::AppContext as _;
            cx.new(|cx| crate::runtime::Seat::new(MODULE, cx))
        });
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let _heard = cx.update(|cx| {
            let heard = heard.clone();
            cx.subscribe(&seat, move |_, intent: &crate::runtime::Intent, _| {
                heard.borrow_mut().push(intent.clone())
            })
        });
        cx.run_until_parked();
        let instance = seat.read_with(cx, |seat, _| seat.instance());
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(400.), gpui_kit::px(300.)),
            |_, _| crate::render::ViewTree::new(view_wire::Node::empty()),
        );
        let handle: AnyWindowHandle = window.into();
        let key = crate::runtime::WindowKey::unique();
        let served = move |_: &App| vec![("console".to_owned(), key, handle)];
        let read = || Request::Tree {
            filter: Filter::default(),
            compact: true,
            bounds: false,
        };
        for (settles, expected) in [(false, false), (true, true)] {
            crate::runtime::intent_for_test(MODULE, instance, crate::runtime::Intent::Badge(3));
            heard.borrow_mut().clear();
            let turning = seat.clone();
            cx.spawn(async move |mut cx| {
                let settle = move |cx: &mut App| turning.update(cx, |seat, cx| seat.turn(cx));
                let quiet = |_: &mut App| {};
                let door = Door {
                    windows: &served,
                    settle: if settles { &settle } else { &quiet },
                };
                answer(read(), &door, &mut Seen::default(), &mut cx).await
            })
            .await;
            assert_eq!(
                heard.borrow().contains(&crate::runtime::Intent::Badge(3)),
                expected,
                "settles={settles}: the read {} the seat",
                if settles { "turned" } else { "left" }
            );
        }
    }
}
