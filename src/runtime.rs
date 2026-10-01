//! Program-owned views. Every screen the app draws inside its chrome is a
//! wasm view a PROGRAM on the connected network ships (`backend::views`:
//! the roster's code blob, its `ducktape.view` section) — nothing wasm
//! ships beside the binary, and with no node there is no view. A view
//! developer's `DUCKTAPE_VIEWS_DIR` ([`override_views_from`]) is the one
//! exception, and says so in app.log for every view it supplies. A view is
//! ticked inside a fuel budget (`FUEL_PER_TICK`, the one ceiling on a
//! call), and presented through native gpui-kit controls in its seat.
//!
//! The app knows no program by name. The roster's order is the rail's, a
//! view's manifest names its tab, and what a view asks of the app is the
//! kernel contract (`kernel`) alone; its props are the session basics every
//! view gets.
//!
//! This file is also the submodules' PRELUDE: `guest`, `roster`, `seat`,
//! `input` and `display_diagnostics` open with `use super::*;`,
//! so every private `use` below (`Guest`, `Connection`, `Mounted`, `Slot`,
//! `Instant`, `wire`, the wasmtime types...) and every constant is theirs
//! too. A name a child uses without importing it comes from here. `kernel`,
//! `clipboard`, `notify` and `store` import what they need by name.

mod clipboard;
mod guest;
mod kernel;
pub(crate) mod notify;
mod roster;
mod seat;
mod store;

pub(crate) use kernel::local_offset;
pub use roster::{Link, RailRow, connected, deployments_checked, props, valid_route};
pub(crate) use roster::{Roster, changes_channel, roster};
pub(crate) use seat::{Failure, NODE_UNREACHABLE, Seat};
pub use seat::{Loads, override_views_from};

use guest::Guest;
use roster::{Connection, code_digest, connection, rail_moved};
use seat::{
    LoadTiming, Loaded, Mounted, Slot, Unloaded, log_source, registry, spawn_load, view_override,
};

/// Shared HTTP connections need a continuously driven I/O runtime. Loader
/// threads can compile or join child loads between requests; their own parked
/// runtimes would strand the pooled sockets another loader reuses.
pub(crate) fn handle() -> tokio::runtime::Handle {
    kernel::handle()
}

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::editor::wire::EditorStore;
use gpui_kit::AppContext as _;
use pictures::Pictures;
use view_wire as wire;
use wasmtime::{
    Cache, CacheConfig, Caller, Config, Engine, Linker, Memory, Module, OptLevel, Store,
    StoreLimits, StoreLimitsBuilder, TypedFunc,
};
use wire::methods::Capability;

/// One desktop window, as the shell and the notification policy (which
/// window is in front) name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WindowKey(pub(crate) u64);

impl WindowKey {
    pub(crate) fn unique() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What a view asked the app itself to do: `host.badge`, `link.open`, or a
/// `notify.post` that changed the centre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `host.badge`: its unread count on the menu bar; 0 or less clears it.
    Badge(i64),
    /// `link.open`: a link it pressed.
    OpenLink(String),
    /// A notice was posted: the bell and the permission bar redraw.
    Notified,
    /// A view was seated in its tab, first or after a new deployment: its
    /// minimum width is known, and the desk widens a window under it.
    Seated,
}

/// Instruction budget for one call into a view: a ceiling that ends a
/// runaway, not a cost. Measured 2026-09 (`RUST_LOG=ducktape::perf=debug`):
/// chat's heaviest tick ~72M (#general, a menu or the emoji picker opening),
/// forge with a large file ~45M, explorer ~26M; the ceiling keeps the
/// heaviest under 30% of it.
const FUEL_PER_TICK: u64 = 250_000_000;
const MEMORY_LIMIT: usize = 64 << 20;
/// A frame the view sends past this ends it: nothing a screen needs is
/// megabytes, and the host would decode all of it on the window thread.
const MAX_FRAME_BYTES: usize = 8 << 20;
const MAX_REQUESTS_PER_TICK: usize = 256;
const MAX_PAYLOAD_BYTES: usize = 1 << 20;
/// The most one op may carry: the kernel bounds nothing, this host does.
const MAX_OP_BYTES: usize = 16 << 20;
/// The gap before the first re-attempt at a candidate whose load failed,
/// and the longest that gap widens to. A load fetches the artifact,
/// instantiates it, restores the snapshot and verifies the first tree —
/// and compiles the view too, whenever `compiled_view` does not
/// already hold it. A deployment that cannot load fails that way every
/// time, so trying it once a block buys nothing and never stops. Widening
/// the gap rather than giving up keeps a fetch that failed on the
/// transport recoverable: nothing is suppressed for good, only spaced out.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// The route a link asked of each module's view, waiting for that view's
/// first `host.route` subscriber. One per module: a newer link replaces an
/// older one no view has read yet.
fn pending_routes() -> &'static Mutex<std::collections::BTreeMap<&'static str, String>> {
    static ROUTES: OnceLock<Mutex<std::collections::BTreeMap<&'static str, String>>> =
        OnceLock::new();
    ROUTES.get_or_init(Mutex::default)
}

