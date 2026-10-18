//! Program-owned views. Every screen the app draws inside its chrome is a
//! wasm view a PROGRAM on the connected network ships (`backend::views`:
//! the roster's code blob, its `ducktape.view` section) — nothing wasm
//! ships beside the binary, and with no node there is no view. A view
//! developer's override directory (`Dirs::views_override`, the desktop's
//! `DUCKTAPE_VIEWS_DIR`) is the one exception, and says so in app.log for
//! every view it supplies. A view is
//! ticked inside a fuel budget (`FUEL_PER_TICK`, the one ceiling on a
//! call), and presented through native gpui-kit controls in its seat.
//!
//! The app knows no program by name. The roster's order is the rail's, a
//! view's manifest names its tab, and what a view asks of the app is the
//! kernel contract (`kernel`) alone; its props are the session basics every
//! view gets.
//!
//! This file is also the submodules' PRELUDE: `view`, `roster`, `seat`,
//! `input` and `display_diagnostics` open with `use super::*;`,
//! so every private `use` below (`View`, `Connection`, `Mounted`, `Slot`,
//! `Instant`, `wire`, the wasmtime types...) and every constant is theirs
//! too. A name a child uses without importing it comes from here. `kernel`,
//! `clipboard`, `picker`, `notify` and `store` import what they need by
//! name. Each program's notifier (`notifier`) is a view with no tree, run
//! off the window thread.

mod clipboard;
pub mod consent;
mod kernel;
pub(crate) mod notifier;
pub mod notify;
mod picker;
mod roster;
mod seat;
mod store;
mod view;
pub mod web;

#[cfg(feature = "call")]
pub(crate) use kernel::live::Subscribed;
pub use kernel::live::{HostLive, HostPublication, HostSubscription, TrackHeard};
pub use kernel::{Answered, day, local_offset};
// a host with no screen runs each program's notifier per account (`crates/pushd`)
pub use notifier::{Carried, Recipient, Said, notify_for};
// the web view cleans a page's title as a window's (render/web.rs), the
// av pieces a device's name (render/av.rs)
#[cfg(any(feature = "web", feature = "av"))]
pub use kernel::shown_title;
// a device's state line, kept whole up to the wire's 200 bytes
// (render/av.rs)
#[cfg(feature = "av")]
pub use kernel::plain;
pub use roster::{
    Link, Prep, RailRow, connected, deployments_checked, disconnected, props, valid_route,
};
pub use roster::{Roster, changes_channel, roster};
pub use seat::{Back, Failure};
#[cfg(feature = "ui")]
pub use seat::{Backgrounded, Placed, Seat, serve_starts};

use roster::{Connection, code_digest, connection, rail_moved};
use seat::{
    Load, LoadTiming, Loaded, Mounted, Registry, Slot, Unloaded, ViewCode, log_source, queue,
    registry, view_override,
};
use view::{Redraw, View};

/// Shared HTTP connections need a continuously driven I/O runtime. Loader
/// threads compile between requests; their own parked runtimes would strand
/// the pooled sockets another loader reuses.
pub fn handle() -> tokio::runtime::Handle {
    kernel::handle()
}

/// The app's quit hook (`shell::launch`, the one every quit path reaches):
/// the notification log handed over, then a wait on every in-order queue
/// (store files, the log, banners) and on the live session's close
/// (`kernel::live::quit`), then `kept` dropped. The returned future is the
/// wait; gpui gives it `SHUTDOWN_TIMEOUT` (200 ms).
pub fn quitting(center: &notify::CenterHandle, kept: impl Sized) -> impl Future<Output = ()> {
    center.lock().flush();
    let drained = kernel::drain();
    let live = kernel::live::quit();
    drop(kept);
    async move {
        futures::future::join(drained, live).await;
    }
}

