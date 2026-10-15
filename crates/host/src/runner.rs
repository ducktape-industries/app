//! The view runners this crate carries: wasmtime's ([`WasmtimeRunner`]),
//! the desktop's and Android's; and, in the tests, the landed-later gate's
//! ([`deferred::DeferredRunner`]).

mod wasmtime;

pub use self::wasmtime::{WasmtimeInstance, WasmtimeRunner};

#[cfg(test)]
pub mod deferred;