/// Hold `route` for `module`'s view until it asks, and wake the seats
/// already open on it: a view that is up reads the route in its next
/// turn, and nothing else about it moves.
pub(crate) fn route_to(module: &'static str, route: String) {
    pending_routes()
        .lock()
        .expect("pending routes")
        .insert(module, route);
    // registry, then seat: the order `retry` takes them in
    let registry = registry().lock().expect("module views");
    for seat in registry
        .iter()
        .filter(|((name, _), _)| *name == module)
        .map(|(_, seat)| seat)
    {
        seat.lock().expect("module view lock").wake.send_replace(());
    }
}

/// The route waiting for `module`, handed out once.
pub(crate) fn take_route(module: &str) -> Option<String> {
    pending_routes()
        .lock()
        .expect("pending routes")
        .remove(module)
}

/// Leaks one copy of each distinct module name, so a name can be the
/// `&'static str` key the registry, the rail and the routes use.
pub(crate) fn intern(id: &str) -> &'static str {
    static INTERNED: OnceLock<Mutex<std::collections::BTreeSet<&'static str>>> = OnceLock::new();
    let mut interned = INTERNED
        .get_or_init(Mutex::default)
        .lock()
        .expect("interned ids");
    if let Some(known) = interned.get(id) {
        return known;
    }
    let leaked: &'static str = Box::leak(id.to_owned().into_boxed_str());
    interned.insert(leaked);
    leaked
}

mod display_diagnostics;

pub(crate) mod pictures;

/// The name a view's manifest gives it, the capabilities it declares and
/// the narrowest it is laid out; empty and 0 for one whose manifest cannot
/// be read (`compile` refuses those before a seat).
fn manifest_of(bytes: &[u8]) -> (String, Vec<Capability>, u32) {
    view_wire::manifest::read_manifest(bytes)
        .map(|manifest| (manifest.name, manifest.capabilities, manifest.min_width))
        .unwrap_or_default()
}

/// The narrowest `module`'s view is laid out, in px, once one of its seats
/// holds it drawn; `None` while it loads, failed, or has none.
pub(crate) fn min_width(module: &str) -> Option<f32> {
    let registry = registry().lock().expect("module views");
    registry
        .iter()
        .filter(|((name, _), _)| *name == module)
        .find_map(
            |(_, seat)| match &seat.lock().expect("module view lock").slot {
                Slot::Ready(guest) => Some(guest.min_width as f32),
                _ => None,
            },
        )
}

/// Every seat of `module` holds a drawn view whose manifest says
/// `min_width`, or, with none yet, one is preloaded for its first pane: a
/// WAT view with the five exports whose every tick draws an empty tree.
#[cfg(test)]
pub(crate) fn seat_for_test(module: &'static str, min_width: u32) {
    seat_drawing_for_test(module, min_width, wire::Node::empty());
}

/// `frame` encoded as the string of a WAT `data` segment, and its length.
#[cfg(test)]
pub(crate) fn wat_frame(frame: &wire::Frame) -> (String, u32) {
    let frame = wire::encode(frame);
    let bytes = frame.iter().map(|byte| format!("\\{byte:02x}")).collect();
    (bytes, frame.len() as u32)
}

/// [`seat_for_test`], its view drawing `root` on every tick.
#[cfg(test)]
pub(crate) fn seat_drawing_for_test(module: &'static str, min_width: u32, root: wire::Node) {
    let (bytes, len) = wat_frame(&wire::Frame {
        root: Some(root),
        ..Default::default()
    });
    let tick = wire::abi::pack(65536, len);
    let code = Module::new(
        guest::engine(),
        format!(
            r#"(module
            (memory (export "memory") 2)
            (data (i32.const 65536) "{bytes}")
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const {tick})
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#
        ),
    )
    .unwrap();
    seat_code_for_test(module, min_width, code);
}

