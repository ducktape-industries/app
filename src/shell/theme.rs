//! The product theme is the host core's (`ducktape_host::theme`); what is
//! the desktop's alone is the room macOS's traffic lights take.

pub(crate) use ducktape_host::theme::{FAMILY_MONO, FAMILY_UI, configure_native_theme};

/// The room macOS's traffic lights take at a title bar's left end, when
/// this window draws the bar itself (macOS, not fullscreen); `None`
/// elsewhere, where the system draws the title bar.
pub(super) fn traffic_lights(window: &gpui_kit::Window) -> Option<f32> {
    (cfg!(target_os = "macos") && !window.is_fullscreen()).then_some(78.)
}
