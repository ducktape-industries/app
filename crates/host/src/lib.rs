//! The blind host core: what the desktop shell and the phone shell share.
//! It reaches a node, reads its roster, seats each program's view and draws
//! what the view sends; it knows no program by name and names no shell.
//!
//! - [`render`]: `ViewTree`, one wire tree drawn as GPUI elements.
//! - [`runtime`]: seats, views, the kernel relay, replies, notify, store,
//!   clipboard, the roster.
//! - [`runner`]: the wasmtime view runner (`contract::ViewRunner`), feature
//!   `wasmtime` (on by default; off on iOS, whose runner is
//!   `crates/host-jsc`).
//! - [`backend`]: the node client and the frames the seated key signs, a
//!   view's wasm out of its program's blob, prefs, recent nodes, the app's
//!   own directories.
//! - [`fonts`], [`theme`], [`a11y`]: faces and fallbacks, the product theme,
//!   role/name/keyboard helpers for native controls.
//! - [`perf`]: the performance journal.
//! - [`ax`] (feature `ax-door`, and every test): the AX test door.
//! - [`contract`]: THE interface file: what a shell gives the host
//!   (`OsServices`, `Boot`), feeds it (`Inbound`), and what a view runner
//!   implements. Read it first.
//! - [`session`]: the entities every shell draws from (`Session`,
//!   `Account`, `Chain`, `Screen`, `Rail`, `Seats`, `Notifications`,
//!   `Prefs`, `Toast`).
//! - [`start`]: `start(Boot) -> (Host, Inbound)`, the stores and entities
//!   made, the inbound drain spawned.
//!
//! `ARCHITECTURE.md` at the repository root is the guide; its map names
//! these modules by their old `src/` paths until its phone section lands.

pub mod a11y;
// the AX test door: in a build with the `ax-door` feature only (kit and qa),
// and in the tests, which read the trees it reads
#[cfg(any(test, feature = "ax-door"))]
pub mod ax;
pub mod backend;
pub mod contract;
pub mod fonts;
pub mod perf;
pub mod render;
pub mod runner;
pub mod runtime;
pub mod session;
pub mod start;
pub mod theme;