/// A lock the window thread and the loaders share, taken even after a
/// thread panicked holding it: a panic on a loader never takes the window
/// thread, and every window with it, down on its next turn.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::contract::{
    CodeSource, FUEL_PER_TICK, Fault, Landed, Landing, Prepared, SeatId, ViewInstance, ViewRunner,
};
#[cfg(feature = "ui")]
use gpui_kit::AppContext as _;
use pictures::Pictures;
use view_wire as wire;
use wire::manifest::Tint;
use wire::methods::{Capability, refusal};

/// One desktop window, as the shell and the notification policy (which
/// window is in front) name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowKey(pub u64);

impl WindowKey {
    pub fn unique() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What a view asked the app itself to do: `host.badge`, `host.title`,
/// `link.open`, `host.float`, `host.unfloat`, or a `notify.post` that
/// changed the centre. An `op.submit`
/// waiting on the person's yes wakes the shell itself (`consent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `host.badge`: its unread count on the menu bar; 0 or less clears it.
    Badge(i64),
    /// `host.title`: the name of its window, cleaned (`kernel`); empty
    /// clears it.
    Title(String),
    /// `link.open`: a link it pressed.
    OpenLink(String),
    /// `host.show`: one of the host's own places, asked for by a press in
    /// the view; how it looks is each shell's (the desktop's account menu,
    /// the phone's account sheet).
    Show(view_wire::methods::Place),
    /// A notice was posted: the bell and the permission bar redraw.
    Notified,
    /// A view was seated in its tab, first or after a new deployment: its
    /// minimum width is known, and the desk widens a window under it.
    Seated,
    /// `host.float`: its pane in a window of its own, at a press or key.
    Float(wire::methods::Float),
    /// `host.unfloat`: its pane back on the desk it left.
    Unfloat,
}

/// How many view loads run at once, each on a loader thread of its own
/// (`Limits::loaders`: the desktop one a core, a phone one), so a long
/// roster waits its turn rather than forking a thread per program.
fn loaders() -> usize {
    crate::contract::os().limits().loaders
}

type Routes = Mutex<std::collections::BTreeMap<(&'static str, u64), String>>;

/// The route a link asked of one seat's view, by the seat's registry key
/// (`program`, its pane's instance), waiting for that view's first
/// `host.route` subscriber. One per seat: a newer link replaces an older
/// one the view has not read yet.
fn pending_routes() -> &'static Routes {
    static ROUTES: OnceLock<Routes> = OnceLock::new();
    ROUTES.get_or_init(Mutex::default)
}

/// Hold `route` for seat `instance` of `program` until its view asks, and
/// wake that seat if it is open: its view reads the route in its next
/// turn, and no other seat of the program hears of it.
pub fn route_to(program: &'static str, instance: u64, route: String) {
    pending_routes()
        .lock()
        .expect("pending routes")
        .insert((program, instance), route);
    // registry, then seat: the order `retry` takes them in
    if let Some(seat) = lock(registry()).get(&(program, instance)) {
        lock(seat).wake.send_replace(());
    }
}

/// The route waiting for seat `instance` of `program`, handed out once.
pub fn take_route(program: &'static str, instance: u64) -> Option<String> {
    pending_routes()
        .lock()
        .expect("pending routes")
        .remove(&(program, instance))
}

/// The ids of a node and of its identified ancestors, root first: the key
/// every piece of retained native state is stored under. Only a node with an
/// id (`Node::identity`) adds a segment; one without shares its nearest
/// identified ancestor's path, so only identified nodes may write a map
/// keyed by it.
pub type AuthoredPath = Vec<wire::ElementIdWire>;

/// Pushes the id `node` is filed under onto `path` (`wire::identity`: its
/// own, else, as row `row` of a list, its index); the caller pops on the
/// way out when this answers `true`.
pub fn enter_scope(node: &wire::Node, row: Option<usize>, path: &mut AuthoredPath) -> bool {
    match wire::identity::segment(node.identity().cloned(), row) {
        Some(id) => {
            path.push(id);
            true
        }
        None => false,
    }
}

/// Leaks one copy of each distinct program name, so a name can be the
/// `&'static str` key the registry, the rail and the routes use.
pub fn intern(id: &str) -> &'static str {
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

pub mod pictures;

