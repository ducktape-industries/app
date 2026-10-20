//! Android: longbridge gpui-mobile's host-driven path (the fork's
//! `vendor/gpui-pre-mobile`, `android::host`). `phone/android`'s one
//! Activity owns a `SurfaceView` and hands gm its surface, touches, keys and
//! lifecycle through the JNI entries in [`jni`]; gm runs gpui on a
//! process-lived thread of its own, where the host starts at the first
//! surface.
//!
//! gm's host path answers no insets (its `AndroidWindow` implements none of
//! `PlatformWindow::insets`), so the Activity's window-insets listener hands
//! them in here, in pixels, and [`insets`] answers them in the window's
//! points.

pub mod jni;

use ducktape_host::contract::SoftKeyboard;
use gpui_kit::{Edges, Window, WindowInsets, px};
use gpui_mobile::KeyboardType;
use std::sync::atomic::{AtomicI32, Ordering};

static SAFE: [AtomicI32; 4] = [const { AtomicI32::new(0) }; 4];
static IME: AtomicI32 = AtomicI32::new(0);

fn set_insets(safe: [i32; 4], ime: i32) {
    for (kept, value) in SAFE.iter().zip(safe) {
        kept.store(value, Ordering::Relaxed);
    }
    IME.store(ime, Ordering::Relaxed);
    super::send(ducktape_host::contract::Signal::InsetsChanged);
}

/// The insets the Activity handed in, in `window`'s points.
pub fn insets(window: &Window) -> WindowInsets {
    let scale = window.scale_factor();
    let points = |pixels: &AtomicI32| px(pixels.load(Ordering::Relaxed) as f32 / scale);
    let [top, right, bottom, left] = &SAFE;
    WindowInsets {
        safe_area: Edges {
            top: points(top),
            right: points(right),
            bottom: points(bottom),
            left: points(left),
        },
        ime: Edges {
            bottom: points(&IME),
            ..Edges::default()
        },
    }
}

/// This phone as the Account tab names it (Expo `platform.phone`, React
/// Native's `Build.MODEL` and `Build.VERSION.RELEASE`): the model and the
/// system's version, from the properties those two read.
pub fn phone() -> String {
    let info = gpui_mobile::packages::device_info::get_device_info().ok();
    let (model, release) = info.map_or_else(Default::default, |info| (info.model, info.os_version));
    let model = if model.is_empty() {
        "Android".to_owned()
    } else {
        model
    };
    format!("{model} · Android {release}")
}

/// Raises the keyboard the focused field asks for, or lowers it.
pub fn soft_keyboard(want: SoftKeyboard) {
    use gpui_kit::VirtualKeyboard as Asked;
    match want {
        SoftKeyboard::Show(field) => {
            gpui_mobile::show_keyboard_with_type(match field.virtual_keyboard {
                Asked::Text | Asked::Search => KeyboardType::Default,
                Asked::Numeric => KeyboardType::NumberPad,
                Asked::Decimal => KeyboardType::Decimal,
                Asked::Tel => KeyboardType::Phone,
                Asked::Email => KeyboardType::EmailAddress,
                Asked::Url => KeyboardType::URL,
            })
        }
        SoftKeyboard::Hide => gpui_mobile::hide_keyboard(),
    }
}
