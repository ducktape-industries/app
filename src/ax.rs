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
//! holding focus, else the first — with the Tab walk when `walk`; `launcher`
//! is the caller's word that the shell screen is not the desk (AX-018), and
//! the shell says which controls its Help lists with a chord (AX-114).
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
pub(crate) mod audit;
mod http;
mod tree;

use actions::{current, drag_by_id, perform_by_id, press_keys, read, reveal, shortcuts};
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
        self.keeps_scope(&node.scope)
    }

    /// A node's `in`, or the scope of the element the fork refused.
    fn keeps_scope(&self, scope: &str) -> bool {
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

/// One window the door serves: its door name (`console`, `console2`, …),
/// the shell's key for it, and its handle.
pub(crate) type Served = (String, crate::runtime::WindowKey, AnyWindowHandle);

const POLL: Duration = Duration::from_millis(50);

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
/// `windows` lists the windows it serves, by name.
pub(crate) async fn serve(
    mut calls: futures::channel::mpsc::UnboundedReceiver<Call>,
    windows: impl Fn(&App) -> Vec<Served>,
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
    windows: &impl Fn(&App) -> Vec<Served>,
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
                .find_map(|(window, _, handle)| (window == name).then_some(handle));
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
                .find_map(|(name, _, other)| (other == handle).then_some(name))
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
        Request::Audit {
            filter,
            walk,
            launcher,
        } => {
            let before = read(windows, &all, false, seen, cx);
            let Some(handle) = keyboard_window(filter.window.as_deref(), &before, windows, cx)
            else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let name = cx
                .update(|cx| windows(cx))
                .into_iter()
                .find_map(|(name, _, other)| (other == handle).then_some(name))
                .unwrap_or_default();
            handle
                .update(cx, |_, window, cx| {
                    let mut reading = audit::observe(
                        window,
                        cx,
                        &name,
                        walk,
                        |scope| filter.keeps_scope(scope),
                        |window, cx| current(&name, window, cx, true, seen),
                    );
                    reading.chords = crate::shell::chords();
                    Reply::ok(json!(audit::audit(&reading, launcher)))
                })
                .unwrap_or_else(|_| Reply::new(404, json!({ "error": "no such window" })))
        }
        Request::Wait(wait) => {
            let deadline = deadline(wait.deadline_ms);
            loop {
                let nodes = read(windows, &all, false, seen, cx);
                if let Some(reply) = wait.step(&nodes, Instant::now() >= deadline) {
                    return reply;
                }
                cx.background_executor().timer(POLL).await;
            }
        }
        Request::Perf { by_instance } => {
            let served = cx.update(|cx| windows(cx));
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
            let _ = read(windows, &all, false, seen, cx);
            let name = id.split_once(':').map_or("", |(name, _)| name).to_owned();
            cx.update(|cx| windows(cx))
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
    /// The window's a11y tree is on: its guest views draw uncached.
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
/// loudly instead of passing on empty data; `cache_on` says whether every
/// served window still draws its guest views cached.
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
    windows: &impl Fn(&App) -> Vec<Served>,
    seen: &mut Seen,
    cx: &mut AsyncApp,
) -> Vec<AxNode> {
    let deadline = deadline(deadline_ms.unwrap_or(2000));
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
    windows: &impl Fn(&App) -> Vec<Served>,
    cx: &mut AsyncApp,
) -> Option<AnyWindowHandle> {
    let list = cx.update(|cx| windows(cx));
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
}