/// What a view's manifest says, read before any of its code runs: the
/// name, icon, tint and description the rail and Home list it by, the narrowest
/// it is laid out, the capabilities it declares, its targets, and the wire
/// it was built against (another than this app's runs all the same, and
/// whatever then fails says so: `View::failure`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub name: String,
    pub icon: String,
    pub tint: Tint,
    pub description: String,
    pub min_width: u32,
    pub capabilities: Vec<Capability>,
    pub targets: Vec<String>,
    pub wire: String,
}

/// `bytes`' manifest as the host reads it; empty and 0 for one whose
/// manifest cannot be read (`prepare` refuses those before a seat).
fn manifest_of(bytes: &[u8]) -> Listing {
    view_wire::manifest::read_manifest(bytes)
        .map(|manifest| Listing {
            name: manifest.name,
            icon: manifest.icon,
            tint: manifest.tint,
            description: manifest.description,
            min_width: manifest.min_width,
            capabilities: manifest.capabilities,
            targets: manifest.targets,
            wire: manifest.wire_id,
        })
        .unwrap_or_default()
}

/// What `program`'s manifest says, once one of its seats has read it: the
/// phone's product page (wasm-mobile SPEC §7.2). `None` while its bytes
/// are coming, when it failed, or ships no view.
pub fn listing(program: &str) -> Option<Listing> {
    // a drawn view and its compiled code carry the manifest's facts under
    // the same names
    macro_rules! listing_of {
        ($held:expr) => {
            Listing {
                name: $held.name.clone(),
                icon: $held.icon.clone(),
                tint: $held.tint,
                description: $held.description.clone(),
                min_width: $held.min_width,
                capabilities: $held.capabilities.clone(),
                targets: $held.targets.clone(),
                wire: $held.wire.clone(),
            }
        };
    }
    let registry = lock(registry());
    registry
        .iter()
        .filter(|((name, _), _)| *name == program)
        .find_map(|(_, seat)| match &lock(seat).slot {
            Slot::Ready(view) => Some(listing_of!(view)),
            Slot::Compiled(code) => Some(listing_of!(code)),
            Slot::Listed(listing) | Slot::Compiling(listing) => Some((**listing).clone()),
            _ => None,
        })
}

/// What `capability` lets a view do, in a line a person reads: the Expo
/// app's words (mobile `src/network/words.ts` `ALLOWS`, explorer-view's
/// program page). Every capability is worded here, so a new one is
/// worded before it ships.
pub fn allows(capability: Capability) -> &'static str {
    match capability {
        Capability::Chain => "reads the chain: blocks, transactions and the node's status",
        Capability::Program => "asks programs questions and follows what they change",
        Capability::Op => "sends operations to the chain in your name",
        Capability::Invite => "mints invites with the node's key",
        Capability::Link => "opens links: another view, or the browser",
        Capability::Blob => "reads blobs the node holds",
        Capability::Host => "knows the session: who you are, which chain, when it is on screen",
        Capability::Clock => "wakes on a timer",
        Capability::Clipboard => "reads and writes the clipboard",
        Capability::Notify => "posts notifications",
        Capability::Store => "keeps its own data on this device",
        Capability::Web => "shows web pages, and takes the web links the app opens",
        Capability::Live => {
            "sends and hears live streams in its program's rooms, as the program grants"
        }
        Capability::Camera => {
            "shows your camera's picture: the app draws it, the view never sees it"
        }
        Capability::Microphone => {
            "uses your microphone and speakers: the app records and plays, the view never hears it"
        }
        Capability::File => {
            "lets you pick files on this device: it sees their names and sizes, never where they are"
        }
    }
}

/// The narrowest `program`'s view is laid out, in px, once one of its seats
/// holds it drawn or compiled; `None` while it loads, failed, or has none.
pub fn min_width(program: &str) -> Option<f32> {
    let registry = lock(registry());
    registry
        .iter()
        .filter(|((name, _), _)| *name == program)
        .find_map(|(_, seat)| match &lock(seat).slot {
            Slot::Ready(view) => Some(view.min_width as f32),
            Slot::Compiled(view) => Some(view.min_width as f32),
            _ => None,
        })
}

