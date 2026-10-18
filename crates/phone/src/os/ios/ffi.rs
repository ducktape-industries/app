//! The iOS launcher's entry. `phone/ios/App.swift`'s `main` calls
//! [`ducktape_phone_main`] with the app container's Documents and Caches
//! paths; it never returns: gpui_ios runs `UIApplicationMain` with its own
//! delegate, and the host starts on the main thread once UIKit is up.
//!
//! Foreign functions defined here, called on the main thread:
//! - `ducktape_phone_main(documents, caches)`, once: two NUL-terminated
//!   UTF-8 paths the caller owns, read before anything else runs.
//! - `ducktape_phone_back_begins(x, y, vx, vy, held) -> begins`: whether
//!   the launcher's swipe from anywhere starts (the left edge's always
//!   does). Called from inside UIKit's touch handling.
//! - `ducktape_phone_back_drag(x, speed)`: a swipe back under way, `x`
//!   points in, `speed` points per second; `ducktape_phone_back_let_go()`:
//!   let go, the shell judges whether it goes back;
//!   `ducktape_phone_back_cancel()`: ended without going back.
//! - `ducktape_phone_link(url)`: a call's Answer (CallKit), its link.
//! - `ducktape_phone_push_token(kind, token)`: PushKit's token, as hex.
//! - `ducktape_phone_notice(payload, at, front, opened)`: a program's
//!   notice the OS showed, delivered or opened.
//! - `ducktape_phone_call_audio(active)`: CallKit's audio session up or down.
//!
//! Swift's entries called from here (`App.swift`):
//! - `ducktape_phone_call_voice(active)`: the call's voice I/O made or gone
//!   (`ducktape_av::voice`): CallKit's answered call ends with it. Any
//!   thread.
//! - `ducktape_phone_remote_notification(json) -> taken`: a background push
//!   the app delegate (gpui_ios) handed over, its payload as JSON: a
//!   withdrawn call's ring stops. Main thread.
//! - `ducktape_phone_withdraw_notice(tag)`: the OS's notice under the push
//!   server's `tag` out of its list. Any thread.
//!
//! The regular remote-notification token (kind `apns`, the one a withdraw
//! is pushed to) comes through gpui_ios's delegate hooks, set on the
//! platform before UIKit runs; the launcher asks UIKit to register.
//!
//! No `catch_unwind`: the phone profile builds with `panic = "abort"`
//! (gpui_ios's ObjC entries have no unwind guard, `README.ducktape.md`).
#![allow(unsafe_code, reason = "the C ABI entry the Swift launcher calls")]

use std::ffi::{CStr, c_char};
use std::path::PathBuf;

#[cfg(feature = "call")]
unsafe extern "C" {
    fn ducktape_phone_call_voice(active: u8);
}

unsafe extern "C" {
    fn ducktape_phone_remote_notification(json: *const c_char) -> u8;
    fn ducktape_phone_withdraw_notice(tag: *const c_char);
}

/// The OS's notice under `tag` out of its list, through the launcher
/// (`crate::os::withdraw_notice`).
pub(crate) fn withdraw_notice(tag: &str) {
    match std::ffi::CString::new(tag) {
        // SAFETY: a Swift `@_cdecl` entry taking a C string alive for the
        // call; any thread (it hands on to the main queue).
        Ok(tag) => unsafe { ducktape_phone_withdraw_notice(tag.as_ptr()) },
        Err(_) => tracing::error!(target: "ducktape::phone", "a notice's tag held a NUL"),
    }
}

/// A background push, as the app delegate hands it over: to the
/// launcher, which knows the carrier's payload and answers whether it
/// took it (a withdrawn call's ring stopped).
fn remote_notification(json: String) -> bool {
    match std::ffi::CString::new(json) {
        // SAFETY: a Swift `@_cdecl` entry taking a C string alive for the
        // call, on the main thread (where UIKit delivers the push).
        Ok(json) => unsafe { ducktape_phone_remote_notification(json.as_ptr()) != 0 },
        Err(_) => {
            tracing::error!(target: "ducktape::phone", "a push's payload held a NUL");
            false
        }
    }
}

/// The call's voice I/O made (`true`) or gone, told to the launcher's
/// CallKit call.
#[cfg(feature = "call")]
fn call_voice(alive: bool) {
    // SAFETY: a Swift `@_cdecl` entry taking one byte, any thread.
    unsafe { ducktape_phone_call_voice(u8::from(alive)) }
}

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
    let (documents, caches) = unsafe { (text(documents), text(caches)) };
    let (documents, caches) = (documents.map(PathBuf::from), caches.map(PathBuf::from));
    let (Some(documents), Some(caches)) = (documents, caches) else {
        tracing::error!(target: "ducktape::phone", "the launcher passed no app container");
        std::process::abort();
    };
    #[cfg(feature = "call")]
    ducktape_av::voice::on_life(call_voice);
    let platform = gpui_ios::IosPlatform::new();
    platform.on_remote_notification_token(Box::new(|token| match token {
        Ok(token) => {
            let token: String = token.iter().map(|byte| format!("{byte:02x}")).collect();
            crate::os::push_token("apns", token);
        }
        // without the `aps-environment` entitlement, every launch: no
        // token, so a withdraw never reaches this phone (the ring's
        // `until` ends it)
        Err(why) => {
            tracing::warn!(target: "ducktape::phone", %why, "no remote-notification token");
            crate::os::push_token("apns", String::new());
        }
    }));
    platform.on_remote_notification(Box::new(remote_notification));
    let app = gpui_kit::Application::with_platform(std::rc::Rc::new(platform))
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

