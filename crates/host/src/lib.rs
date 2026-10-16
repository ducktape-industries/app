//! The blind host core: what the desktop shell and the phone shell share.
//! It reaches a node, reads its roster, seats each program's view and draws
//! what the view sends; it knows no program by name and names no shell.
//!
//! - [`render`]: `ViewTree`, one wire tree drawn as GPUI elements.
//! - [`runtime`]: seats, views (wasmtime), the kernel relay, replies,
//!   notify, store, clipboard, the roster.
//! - [`backend`]: the node client and the frames the seated key signs, a
//!   view's wasm out of its program's blob, prefs, recent nodes, the app's
//!   own directories.
//! - [`fonts`], [`theme`], [`a11y`]: faces and fallbacks, the product theme,
//!   role/name/keyboard helpers for native controls.
//! - [`perf`]: the performance journal.
//! - [`ax`] (feature `ax-door`, and every test): the AX test door.
//!
//! `ARCHITECTURE.md` at the repository root is the guide; its map names
//! these modules by their old `src/` paths until its phone section lands.
//!
//! Without the `ui` feature (default on) the crate is what a host with no
//! screen runs (`crates/pushd`): [`runtime`] without its seats' entities or
//! the clipboard, [`backend`], [`contract`] and [`perf`]; nothing that
//! draws, and no gpui-kit in its graph.

// without `ui`, what only the window half calls (a seat's entity, the
// clipboard's mount, the shell's session wiring) is never called
#![cfg_attr(not(feature = "ui"), allow(dead_code))]

#[cfg(feature = "ui")]
pub mod a11y;
// the AX test door: in a build with the `ax-door` feature only (kit and qa),
// and in the tests, which read the trees it reads
#[cfg(any(test, feature = "ax-door"))]
pub mod ax;
pub mod backend;
pub mod contract;
#[cfg(feature = "ui")]
pub mod fonts;
pub mod perf;
#[cfg(feature = "ui")]
pub mod render;
pub mod runtime;
#[cfg(feature = "ui")]
pub mod session;
#[cfg(feature = "ui")]
pub mod start;
#[cfg(feature = "ui")]
pub mod theme;