/// Every seat of `program` holds a drawn view whose manifest says
/// `min_width`, or, with none yet, one is preloaded for its first pane: a
/// WAT view with the five exports whose every tick draws an empty tree.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_for_test(program: &'static str, min_width: u32) {
    seat_drawing_for_test(program, min_width, wire::Node::empty());
}

/// `program`'s seat preloaded as far as compiled, its view's manifest saying
/// `name` and `icon` (none when empty): a rail row no pane has opened.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_compiled_for_test(program: &'static str, name: &str, icon: &str) {
    let seat = Mounted::seat();
    lock(&seat).slot = Slot::Compiled(ViewCode::for_test(name, icon, 320));
    lock(registry()).insert((program, 0), seat);
}

/// `program`'s seat with its bytes here and its manifest read (`name`), its
/// code not compiled yet: a row being prepared.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_listed_for_test(program: &'static str, name: &str) {
    let seat = Mounted::seat();
    lock(&seat).slot = Slot::Listed(Box::new(Listing {
        name: name.into(),
        ..Default::default()
    }));
    lock(registry()).insert((program, 0), seat);
}

/// `frame` encoded as the string of a WAT `data` segment, and its length.
#[cfg(any(test, feature = "test-support"))]
pub fn wat_frame(frame: &wire::Frame) -> (String, u32) {
    let frame = wire::encode(&crate::render::sent(frame.clone()));
    let bytes = frame.iter().map(|byte| format!("\\{byte:02x}")).collect();
    (bytes, frame.len() as u32)
}

/// [`seat_for_test`], its view drawing `root` on every tick.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_drawing_for_test(program: &'static str, min_width: u32, root: wire::Node) {
    seat_frame_for_test(
        program,
        min_width,
        wire::Frame {
            root: Some(root),
            ..Default::default()
        },
    );
}

/// [`seat_for_test`], its view answering every tick with `frame`.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_frame_for_test(program: &'static str, min_width: u32, frame: wire::Frame) {
    let (bytes, len) = wat_frame(&frame);
    let tick = wire::abi::pack(65536, len);
    let code = view::prepared_for_test(format!(
        r#"(module
            (memory (export "memory") 2)
            (data (i32.const 65536) "{bytes}")
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const {tick})
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#
    ));
    seat_code_for_test(program, min_width, code);
}

/// [`seat_for_test`], its view saying `busy` (out of budget, tick again
/// soon) on its first `busy_ticks` ticks and quiet from then on.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_busy_for_test(program: &'static str, min_width: u32, busy_ticks: u32) {
    let frame = |busy| wire::Frame {
        root: Some(wire::Node::empty()),
        busy,
        ..Default::default()
    };
    let code = frames_code(&frame(true), busy_ticks, &frame(false));
    seat_code_for_test(program, min_width, code);
}

/// [`seat_for_test`], its view drawing `first` on its first tick, saying
/// `busy` (tick again soon), and `then` on every later tick: a view whose
/// first frame is not its settled one.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_frames_for_test(
    program: &'static str,
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
    seat_code_for_test(program, min_width, code);
}

/// A view's code answering its first `first_ticks` ticks with `first` and
/// every later tick with `then`.
#[cfg(any(test, feature = "test-support"))]
pub fn frames_code(first: &wire::Frame, first_ticks: u32, then: &wire::Frame) -> [u8; 32] {
    let (first, first_len) = wat_frame(first);
    let (then, then_len) = wat_frame(then);
    assert!(
        first_len < 4096 && then_len < 4096,
        "test frames fit their pages"
    );
    let first_tick = wire::abi::pack(65536, first_len);
    let then_tick = wire::abi::pack(69632, then_len);
    view::prepared_for_test(format!(
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
    ))
}

