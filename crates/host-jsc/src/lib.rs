//! The iOS view runner (wasm-mobile SPEC §6): `contract::ViewRunner` over
//! JavaScriptCore. iOS lets no process of ours JIT, so a view's wasm runs in
//! WebKit's content process, in ONE hidden 1x1 `WKWebView` whose page
//! (`index.html` + `glue.js`, served by the app's own `dt://` scheme
//! handler) holds every seated view as a `WebAssembly.Instance`, and runs
//! every describe module too.
//!
//! - **Bounds.** JavaScriptCore has no fuel and enforces no view memory
//!   setting, and a page stuck in a wasm loop cannot be stopped from
//!   outside. So `prepare` takes the code through `view_meter::accepts` and
//!   `view_meter::meter`, which compile the fuel count and the
//!   `memory.view_bytes` setting into the module: the same numbers and the
//!   same faults as wasmtime. A view's call runs until it returns; a
//!   describe module's stops at `program.describe`'s fuel. The metered
//!   bytes are kept under the runner's cache directory as
//!   `<sha256>-<view_bytes>.metered.wasm` ([`metered`]), and served to the
//!   page by that name.
//! - **Calls.** Every call is one `callAsyncJavaScript` round trip, its
//!   bytes base64 both ways (SPEC §6.3), its completion on the main thread,
//!   where it lands on the seat's `Landing`. What the page answers means
//!   what the wasmtime runner's answers mean (`answer.rs`).
//! - **Death.** `webViewWebContentProcessDidTerminate` begins a new page:
//!   every call in flight lands `Fault::Runner`, so does every call on an
//!   instance of the dead page, and a seating asked meanwhile waits for the
//!   new page (`calls.rs`). The host revives each seat itself
//!   (`View::revive`); the runner never restores one.
//! - **Lockdown Mode.** The page answers `typeof WebAssembly` when it
//!   loads; where it is missing every `prepare` is refused as
//!   `Failure::Unsupported("WebAssembly")` (SPEC §6.6).
//!
//! Map: `answer.rs` (the page's answers, read), `calls.rs` (the page's
//! bookkeeping), `code.rs` (the prepared code, kept while anything holds
//! it), `glue.js` + `index.html` (the page), `runner.rs` (the
//! runner and its instances, iOS), `ffi.rs` (WebKit and UIKit through
//! objc2, the crate's only `unsafe`, iOS), `gate.rs` (the session driver
//! the simulator gate and its wasmtime reference share, feature `gate`).

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner reads it; it is tested everywhere")
)]
mod answer;
#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner keeps it; it is tested everywhere")
)]
mod calls;
#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner keeps it; it is tested everywhere")
)]
mod code;
#[cfg(target_os = "ios")]
mod ffi;
#[cfg(any(test, feature = "gate"))]
pub mod gate;
#[cfg(target_os = "ios")]
mod runner;

#[cfg(target_os = "ios")]
pub use runner::JscRunner;

use ducktape_host::contract::{CodeSource, Failure};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner's; tested everywhere")
)]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner's; tested everywhere")
)]
const DESCRIBE_MEMORY: u64 = 64 << 20;

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner's; tested everywhere")
)]
const DESCRIBE_FUEL: u64 = 250_000_000;

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner's; tested everywhere")
)]
const VIEW_FUEL: i64 = i64::MAX;

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner's; tested everywhere")
)]
fn file_name(hash: &[u8; 32], view_bytes: u64) -> String {
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{hex}-{view_bytes}.metered.wasm")
}

#[cfg_attr(
    not(target_os = "ios"),
    allow(dead_code, reason = "the iOS runner's; tested everywhere")
)]
fn metered(
    cache: Option<&Path>,
    hash: &[u8; 32],
    code: &[u8],
    view_bytes: u64,
) -> Result<(Vec<u8>, CodeSource), Failure> {
    let path = cache.map(|dir| dir.join(file_name(hash, view_bytes)));
    if let Some(bytes) = path.as_ref().and_then(|path| std::fs::read(path).ok()) {
        return Ok((bytes, CodeSource::Disk));
    }
    let metered = view_meter::meter(code, view_bytes).map_err(|refusal| match refusal {
        view_meter::Refusal::Unsupported(what) => Failure::Unsupported(what),
        refusal @ view_meter::Refusal::Memory { .. } => Failure::Runner(refusal.to_string()),
    })?;
    if let Some(path) = &path {
        let kept = path.with_extension("partial");
        let written = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
            .and_then(|()| std::fs::write(&kept, &metered))
            .and_then(|()| std::fs::rename(&kept, path));
        if let Err(error) = written {
            tracing::warn!(target: "ducktape::jsc", reason = "view_code_not_kept", %error);
        }
    }
    Ok((metered, CodeSource::Cold))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        (func (export "init"))
        (func (export "tick") (param i32 i32) (result i64) i64.const 0)
        (func (export "snapshot") (result i64) i64.const 0)
        (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#;

    #[test]
    fn metered_code_is_kept_and_read_back_by_input_and_budget() {
        let dir = std::env::temp_dir().join(format!("host-jsc-cache-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        let code = wat::parse_str(VIEW).unwrap();
        let hash = [3; 32];
        let (cold, source) = metered(Some(&dir), &hash, &code, 4 << 16).unwrap();
        assert_eq!(source, CodeSource::Cold);
        assert_eq!(cold, view_meter::meter(&code, 4 << 16).unwrap());
        let (disk, source) = metered(Some(&dir), &hash, &code, 4 << 16).unwrap();
        assert_eq!((disk, source), (cold, CodeSource::Disk));
        let (_, source) = metered(Some(&dir), &hash, &code, 8 << 16).unwrap();
        assert_eq!(source, CodeSource::Cold);
        assert!(dir.join(file_name(&hash, 4 << 16)).is_file());
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|entry| !entry
                .unwrap()
                .path()
                .to_string_lossy()
                .ends_with(".partial")),
            "a file kept half-written"
        );
        let _gone = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_the_runners_do_not_take_is_unsupported_as_on_wasmtime() {
        let threads = wat::parse_str(
            r#"(module (memory 1 1 shared) (func (export "f") i32.const 0 i32.atomic.load drop))"#,
        )
        .unwrap();
        assert!(matches!(
            metered(None, &[0; 32], &threads, 64 << 20),
            Err(Failure::Unsupported(_))
        ));
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn a_describe_module_is_bounded_as_program_describe_bounds_it() {
        assert_eq!(DESCRIBE_MEMORY, describe::host::MEMORY as u64);
        assert_eq!(DESCRIBE_FUEL, describe::host::FUEL);
    }
}