/// [`seat_for_test`], its view saying `busy` (out of budget, tick again
/// soon) on its first `busy_ticks` ticks and quiet from then on.
#[cfg(test)]
pub(crate) fn seat_busy_for_test(module: &'static str, min_width: u32, busy_ticks: u32) {
    let frame = |busy| wire::Frame {
        root: Some(wire::Node::empty()),
        busy,
        ..Default::default()
    };
    let code = frames_code(&frame(true), busy_ticks, &frame(false));
    seat_code_for_test(module, min_width, code);
}

/// [`seat_for_test`], its view drawing `first` on its first tick, saying
/// `busy` (tick again soon), and `then` on every later tick: a view whose
/// first frame is not its settled one.
#[cfg(test)]
pub(crate) fn seat_frames_for_test(
    module: &'static str,
    min_width: u32,
    first: wire::Node,
    then: wire::Node,
) {
    let frame = |root, busy| wire::Frame {
        root: Some(root),
        busy,
        ..Default::default()
    };
    let code = frames_code(&frame(first, true), 1, &frame(then, false));
    seat_code_for_test(module, min_width, code);
}

/// A view's code answering its first `first_ticks` ticks with `first` and
/// every later tick with `then`.
#[cfg(test)]
pub(crate) fn frames_code(first: &wire::Frame, first_ticks: u32, then: &wire::Frame) -> Module {
    let (first, first_len) = wat_frame(first);
    let (then, then_len) = wat_frame(then);
    assert!(
        first_len < 4096 && then_len < 4096,
        "test frames fit their pages"
    );
    let first_tick = wire::abi::pack(65536, first_len);
    let then_tick = wire::abi::pack(69632, then_len);
    Module::new(
        guest::engine(),
        format!(
            r#"(module
            (memory (export "memory") 2)
            (global $n (mut i32) (i32.const 0))
            (data (i32.const 65536) "{first}")
            (data (i32.const 69632) "{then}")
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64)
                global.get $n i32.const 1 i32.add global.set $n
                global.get $n i32.const {first_ticks} i32.le_u
                if (result i64) i64.const {first_tick} else i64.const {then_tick} end)
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#
        ),
    )
    .unwrap()
}

/// An intent `module`'s seat `instance` will hand over on its next update,
/// as a `host.badge` or `link.open` request would leave it.
#[cfg(test)]
pub(crate) fn intent_for_test(module: &str, instance: u64, intent: Intent) {
    let registry = registry().lock().unwrap();
    let seat = &registry[&(module, instance)];
    let Slot::Ready(guest) = &mut seat.lock().unwrap().slot else {
        panic!("{module} is not seated");
    };
    guest.intents.push(intent);
}

/// `module`'s seat `instance` traps: its guest keeps its tree but shows a
/// fault from now on, and the seat is woken as a trapping tick would.
#[cfg(test)]
pub(crate) fn fault_for_test(module: &str, instance: u64) {
    let registry = registry().lock().unwrap();
    let mut locked = registry[&(module, instance)].lock().unwrap();
    let Slot::Ready(guest) = &mut locked.slot else {
        panic!("{module} is not seated");
    };
    guest.fault = Some("trapped for the test".into());
    locked.wake.send_replace(());
}

#[cfg(test)]
pub(crate) fn seat_code_for_test(module: &'static str, min_width: u32, code: Module) {
    let ready = || {
        let mut guest = Guest::instantiate(module, &code, module).unwrap();
        guest.min_width = min_width;
        Slot::Ready(Box::new(guest))
    };
    let mut registry = registry().lock().unwrap();
    let mut seats: Vec<_> = registry
        .iter()
        .filter(|((name, _), _)| *name == module)
        .map(|(_, seat)| seat.clone())
        .collect();
    if seats.is_empty() {
        seats.push(Mounted::seat());
        registry.insert((module, 0), seats[0].clone());
    }
    for seat in seats {
        let mut locked = seat.lock().unwrap();
        locked.slot = ready();
        locked.wake.send_replace(());
    }
}