/// [`seat_for_test`], its view answering `frames` in order, one per tick,
/// and the last one on every tick after: an `unchanged` frame after a full
/// one is a view with nothing new to show.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_ticking_for_test(program: &'static str, min_width: u32, frames: &[wire::Frame]) {
    seat_ticking_on_for_test(crate::contract::runner(), program, min_width, frames)
}

/// [`seat_ticking_for_test`], its code prepared on and seated by `runner`.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_ticking_on_for_test(
    runner: &Arc<dyn ViewRunner>,
    program: &'static str,
    min_width: u32,
    frames: &[wire::Frame],
) {
    assert!(!frames.is_empty(), "a view answers at least one frame");
    let mut data = String::new();
    let mut arms = String::new();
    for (nth, frame) in frames.iter().enumerate() {
        let (bytes, len) = wat_frame(frame);
        assert!(len < 4096, "test frames fit their pages");
        let at = 65536 + 4096 * nth as u32;
        data += &format!("(data (i32.const {at}) \"{bytes}\")\n");
        let tick = wire::abi::pack(at, len);
        // the nth tick answers the nth frame; the last frame answers the rest
        if nth + 1 < frames.len() {
            arms += &format!(
                "global.get $n i32.const {} i32.le_u if (result i64) i64.const {tick} else ",
                nth + 1
            );
        } else {
            arms += &format!("i64.const {tick}{}", " end".repeat(nth));
        }
    }
    let code = view::prepared_on_for_test(
        runner,
        format!(
            r#"(module
            (memory (export "memory") 2)
            (global $n (mut i32) (i32.const 0))
            {data}
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64)
                global.get $n i32.const 1 i32.add global.set $n
                {arms})
            (func (export "snapshot") (result i64) unreachable)
            (func (export "restore") (param i32 i32) (result i64) unreachable))"#
        ),
    );
    seat_code_on_for_test(runner, program, min_width, code);
}

/// An intent `program`'s seat `instance` will hand over on its next update,
/// as a `host.badge` or `link.open` request would leave it.
#[cfg(any(test, feature = "test-support"))]
pub fn intent_for_test(program: &str, instance: u64, intent: Intent) {
    let registry = registry().lock().unwrap();
    let seat = &registry[&(program, instance)];
    let Slot::Ready(view) = &mut seat.lock().unwrap().slot else {
        panic!("{program} is not seated");
    };
    view.intents.push(intent);
}

/// `program`'s seat `instance` listens for routes, as its view's
/// `host.route` subscription (request 99) would.
#[cfg(any(test, feature = "test-support"))]
pub fn listen_for_routes_for_test(program: &str, instance: u64) {
    let registry = registry().lock().unwrap();
    let Slot::Ready(view) = &mut registry[&(program, instance)].lock().unwrap().slot else {
        panic!("{program} is not seated");
    };
    view.route_subscriptions.push(99);
}

/// The routes `program`'s seat `instance` was handed, in order.
#[cfg(any(test, feature = "test-support"))]
pub fn routes_for_test(program: &str, instance: u64) -> Vec<String> {
    let registry = registry().lock().unwrap();
    let Slot::Ready(view) = &registry[&(program, instance)].lock().unwrap().slot else {
        panic!("{program} is not seated");
    };
    let routed = |event: &wire::Event| match event {
        wire::Event::Response {
            id,
            result: Ok(item),
            ..
        } if view.route_subscriptions.contains(id) => wire::methods::decode(item).ok(),
        _ => None,
    };
    view.ticked_with
        .iter()
        .flatten()
        .filter_map(routed)
        .collect()
}

/// Every drawn view of `program` gives itself `name`, as its manifest
/// would: the rail reads it on its next refresh.
#[cfg(any(test, feature = "test-support"))]
pub fn name_for_test(program: &str, name: &str) {
    for ((seated, _), seat) in lock(registry()).iter() {
        if *seated == program
            && let Slot::Ready(view) = &mut lock(seat).slot
        {
            view.name = name.into();
        }
    }
}

