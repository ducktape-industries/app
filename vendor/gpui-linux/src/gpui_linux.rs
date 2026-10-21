#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{current_platform, linux_platform};

/// [`set_clipboard_wait`]'s function.
static CLIPBOARD_WAIT: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Sets the function a clipboard read (X11 or Wayland) calls on the main
/// thread while it waits for the owner's answer, between its polls. An owner
/// in this process that answers only while an event loop on the main thread
/// turns would otherwise wait out every read (as GTK, whose clipboard wait
/// runs a nested main loop). `wait` must not block. The first one set stays.
pub fn set_clipboard_wait(wait: fn()) {
    let _ = CLIPBOARD_WAIT.set(wait);
}
