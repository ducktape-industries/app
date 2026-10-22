//! macOS: CEF is not linked. Its framework is loaded first, in the app and in
//! every child: beside an unbundled binary, whose children are itself, or in
//! the bundle's `Frameworks/`, whose children are its helper apps.

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const FRAMEWORK: &str = "Chromium Embedded Framework.framework";

pub(crate) fn framework() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.to_path_buf(),
        dir.join("../Frameworks"),
        dir.join("../../.."),
    ]
    .into_iter()
    .map(|at| at.join(FRAMEWORK))
    .find(|path| path.is_dir())
}

pub(crate) fn subprocess() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent()?.join(FRAMEWORK).is_dir().then_some(exe)
}

pub(crate) fn load() -> bool {
    static LOADED: OnceLock<bool> = OnceLock::new();
    *LOADED.get_or_init(|| {
        let Some(library) =
            framework().and_then(|dir| c_path(&dir.join("Chromium Embedded Framework")))
        else {
            return false;
        };
        // SAFETY: a NUL-terminated path that outlives the call
        cef::load_library(Some(unsafe { &*library.as_ptr() })) == 1
    })
}

pub(crate) fn sandbox(args: &cef::MainArgs) -> bool {
    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    const RTLD_NOW: c_int = 2;
    let Some(library) =
        framework().and_then(|dir| c_path(&dir.join("Libraries/libcef_sandbox.dylib")))
    else {
        return false;
    };
    // SAFETY: NUL-terminated strings; the symbol is CEF's
    // `void* cef_sandbox_initialize(int argc, char** argv)`, and its context
    // is kept for the child's life (never destroyed)
    unsafe {
        let handle = dlopen(library.as_ptr(), RTLD_NOW);
        if handle.is_null() {
            return false;
        }
        let initialize = dlsym(handle, c"cef_sandbox_initialize".as_ptr());
        if initialize.is_null() {
            return false;
        }
        let initialize: extern "C" fn(c_int, *mut *mut c_char) -> *mut c_void =
            std::mem::transmute(initialize);
        !initialize(args.argc, args.argv).is_null()
    }
}

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

pub(crate) fn key(key: &str) -> Option<(i32, u16)> {
    const LETTERS: [i32; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F,
        0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [i32; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    Some(match key {
        "backspace" => (0x33, 0x7F),
        "tab" => (0x30, 0x09),
        "enter" => (0x24, 0x0D),
        "escape" => (0x35, 0x1B),
        "space" => (0x31, 0x20),
        "pageup" => (0x74, 0xF72C),
        "pagedown" => (0x79, 0xF72D),
        "end" => (0x77, 0xF72B),
        "home" => (0x73, 0xF729),
        "left" => (0x7B, 0xF702),
        "up" => (0x7E, 0xF700),
        "right" => (0x7C, 0xF703),
        "down" => (0x7D, 0xF701),
        "delete" => (0x75, 0xF728),
        _ => match key.as_bytes() {
            [byte @ b'a'..=b'z'] => (LETTERS[(byte - b'a') as usize], *byte as u16),
            [byte @ b'0'..=b'9'] => (DIGITS[(byte - b'0') as usize], *byte as u16),
            _ => return None,
        },
    })
}
