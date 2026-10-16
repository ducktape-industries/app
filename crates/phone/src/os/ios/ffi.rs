//! The iOS launcher's entry. `phone/ios/App.swift`'s `main` calls
//! [`ducktape_phone_main`] with the app container's Documents and Caches
//! paths; it never returns: gpui_ios runs `UIApplicationMain` with its own
//! delegate, and the host starts on the main thread once UIKit is up.
//!
//! Foreign functions defined here, called on the main thread:
//! - `ducktape_phone_main(documents, caches)`, once: two NUL-terminated
//!   UTF-8 paths the caller owns, read before anything else runs.
//! - `ducktape_phone_back()`: the left edge's swipe the launcher follows,
//!   as Back.
//! - `ducktape_phone_back_drag(x)`: that swipe under way, `x` points in;
//!   `ducktape_phone_back_cancel()`: let go without going back.
//!
//! No `catch_unwind`: the phone profile builds with `panic = "abort"`
//! (gpui_ios's ObjC entries have no unwind guard, `README.ducktape.md`).
#![allow(unsafe_code, reason = "the C ABI entry the Swift launcher calls")]

use std::ffi::{CStr, c_char};
use std::path::PathBuf;

/// Runs the phone: the host over `documents` and `caches`, its one window.
///
/// # Safety
/// `documents` and `caches` are NUL-terminated strings alive for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ducktape_phone_main(documents: *const c_char, caches: *const c_char) {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    // SAFETY: the caller's contract above.
    let (documents, caches) = unsafe { (path(documents), path(caches)) };
    let (Some(documents), Some(caches)) = (documents, caches) else {
        tracing::error!(target: "ducktape::phone", "the launcher passed no app container");
        std::process::abort();
    };
    let app = gpui_kit::Application::with_platform(gpui_ios::current_platform(false))
        .with_assets(crate::assets::PhoneAssets);
    // a scene's URL contexts, at launch and later (`duck://`, `ducktape://`,
    // Info.plist `CFBundleURLTypes`)
    app.on_open_urls(|urls| urls.into_iter().for_each(crate::os::link));
    app.run(move |cx| {
        crate::os::start(documents, caches, cx);
        crate::shell::open_window(cx);
        cx.activate(true);
    });
}

/// Back, from the launcher's left edge swipe (`App.swift`): gpui_ios hands
/// no edge gesture to the app, so Back has no other source on iOS. Popped
/// as Android's Back is.
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back() {
    crate::os::send(ducktape_host::contract::Signal::Back);
}

/// The left edge's swipe under way (`App.swift`), `x` points in from where
/// it started: the screen on top follows the finger. Its release is
/// [`ducktape_phone_back`] or [`ducktape_phone_back_cancel`].
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back_drag(x: f64) {
    crate::os::send(ducktape_host::contract::Signal::BackDrag(x as f32));
}

/// The left edge's swipe let go short of Back: the screen on top goes back
/// into place.
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back_cancel() {
    crate::os::send(ducktape_host::contract::Signal::BackDragCancelled);
}

/// A path from the launcher; `None` for null or one that is not UTF-8.
///
/// # Safety
/// `raw` is null or a NUL-terminated string alive for the call.
unsafe fn path(raw: *const c_char) -> Option<PathBuf> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: non-null, NUL-terminated and alive (the caller's contract).
    let text = unsafe { CStr::from_ptr(raw) }.to_str().ok()?;
    Some(PathBuf::from(text))
}