/// Whether the launcher's swipe from anywhere on the screen begins
/// (`App.swift`, as UIKit is about to start it): its finger came down at
/// (`x`, `y`) in the window's points, moves at (`vx`, `vy`) points per
/// second, and rested `held` seconds before it moved; 1 when it begins.
/// Read against what the last draw showed (`shell::swipe`): UIKit asks
/// from inside its touch handling, where the app is not reached.
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back_begins(x: f64, y: f64, vx: f64, vy: f64, held: f64) -> u8 {
    use gpui_kit::{point, px};
    let held = std::time::Duration::try_from_secs_f64(held).unwrap_or(std::time::Duration::MAX);
    let start = point(px(x as f32), px(y as f32));
    let moving = point(px(vx as f32), px(vy as f32));
    u8::from(crate::shell::swipe::begins(start, moving, held))
}

/// A swipe back under way (`App.swift`: from the left edge, or from
/// anywhere it began), `x` points in from where it started, moving `speed`
/// points per second to the right: the screen on top follows the finger.
/// gpui_ios hands no gesture of the system's to the app, so Back has no
/// other source on iOS. It ends with [`ducktape_phone_back_let_go`] or
/// [`ducktape_phone_back_cancel`].
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back_drag(x: f64, speed: f64) {
    crate::os::send(ducktape_host::contract::Signal::BackDrag {
        x: x as f32,
        speed: speed as f32,
    });
}

/// The swipe let go, where and as fast as its last drag said: Back when it
/// went far or fast enough, else the screen on top goes back into place.
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back_let_go() {
    crate::os::send(ducktape_host::contract::Signal::BackLetGo);
}

/// The swipe ended without going back (UIKit cancelled it, or a presented
/// controller took it): the screen on top goes back into place.
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_back_cancel() {
    crate::os::send(ducktape_host::contract::Signal::BackDragCancelled);
}

/// A call answered in CallKit (`App.swift` `Ring`): its `duck://` link,
/// opened as a link the OS hands over is.
///
/// # Safety
/// `url` is null or a NUL-terminated string alive for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ducktape_phone_link(url: *const c_char) {
    // SAFETY: the caller's contract above.
    match unsafe { text(url) } {
        Some(url) => {
            tracing::info!(target: "ducktape::phone", %url, "a call was answered: its link opens");
            crate::os::link(url);
        }
        None => tracing::error!(target: "ducktape::phone", "a call's link was not UTF-8"),
    }
}

/// This device's push token of `kind` (`voip`: PushKit's, as hex), for the
/// device registry; empty when the OS withdrew it.
///
/// # Safety
/// `kind` and `token` are null or NUL-terminated strings alive for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ducktape_phone_push_token(kind: *const c_char, token: *const c_char) {
    // SAFETY: the caller's contract above.
    let (kind, token) = unsafe { (text(kind), text(token)) };
    if let (Some(kind), Some(token)) = (kind, token) {
        crate::os::push_token(&kind, token);
    }
}

/// A program's notice the OS showed (`App.swift` `Ring`): its payload as
/// JSON, the Unix seconds it was shown at; `front` when it came while the
/// app was in front (the OS shows nothing then), `opened` when the person
/// opened it. For the Notifications tab ([`crate::os::os_notice`]).
///
/// # Safety
/// `payload` is null or a NUL-terminated string alive for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ducktape_phone_notice(
    payload: *const c_char,
    at: f64,
    front: u8,
    opened: u8,
) {
    // SAFETY: the caller's contract above.
    match unsafe { text(payload) } {
        Some(payload) => crate::os::os_notice(&payload, at as i64, front != 0, opened != 0),
        None => tracing::error!(target: "ducktape::phone", "an OS notice's payload was not UTF-8"),
    }
}

/// CallKit's audio session up (non-zero) or down, a call answered or ended.
// ponytail: logged only; the call's voice (`ducktape_av`, lane B-HOST)
// opens on this session from here.
#[unsafe(no_mangle)]
pub extern "C" fn ducktape_phone_call_audio(active: u8) {
    tracing::info!(target: "ducktape::phone", active = active != 0, "CallKit's audio session");
}

/// A string from the launcher; `None` for null or one that is not UTF-8.
///
/// # Safety
/// `raw` is null or a NUL-terminated string alive for the call.
unsafe fn text(raw: *const c_char) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: non-null, NUL-terminated and alive (the caller's contract).
    let text = unsafe { CStr::from_ptr(raw) }.to_str().ok()?;
    Some(text.to_owned())
}
