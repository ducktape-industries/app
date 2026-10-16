//! The view runners this crate carries: wasmtime's (`WasmtimeRunner`,
//! feature `wasmtime`), the desktop's and Android's; and, in the tests, the
//! landed-later gate's (`deferred::DeferredRunner`). iOS runs views in
//! JavaScriptCore (`crates/host-jsc`), with this crate's `wasmtime` off.

#[cfg(feature = "wasmtime")]
mod wasmtime;

#[cfg(feature = "wasmtime")]
pub use self::wasmtime::{WasmtimeInstance, WasmtimeRunner};

#[cfg(test)]
pub mod deferred;
