//! Program-owned views. Every screen the app draws inside its chrome is a
//! wasm view a PROGRAM on the connected network ships (`backend::views`:
//! the roster's code blob, its `ducktape.view` section) — nothing wasm
//! ships beside the binary, and with no node there is no view. A view
//! developer's `DUCKTAPE_VIEWS_DIR` ([`override_views_from`]) is the one
//! exception, and says so in app.log for every view it supplies. A view is
//! ticked inside a fuel and time budget, and presented through native
//! gpui-kit controls in its seat.
//!
//! The app knows no program by name. The roster's order is the rail's, a
//! view's manifest names its tab, and what a view asks of the app is the
//! kernel contract (`kernel`) alone; its props are the session basics every
//! view gets.

mod clipboard;
mod guest;
mod kernel;
pub(crate) mod notify;
mod roster;
mod seat;
mod store;
mod widget;

pub(crate) use kernel::command_held;
#[cfg(test)]
pub(crate) use roster::list_for_test;
pub use roster::{
    Link, RailRow, connected, deployments_checked, listed_view, parse_link, props, rail,
    valid_route,
};
pub(crate) use seat::{Failure, NODE_UNREACHABLE, retry};
pub use seat::{Loads, override_views_from};
pub(crate) use widget::NativeModuleView;

use guest::Guest;
use roster::{Connection, code_digest, connection, listed_code};
use seat::{
    LoadTiming, Loaded, Mounted, Slot, Unloaded, log_source, mounted, registry, spawn_load,
    view_override,
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

/// One desktop window, as the shell and the notification policy (which
/// window is in front) name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct WindowKey(u64);

impl WindowKey {
    pub(crate) fn unique() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What a module view asked the app itself to do, off its `host.*` doors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `host.badge`: its unread count on the menu bar; 0 or less clears it.
    Badge(i64),
    /// `host.open_link`: a link it pressed.
    OpenLink(String),
    /// A notice was posted: the bell and the permission bar redraw.
    Notified,
}

/// Instruction budget for one call into a view: a ceiling that ends a
/// runaway, not a cost. Measured 2026-09 (`RUST_LOG=ducktape::fuel=debug`):
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

// ---------- the node seat ----------

/// The route a link asked of each module's view, waiting for that view's
/// first `host.route` subscriber. One per module: a newer link replaces an
/// older one no view has read yet.
fn pending_routes() -> &'static Mutex<std::collections::BTreeMap<&'static str, String>> {
    static ROUTES: OnceLock<Mutex<std::collections::BTreeMap<&'static str, String>>> =
        OnceLock::new();
    ROUTES.get_or_init(Mutex::default)
}

/// Hold `route` for `module`'s view until it asks.
pub(crate) fn route_to(module: &'static str, route: String) {
    pending_routes()
        .lock()
        .expect("pending routes")
        .insert(module, route);
}

/// The route waiting for `module`, handed out once.
pub(crate) fn take_route(module: &str) -> Option<String> {
    pending_routes()
        .lock()
        .expect("pending routes")
        .remove(module)
}

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

pub(crate) mod input;
mod pictures;

/// The name a view's manifest gives it and the capabilities it declares;
/// empty for one whose manifest cannot be read (`compile` refuses those
/// before a seat).
fn manifest_of(bytes: &[u8]) -> (String, Vec<String>) {
    view_wire::manifest::read_manifest(bytes)
        .map(|manifest| (manifest.name, manifest.capabilities))
        .unwrap_or_default()
}
