//! The loopback HTTP accessibility door reads trees and queues actions through WindowA11y.
//! QA clients use it to inspect and drive the app.
use futures::StreamExt as _;
use gpui_kit::accesskit::{
    Action, ActionData, ActionRequest, NodeId, Role, Toggled, TreeId, TreeUpdate,
};
use gpui_kit::{AnyWindowHandle, App, AsyncApp, Window};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub use ducktape_a11y_platform::WindowA11y;
#[cfg(any(test, feature = "test-support"))]
pub use ducktape_a11y_platform::headless::Harness;

mod actions;
pub mod audit;
mod http;
pub mod inspect;
pub mod natives;
pub mod shown;
mod tree;

use actions::{current, drag_by_id, perform, press_keys, read, reveal, shortcuts, type_text};
#[cfg(feature = "ax-door")]
pub use http::{cli, open, open_at};
pub use tree::{AxNode, modal_active, snapshot};
use tree::{compact, delta, nearest, offers};

#[derive(Debug, Default, PartialEq)]
pub struct Filter {
    window: Option<String>,
    view: Option<String>,
}

impl Filter {
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
pub struct Act {
    id: String,
    action: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Wait {
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
pub struct Key {
    #[serde(default)]
    keys: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
    #[serde(default = "yes")]
    delta: bool,
}

fn yes() -> bool {
    true
}

/// `POST /drag`: what a mouse sends to one window — a left press at `from`,
/// `steps` moves with the button held, a release at `to`. Logical px; with
/// `id`, local to that node's painted bounds, else window coordinates.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Drag {
    #[serde(default)]
    id: Option<String>,
    from: [f32; 2],
    to: [f32; 2],
    #[serde(default)]
    steps: Option<u32>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

impl Drag {
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
pub enum Request {
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
    /// program unless asked by instance. Answered without a window read.
    Perf {
        by_instance: bool,
    },
    /// `POST /perf/reset`: the registry's counters back to zero.
    PerfReset,
    /// `POST /motion {"reduce": bool}`: the system's ask for less motion,
    /// set as the OS sets it (`App::set_reduce_motion`), so a walk can hold
    /// the designed animations still and put them back. Once set, gpui no
    /// longer follows the OS's own ask.
    Motion(Motion),
    /// `/inspect/*`: the views' own trees, not the accessibility tree
    /// (`inspect`).
    Inspect(inspect::Ask),
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Motion {
    reduce: bool,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Reveal {
    id: String,
}

#[derive(Debug, PartialEq)]
pub struct Reply {
    status: u16,
    body: String,
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
pub struct Seen {
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
pub type Call = (Request, std::sync::mpsc::Sender<Reply>);

/// One window the door serves: its door name (`console`, `console2`, …,
/// kept for the window's life: `Windows::served`), the shell's key for it,
/// and its handle.
pub type Served = (String, crate::runtime::WindowKey, AnyWindowHandle);

/// What a walk puts back once it ends (`Door::keep`).
pub type Restore = Box<dyn FnOnce(&mut App)>;

pub type Stopped = (AnyWindowHandle, &'static str, String);

/// What the door serves: the windows, by name, and the seats behind them.
pub struct Door<'a> {
    /// The windows it serves, by name.
    pub windows: &'a dyn Fn(&App) -> Vec<Served>,
    /// Turns every seat, called before each read: a reply the view has
    /// answered is in the tree the read draws, as the draw itself took it
    /// in before the seat left the draw path (`Seats::settle`). `false`
    /// while a seat's tick is in flight: the read parks until it lands.
    pub settle: &'a dyn Fn(&mut App) -> bool,
    /// What the walk keeps before its arrows pick a radio group's choices
    /// (a pick saves) and puts back after, however the walk ends: the
    /// shell's own (`Kept`); `None` for a window that is not the shell's.
    pub keep: &'a dyn Fn(&Window, &App) -> Option<Restore>,
    /// The shell's command chords, `(control name, chord)`, for the audit.
    pub chords: &'a dyn Fn() -> Vec<(String, String)>,
    /// Every seated view the inspector reads: its window, program and
    /// tree (`Seats::seated`).
    pub views: &'a dyn Fn(&App) -> Vec<inspect::Seated>,
    pub reach: &'a dyn Fn(AnyWindowHandle) -> Option<Rc<WindowA11y>>,
    pub stopped: &'a dyn Fn(&App) -> Vec<Stopped>,
}

#[cfg(feature = "ax-door")]
thread_local! {
    static PLATFORM: std::cell::RefCell<Option<Rc<ducktape_a11y_platform::A11yPlatform>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "ax-door")]
pub fn wrap(platform: Rc<dyn gpui_kit::Platform>) -> Rc<dyn gpui_kit::Platform> {
    let wrapped = ducktape_a11y_platform::A11yPlatform::new(platform);
    PLATFORM.with(|kept| kept.replace(Some(wrapped.clone())));
    wrapped
}

#[cfg(feature = "ax-door")]
pub fn reach(window: AnyWindowHandle) -> Option<Rc<WindowA11y>> {
    PLATFORM.with(|kept| kept.borrow().as_ref()?.window(window))
}

const POLL: Duration = Duration::from_millis(50);

const ACTIVE_ROW_ARROW_PROBE_WAIT: Duration = Duration::from_millis(500);

fn deadline(ms: u64) -> Option<Instant> {
    Instant::now().checked_add(Duration::from_millis(ms))
}

fn passed(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

/// Answers the door's calls on the app's thread until the door is gone.
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
pub async fn serve(
    mut calls: futures::channel::mpsc::UnboundedReceiver<Call>,
    door: Door<'_>,
    cx: &mut AsyncApp,
) {
    cx.update(natives::serve);
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
            let nodes = read(door, &filter, bounds, None, seen, cx).await;
            match small {
                true => Reply::ok(json!(compact(&nodes))),
                false => Reply::ok(json!(nodes)),
            }
        }
        Request::Actions(filter) => Reply::ok(json!(offers(
            &read(door, &filter, false, None, seen, cx).await
        ))),
        Request::Act(act) => {
            let until = act.deadline_ms.and_then(deadline);
            let before = read(door, &all, false, until, seen, cx).await;
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
            let node = target.node;
            let reached = handle.and_then(|handle| Some((handle, (door.reach)(handle)?)));
            let Some((handle, a11y)) = reached else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let value = act.value.unwrap_or_default();
            let queued = perform(&a11y, node, &act.action, &value);
            if !queued {
                return Reply::new(
                    409,
                    json!({ "error": "the accessibility action was not queued" }),
                );
            }
            if act.action == "type" {
                settle(&before, (act.deadline_ms, until), door, seen, cx).await;
                let _ = handle.update(cx, |_, window, cx| type_text(window, cx, &value));
            }
            let after = settle(&before, (act.deadline_ms, until), door, seen, cx).await;
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Key(key) => {
            let until = key.deadline_ms.and_then(deadline);
            let before = match key.delta {
                true => read(door, &all, false, until, seen, cx).await,
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
            let after = settle(&before, (key.deadline_ms, until), door, seen, cx).await;
            Reply::ok(json!(delta(&before, &after)))
        }
        Request::Drag(drag) => {
            if let Err(reply) = drag.checked() {
                return reply;
            }
            let until = drag.deadline_ms.and_then(deadline);
            let before = read(door, &all, false, until, seen, cx).await;
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
            let a11y = (door.reach)(handle);
            let sent = handle
                .update(cx, |_, window, cx| {
                    drag_by_id(&name, window, cx, a11y.as_deref(), &drag)
                })
                .unwrap_or_else(|_| Reply::new(404, json!({ "error": "no such window" })));
            if sent.status != 200 {
                return sent;
            }
            settle(&before, (drag.deadline_ms, until), door, seen, cx).await;
            sent
        }
        Request::Keys(filter) => {
            let before = read(door, &all, false, None, seen, cx).await;
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
            let before = read(door, &all, false, None, seen, cx).await;
            let Some(handle) = keyboard_window(filter.window.as_deref(), &before, door, cx) else {
                return Reply::new(404, json!({ "error": "no such window" }));
            };
            let name = cx
                .update(|cx| (door.windows)(cx))
                .into_iter()
                .find_map(|(name, _, other)| (other == handle).then_some(name))
                .unwrap_or_default();
            let gone = || Reply::new(404, json!({ "error": "no such window" }));
            let Some(a11y) = (door.reach)(handle) else {
                return gone();
            };
            let modal = modal_active(a11y.tree().as_ref());
            let opened = handle.update(cx, |_, window, cx| {
                let kept = walk.then(|| (door.keep)(window, cx)).flatten();
                let observer = audit::Observer::open(
                    window,
                    cx,
                    walk,
                    modal,
                    |scope| filter.keeps(scope),
                    |window, cx| current(&name, window, cx, &a11y, true, seen),
                );
                (kept, observer)
            });
            let Ok((kept, mut observer)) = opened else {
                return gone();
            };
            let reply = async {
                loop {
                    match handle.update(cx, |_, window, cx| observer.press(window, cx)) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(_) => return gone(),
                    }
                    let deadline = Instant::now() + ACTIVE_ROW_ARROW_PROBE_WAIT;
                    loop {
                        settled(door, None, cx).await;
                        let moved = handle
                            .update(cx, |_, window, cx| observer.read(window, cx))
                            .unwrap_or(true);
                        if moved || Instant::now() >= deadline {
                            break;
                        }
                        cx.background_executor().timer(POLL).await;
                    }
                }
                settled(door, None, cx).await;
                handle
                    .update(cx, |_, window, cx| {
                        let mut reading = observer.finish(window, cx);
                        reading.chords = (door.chords)();
                        reading.refused =
                            audit::refused_in(&name, handle, (door.stopped)(cx), |scope| {
                                filter.keeps(scope)
                            });
                        Reply::ok(json!(audit::audit(&reading, launcher)))
                    })
                    .unwrap_or_else(|_| gone())
            }
            .await;
            if let Some(kept) = kept {
                cx.update(|cx| kept(cx));
            }
            reply
        }
        Request::Wait(wait) => {
            let deadline = deadline(wait.deadline_ms);
            loop {
                let nodes = read(door, &all, false, deadline, seen, cx).await;
                if let Some(reply) = wait.step(&nodes, passed(deadline)) {
                    return reply;
                }
                cx.background_executor().timer(POLL).await;
            }
        }
        Request::Perf { by_instance } => {
            let served = cx.update(|cx| (door.windows)(cx));
            let mut windows = Vec::with_capacity(served.len());
            for (name, key, handle) in served {
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
        Request::Inspect(ask) => inspect::answer(ask, door, cx).await,
        Request::Motion(Motion { reduce }) => {
            cx.update(|cx| cx.set_reduce_motion(reduce));
            Reply::ok(json!({}))
        }
        Request::Reveal(Reveal { id }) => {
            let _ = read(door, &all, false, None, seen, cx).await;
            let name = id.split_once(':').map_or("", |(name, _)| name).to_owned();
            cx.update(|cx| (door.windows)(cx))
                .into_iter()
                .find_map(|(window, _, handle)| (window == name).then_some(handle))
                .and_then(|handle| {
                    let a11y = (door.reach)(handle)?;
                    handle
                        .update(cx, |_, window, _| reveal(&name, window, &a11y, &id))
                        .ok()
                })
                .unwrap_or_else(|| Reply::new(404, json!({ "error": "no such window" })))
        }
    }
}

struct ServedPerf {
    name: String,
    key: crate::runtime::WindowKey,
    a11y: bool,
    gpui: serde_json::Value,
}

fn perf_off() -> Reply {
    Reply::new(
        409,
        json!({ "error": "perf is off: launch with DUCKTAPE_PERF=1" }),
    )
}

fn perf_reset_reply() -> Reply {
    if !crate::perf::on() {
        return perf_off();
    }
    crate::perf::reset();
    Reply::ok(json!({ "reset": true }))
}

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

#[cfg(feature = "perf-deep")]
fn gpui_perf(window: &Window) -> serde_json::Value {
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

pub(crate) async fn settled(door: &Door<'_>, until: Option<Instant>, cx: &mut AsyncApp) {
    while !cx.update(|cx| (door.settle)(cx)) && !passed(until) {
        cx.background_executor().timer(POLL).await;
    }
}

async fn settle(
    before: &[AxNode],
    (deadline_ms, until): (Option<u64>, Option<Instant>),
    door: &Door<'_>,
    seen: &mut Seen,
    cx: &mut AsyncApp,
) -> Vec<AxNode> {
    let deadline = deadline(deadline_ms.unwrap_or(2000));
    let mut after = before.to_vec();
    let mut last = serde_json::to_string(&after).unwrap_or_default();
    let mut quiet = 0;
    while quiet < 3 && !passed(deadline) {
        cx.background_executor().timer(POLL).await;
        let now = read(door, &Filter::default(), false, until, seen, cx).await;
        let key = serde_json::to_string(&now).unwrap_or_default();
        match key == last {
            true => quiet += 1,
            false => (quiet, last, after) = (0, key, now),
        }
    }
    after
}

#[cfg(any(test, feature = "test-support"))]
pub fn ask<V: 'static>(
    harness: &ducktape_a11y_platform::headless::Harness<V>,
    window: AnyWindowHandle,
    name: &str,
    request: Request,
    keep: &'static dyn Fn(&Window, &App) -> Option<Restore>,
) -> Reply {
    let platform = harness.platform.clone();
    let name = name.to_owned();
    let key = crate::runtime::WindowKey::unique();
    harness.run(async move |cx| {
        let windows = move |_: &App| vec![(name.clone(), key, window)];
        let reach = move |handle: AnyWindowHandle| platform.window(handle);
        let door = Door {
            windows: &windows,
            settle: &|_| true,
            keep,
            chords: &Vec::new,
            views: &|_| Vec::new(),
            reach: &reach,
            stopped: &|_| Vec::new(),
        };
        answer(request, &door, &mut Seen::default(), cx).await
    })
}

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
    use ducktape_a11y_platform::headless::Harness;

    #[test]
    fn a_deadline_is_the_clients() {
        assert!(!passed(deadline(u64::MAX)));
        assert!(passed(deadline(0)));
    }

    #[test]
    fn drag_steps_are_the_callers() {
        let drag = |steps| Drag {
            id: None,
            from: [0., 0.],
            to: [10., 10.],
            steps: Some(steps),
            window: None,
            deadline_ms: None,
        };
        assert_eq!(drag(1 << 20).checked(), Ok(1 << 20));
        assert_eq!(drag(0).checked().map_err(|reply| reply.status), Err(400));
    }

    fn served(name: &str, a11y: bool) -> ServedPerf {
        ServedPerf {
            name: name.to_owned(),
            key: crate::runtime::WindowKey::unique(),
            a11y,
            gpui: serde_json::Value::Null,
        }
    }

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

    fn opened() -> Harness<crate::render::ViewTree> {
        Harness::open_with(
            gpui_kit::init,
            gpui_kit::size(gpui_kit::px(400.), gpui_kit::px(300.)),
            |_, _| crate::render::ViewTree::new(view_wire::Node::empty()),
        )
    }

    fn served_answer(
        harness: &Harness<crate::render::ViewTree>,
        request: Request,
    ) -> (Reply, Reply, bool) {
        let window: AnyWindowHandle = harness.root.into();
        let answered = ask(harness, window, "console", request, &|_, _| None);
        let perf = Request::Perf { by_instance: false };
        let perf = ask(harness, window, "console", perf, &|_, _| None);
        let active = harness.update(|window, _| window.is_a11y_active());
        (answered, perf, active)
    }

    #[test]
    fn a_press_without_a_read_keeps_the_cache_on() {
        let _on = crate::perf::on_for_test();
        let harness = opened();
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let _heard = harness.app.update(|cx| {
            let heard = heard.clone();
            cx.observe_keystrokes(move |event, _, _| {
                heard.borrow_mut().push(event.keystroke.key.clone())
            })
        });
        let press = Request::Key(Key {
            keys: "tab".into(),
            text: String::new(),
            window: None,
            deadline_ms: None,
            delta: false,
        });
        let (answered, perf, active) = served_answer(&harness, press);
        assert_eq!((answered.status, answered.body.as_str()), (200, "{}"));
        assert_eq!(*heard.borrow(), ["tab"], "the press reached the window");
        assert!(
            !active,
            "a press that reads nothing does not switch a11y on"
        );
        let perf: serde_json::Value = serde_json::from_str(&perf.body).unwrap();
        assert_eq!(perf["cache_on"], true);

        let read = Request::Tree {
            filter: Filter::default(),
            compact: true,
            bounds: false,
        };
        let (_, perf, active) = served_answer(&opened(), read);
        assert!(active, "a tree read switches a11y on");
        let perf: serde_json::Value = serde_json::from_str(&perf.body).unwrap();
        assert_eq!(perf["cache_on"], false);
    }

    #[test]
    fn motion_asks_for_less_motion_and_takes_it_back() {
        let harness = opened();
        for reduce in [true, false] {
            let ask = Request::Motion(Motion { reduce });
            let (answered, _, _) = served_answer(&harness, ask);
            assert_eq!((answered.status, answered.body.as_str()), (200, "{}"));
            assert_eq!(harness.app.update(|cx| cx.reduce_motion()), reduce);
        }
    }

    #[gpui_kit::test]
    async fn a_read_turns_the_seats_first(cx: &mut gpui_kit::TestAppContext) {
        const PROGRAM: &str = "door-read-turns-view";
        crate::runtime::seat_for_test(PROGRAM, 200);
        cx.update(gpui_kit::init);
        let seat = cx.update(|cx| {
            use gpui_kit::AppContext as _;
            cx.new(|cx| {
                let instance = 1;
                crate::runtime::Seat::new(PROGRAM, instance, cx)
            })
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
        struct Pane {
            seat: gpui_kit::Entity<crate::runtime::Seat>,
            _watch: gpui_kit::Subscription,
        }
        impl gpui_kit::Render for Pane {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                cx: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                use gpui_kit::{ParentElement as _, Styled as _};
                gpui_kit::div()
                    .size_full()
                    .children(self.seat.read(cx).tree())
            }
        }
        let window = cx.open_window(
            gpui_kit::size(gpui_kit::px(400.), gpui_kit::px(300.)),
            |window, cx| {
                seat.update(cx, |seat, cx| {
                    seat.place(window.window_handle(), Some((400., 300.)), true, cx)
                });
                Pane {
                    seat: seat.clone(),
                    _watch: cx.observe(&seat, |_, _, cx| cx.notify()),
                }
            },
        );
        cx.run_until_parked();
        let handle: AnyWindowHandle = window.into();
        let key = crate::runtime::WindowKey::unique();
        let served = move |_: &App| vec![("console".to_owned(), key, handle)];
        let read = || Request::Tree {
            filter: Filter::default(),
            compact: true,
            bounds: false,
        };
        for (settles, expected) in [(false, false), (true, true)] {
            crate::runtime::intent_for_test(PROGRAM, instance, crate::runtime::Intent::Badge(3));
            heard.borrow_mut().clear();
            let turning = seat.clone();
            cx.spawn(async move |mut cx| {
                let settle = move |cx: &mut App| {
                    turning.update(cx, |seat, cx| seat.turn(cx));
                    true
                };
                let quiet = |_: &mut App| true;
                let door = Door {
                    keep: &|_, _| None,
                    chords: &Vec::new,
                    views: &|_| Vec::new(),
                    windows: &served,
                    settle: if settles { &settle } else { &quiet },
                    reach: &|_| None,
                    stopped: &|_| Vec::new(),
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
