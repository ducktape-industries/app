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

mod filesystem;
mod guest;
mod kernel;
mod media;
pub(crate) mod notify;
mod roster;
mod seat;
mod widget;

pub(crate) use kernel::chord_of;
pub(crate) use media::capturing;
#[cfg(test)]
pub(crate) use roster::list_for_test;
pub use roster::{
    RailRow, connected, deployments_checked, listed_view, local_link, local_route, props, rail,
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

/// Instruction budget for one call into a view.
const FUEL_PER_TICK: u64 = 100_000_000;
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

/// Which module holds each claimed chord, and how many live subscriptions
/// hold it. Global, because the claim is: two seats answering to one chord
/// would make the key mean whichever guest the shell reached first that
/// frame.
///
/// The COUNT is what lets a chord move. A claim is made per subscription and
/// given back per subscription — including the ones a torn-down guest still
/// had — so a swap, in which the replacement claims before the instance it
/// replaces is dropped, leaves the holder standing rather than releasing the
/// key out from under the view that just took it.
type ChordClaims = std::collections::BTreeMap<String, (&'static str, usize)>;

fn chord_claims() -> &'static Mutex<ChordClaims> {
    static CLAIMS: OnceLock<Mutex<ChordClaims>> = OnceLock::new();
    CLAIMS.get_or_init(Mutex::default)
}

/// Claim `chord` for `module`, or name the module that already holds it.
///
/// FIRST COME HOLDS IT, and a module re-claiming its own is another claim on
/// the same key: a swap installs a new instance of the same id, and a view
/// must not lose its chord by being replaced with itself.
pub(crate) fn claim_chord(chord: &str, module: &'static str) -> Result<(), &'static str> {
    let mut claims = chord_claims().lock().expect("chord claims");
    match claims.get_mut(chord) {
        Some((holder, _)) if *holder != module => Err(holder),
        Some((_, held)) => {
            *held += 1;
            Ok(())
        }
        None => {
            claims.insert(chord.to_owned(), (module, 1));
            Ok(())
        }
    }
}

/// Give one claim on `chord` back. The key is free again once the last one
/// is given back — a chord nothing is listening for is a chord the next view
/// may have, and a claim that outlived its guest would make the key
/// unclaimable until the app restarted.
pub(crate) fn release_chord(chord: &str, module: &'static str) {
    let mut claims = chord_claims().lock().expect("chord claims");
    let Some((holder, held)) = claims.get_mut(chord) else {
        return;
    };
    if *holder != module {
        return;
    }
    *held -= 1;
    if *held == 0 {
        claims.remove(chord);
    }
}

/// The module that holds `chord`, if any — what the shell asks before it
/// carries a press to a seat.
pub(crate) fn chord_holder(chord: &str) -> Option<&'static str> {
    chord_claims()
        .lock()
        .expect("chord claims")
        .get(chord)
        .map(|(holder, _)| *holder)
}

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

#[path = "runtime/display_diagnostics.rs"]
mod display_diagnostics;

#[path = "runtime/input.rs"]
pub(crate) mod input;
#[path = "runtime/pictures.rs"]
mod pictures;

/// The name a view's manifest gives it; "" for one whose manifest
/// cannot be read (`compile` refuses those before a seat).
fn manifest_name(bytes: &[u8]) -> String {
    view_wire::manifest::read_manifest(bytes)
        .map(|manifest| manifest.name)
        .unwrap_or_default()
}