/// Every seated view of `program` declares `capabilities`, as its
/// manifest would.
#[cfg(any(test, feature = "test-support"))]
pub fn declare_for_test(program: &str, capabilities: &[Capability]) {
    for ((name, _), seat) in registry().lock().unwrap().iter() {
        if *name == program
            && let Slot::Ready(view) = &mut lock(seat).slot
        {
            view.capabilities = capabilities.to_vec();
        }
    }
}

/// `program`'s seat `instance` traps: its view keeps its tree but shows a
/// fault from now on, and the seat is woken as a trapping tick would.
#[cfg(any(test, feature = "test-support"))]
pub fn fault_for_test(program: &str, instance: u64) {
    let registry = registry().lock().unwrap();
    let mut locked = registry[&(program, instance)].lock().unwrap();
    let Slot::Ready(view) = &mut locked.slot else {
        panic!("{program} is not seated");
    };
    view.fault = Some("trapped for the test".into());
    locked.wake.send_replace(());
}

/// The events every tick of `program`'s seated view carried, in order:
/// what the view knew when it drew each frame.
#[cfg(any(test, feature = "test-support"))]
pub fn ticks_for_test(program: &str) -> Vec<Vec<wire::Event>> {
    seated_for_test(program, |view| view.ticked_with.clone())
}

/// Whether `program`'s seated view was last told it is on screen
/// (`host.visible`).
#[cfg(any(test, feature = "test-support"))]
pub fn visible_for_test(program: &str) -> bool {
    seated_for_test(program, |view| view.visible)
}

/// Where `program`'s seated view was last told its pane is
/// (`host.placement`).
#[cfg(any(test, feature = "test-support"))]
pub fn placement_for_test(program: &str) -> wire::methods::Placement {
    seated_for_test(program, |view| view.placement)
}

/// The body `program`'s seated view was last laid out in.
#[cfg(any(test, feature = "test-support"))]
pub fn viewport_for_test(program: &str) -> Option<(f32, f32)> {
    seated_for_test(program, |view| view.viewport_sent)
}

/// `read` off the view of `program`'s first seat; it panics unless seated.
#[cfg(any(test, feature = "test-support"))]
fn seated_for_test<T>(program: &str, read: impl FnOnce(&View) -> T) -> T {
    let registry = registry().lock().unwrap();
    let seat = registry
        .iter()
        .find(|((name, _), _)| *name == program)
        .map(|(_, seat)| seat.clone())
        .expect("a seat of the program");
    match &seat.lock().unwrap().slot {
        Slot::Ready(view) => read(view),
        _ => panic!("{program} is not seated"),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn seat_code_for_test(program: &'static str, min_width: u32, code: [u8; 32]) {
    seat_code_on_for_test(crate::contract::runner(), program, min_width, code)
}

/// [`seat_code_for_test`], its view seated by `runner`, which instantiates
/// every view the seat takes from then on.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_code_on_for_test(
    runner: &Arc<dyn ViewRunner>,
    program: &'static str,
    min_width: u32,
    code: [u8; 32],
) {
    let ready = || {
        let mut view = View::instantiate(program, runner, code, program).unwrap();
        view.min_width = min_width;
        Slot::Ready(Box::new(view))
    };
    let mut registry = registry().lock().unwrap();
    let mut seats: Vec<_> = registry
        .iter()
        .filter(|((name, _), _)| *name == program)
        .map(|(_, seat)| seat.clone())
        .collect();
    if seats.is_empty() {
        seats.push(Mounted::seat());
        registry.insert((program, 0), seats[0].clone());
    }
    for seat in seats {
        let mut locked = seat.lock().unwrap();
        locked.slot = ready();
        locked.wake.send_replace(());
    }
}

#[cfg(test)]
mod listing_tests {
    /// A seated view's manifest is what the product page reads; a program
    /// no seat holds has none.
    #[test]
    fn a_seated_views_listing_is_read_off_its_seat() {
        super::seat_for_test("listing-test", 640);
        let listing = super::listing("listing-test").map(|listing| listing.min_width);
        assert_eq!(listing, Some(640));
        assert_eq!(super::listing("listing-test-unseated"), None);
    }
}
