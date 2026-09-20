//! Whether this process may open the camera or the microphone, asked BEFORE
//! the device is. On macOS the consent prompt is raised by the system daemon
//! the open goes through, and until the person answers it the opener is
//! parked: coreaudiod holds the microphone's HAL call, and the camera's
//! capture session delivers no frame at all (a refusal in System Settings
//! parks it the same way, forever). The session's teardown joins that
//! thread from the window thread, so the whole app froze behind one
//! unanswered prompt (observed on macOS 27, A2). Read the status first: an
//! unasked device is asked — the prompt is the system's, shown once — and
//! this open is refused with the sentence to act on; a refused device names
//! the Settings pane. The screen is gated the same way: without Screen
//! Recording consent `CGDisplayCreateImage` hands back the wallpaper with no
//! windows on it and no error, so an unconsented share would go out looking
//! like it worked. Everywhere else the platform has no such gate.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Device {
    Camera,
    Microphone,
    Screen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Authorized,
    NotDetermined,
    Denied,
}

/// `Ok` when the device may be opened now. Otherwise the reason, and — the
/// first time — the system's consent prompt.
pub(crate) fn gate(device: Device) -> Result<(), String> {
    let access = platform::status(device);
    tracing::info!(target: "ducktape::app", reason = "media_access", ?device, ?access, "media device access");
    match access {
        Access::Authorized => Ok(()),
        Access::NotDetermined => {
            platform::request(device);
            Err(refusal(device, access))
        }
        Access::Denied => Err(refusal(device, access)),
    }
}

/// The sentence for a device that will not open: what the person does next.
// ponytail: the first grant asks for a second toggle/rejoin; opening the
// device from the consent callback would save it, at a thread the callback
// has to reach.
pub(crate) fn refusal(device: Device, access: Access) -> String {
    let (name, pane) = match device {
        Device::Camera => ("camera", "Camera"),
        Device::Microphone => ("microphone", "Microphone"),
        Device::Screen => ("screen", "Screen & System Audio Recording"),
    };
    match access {
        Access::Authorized => String::new(),
        // the screen's preflight cannot tell "never asked" from "refused",
        // and its grant only takes effect after a relaunch — one sentence
        Access::NotDetermined if device == Device::Screen => format!(
            "allow Ducktape to record the screen — in the prompt, or in System Settings → Privacy & Security → {pane} — then open Ducktape again and share"
        ),
        Access::NotDetermined => {
            format!("allow Ducktape to use the {name} in the prompt, then turn it on again")
        }
        Access::Denied => format!(
            "{name} access is off for Ducktape: turn it on in System Settings → Privacy & Security → {pane}"
        ),
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{Access, Device};
    use objc2::runtime::Bool;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    // CoreGraphics is already linked for `CGDisplayCreateImage`; the
    // core-graphics crate does not bind these two.
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    /// `AVMediaTypeVideo` / `AVMediaTypeAudio`.
    fn media_type(device: Device) -> objc2::rc::Retained<NSString> {
        NSString::from_str(match device {
            Device::Camera => "vide",
            Device::Microphone => "soun",
            Device::Screen => unreachable!("the screen is not an AVCaptureDevice"),
        })
    }

    /// `AVCaptureDevice.authorizationStatus(for:)`: 0 notDetermined,
    /// 1 restricted, 2 denied, 3 authorized. The screen has no
    /// "not determined": TCC only tells a preflight yes or no, and the
    /// request below prompts once per install — the sentence covers both.
    pub(super) fn status(device: Device) -> Access {
        if device == Device::Screen {
            // SAFETY: a plain CoreGraphics query, valid on any thread.
            return if unsafe { CGPreflightScreenCaptureAccess() } {
                Access::Authorized
            } else {
                Access::NotDetermined
            };
        }
        let cls = class!(AVCaptureDevice);
        // SAFETY: a class method taking one NSString, valid on any thread.
        let status: isize =
            unsafe { msg_send![cls, authorizationStatusForMediaType: &*media_type(device)] };
        match status {
            3 => Access::Authorized,
            0 => Access::NotDetermined,
            _ => Access::Denied,
        }
    }

    /// `AVCaptureDevice.requestAccess(for:)`: the system prompt, answered on
    /// its own thread; the answer is logged and the next open reads it.
    pub(super) fn request(device: Device) {
        if device == Device::Screen {
            // SAFETY: a plain CoreGraphics call; it returns at once and the
            // prompt (when the system shows one) is answered elsewhere.
            let granted = unsafe { CGRequestScreenCaptureAccess() };
            tracing::info!(target: "ducktape::app", reason = "media_access_answered", ?device, granted, "screen recording access requested");
            return;
        }
        let handler = block2::RcBlock::new(move |granted: Bool| {
            tracing::info!(target: "ducktape::app", reason = "media_access_answered", ?device, granted = granted.as_bool(), "media consent prompt answered");
        });
        let cls = class!(AVCaptureDevice);
        // SAFETY: the block is copied by the framework and outlives this call.
        unsafe {
            let _: () = msg_send![cls, requestAccessForMediaType: &*media_type(device), completionHandler: &*handler];
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{Access, Device};

    pub(super) fn status(_device: Device) -> Access {
        Access::Authorized
    }

    pub(super) fn request(_device: Device) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unasked_device_is_refused_with_the_prompt_to_answer() {
        assert_eq!(
            refusal(Device::Camera, Access::NotDetermined),
            "allow Ducktape to use the camera in the prompt, then turn it on again"
        );
    }

    #[test]
    fn an_unasked_screen_says_to_relaunch_after_the_grant() {
        assert_eq!(
            refusal(Device::Screen, Access::NotDetermined),
            "allow Ducktape to record the screen — in the prompt, or in System Settings → Privacy & Security → Screen & System Audio Recording — then open Ducktape again and share"
        );
    }

    #[test]
    fn a_refused_device_names_its_settings_pane() {
        assert_eq!(
            refusal(Device::Microphone, Access::Denied),
            "microphone access is off for Ducktape: turn it on in System Settings → Privacy & Security → Microphone"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn platforms_without_consent_open_at_once() {
        assert_eq!(gate(Device::Camera), Ok(()));
        assert_eq!(gate(Device::Microphone), Ok(()));
        assert_eq!(gate(Device::Screen), Ok(()));
    }
}
