//! macOS: which cameras the system lists and which way each looks, what a
//! listed microphone or speaker is (Core Audio), and what a person is told
//! of a refused device (System Settings' privacy panes). The permission
//! and the camera itself are AVFoundation's, shared with iOS
//! ([`crate::apple`]).
use std::ptr::NonNull;

use objc2::rc::autoreleasepool;
use objc2_av_foundation::{
    AVCaptureDevice, AVCaptureDevicePosition, AVCaptureDeviceTypeBuiltInWideAngleCamera,
};
use objc2_foundation::NSString;

use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertyScope, AudioObjectPropertySelector, kAudioDevicePropertyDataSource,
    kAudioDevicePropertyTransportType, kAudioDeviceTransportTypeBluetooth,
    kAudioDeviceTransportTypeBluetoothLE, kAudioDeviceTransportTypeBuiltIn,
    kAudioDeviceTransportTypeDisplayPort, kAudioDeviceTransportTypeHDMI,
    kAudioHardwarePropertyTranslateUIDToDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioObjectUnknown,
};

use crate::backend::{AudioKind, CameraDevice, Device, Facing};

pub(crate) use crate::apple::{camera_allowed, microphone_allowed, open};

// ---------- what a microphone or speaker is ----------

/// What a listed microphone or speaker is, as Core Audio says: by its
/// transport, and a built-in one's data source (its speakers or the
/// headphone jack).
pub(crate) fn audio_kind(device: &Device, input: bool) -> AudioKind {
    let Some(uid) = device.id.strip_prefix("coreaudio:") else {
        return AudioKind::Other;
    };
    let uid = NSString::from_str(uid);
    let system = kAudioObjectSystemObject as AudioObjectID;
    let global = kAudioObjectPropertyScopeGlobal;
    let found = property(
        system,
        kAudioHardwarePropertyTranslateUIDToDevice,
        global,
        Some(&uid),
    );
    let Some(object) = found.filter(|object| *object != kAudioObjectUnknown) else {
        return AudioKind::Other;
    };
    let scope = match input {
        true => kAudioObjectPropertyScopeInput,
        false => kAudioObjectPropertyScopeOutput,
    };
    kind_of(
        property(object, kAudioDevicePropertyTransportType, global, None),
        property(object, kAudioDevicePropertyDataSource, scope, None),
        input,
    )
}

/// What a device on `transport` with data source `source` is. Only what
/// is sure: a built-in output is the speakers unless its source is the
/// headphone jack, a screen's output its speakers, a Bluetooth input a
/// headset's microphone; a Bluetooth output may be a speaker as well as
/// headphones, and says `Other`.
fn kind_of(transport: Option<u32>, source: Option<u32>, input: bool) -> AudioKind {
    const HEADPHONE_JACK: u32 = u32::from_be_bytes(*b"hdpn");
    let on =
        |transports: &[u32]| transport.is_some_and(|transport| transports.contains(&transport));
    if on(&[kAudioDeviceTransportTypeBuiltIn]) {
        match (source, input) {
            (Some(HEADPHONE_JACK), _) => AudioKind::Headphones,
            (_, false) => AudioKind::Speaker,
            (_, true) => AudioKind::Other,
        }
    } else if input
        && on(&[
            kAudioDeviceTransportTypeBluetooth,
            kAudioDeviceTransportTypeBluetoothLE,
        ])
    {
        AudioKind::Headphones
    } else if !input
        && on(&[
            kAudioDeviceTransportTypeHDMI,
            kAudioDeviceTransportTypeDisplayPort,
        ])
    {
        AudioKind::Speaker
    } else {
        AudioKind::Other
    }
}

/// A `u32` property of a Core Audio object (asked with a CFString
/// `qualifier`, as a UID's translation is), `None` where it has none.
fn property(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    qualifier: Option<&NSString>,
) -> Option<u32> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    // an NSString is its CFString (toll-free bridged); the qualifier is a
    // pointer to it
    let reference: *const NSString = qualifier.map_or(std::ptr::null(), |string| string);
    let (qualifier_size, qualifier_data) = match qualifier {
        Some(_) => (
            size_of::<*const NSString>() as u32,
            (&raw const reference).cast::<std::ffi::c_void>(),
        ),
        None => (0, std::ptr::null()),
    };
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: a `u32` property read into a `u32` of that size; the
    // qualifier, where given, points at a live CFString reference
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            qualifier_size,
            qualifier_data,
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    (status == 0 && size == size_of::<u32>() as u32).then_some(value)
}

/// System Settings' page that allows `kind` for one app.
pub(crate) fn settings_page(kind: crate::Kind) -> Option<&'static str> {
    Some(match kind {
        crate::Kind::Camera => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
        }
        crate::Kind::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
    })
}

/// How a person allows a camera or a microphone the system refused: on
/// that page ([`settings_page`]).
pub(crate) fn allow(kind: crate::Kind) -> &'static str {
    match kind {
        crate::Kind::Camera => {
            "Open System Settings \u{2192} Privacy & Security \u{2192} Camera, turn Ducktape on, then try again."
        }
        crate::Kind::Microphone => {
            "Open System Settings \u{2192} Privacy & Security \u{2192} Microphone, turn Ducktape on, then try again."
        }
    }
}

// ---------- the cameras ----------

/// The cameras the system lists now; the system's default first. Listing
/// asks for no permission.
pub(crate) fn cameras() -> Result<Vec<CameraDevice>, String> {
    autoreleasepool(|_| {
        let video = crate::apple::video()?;
        // the one list that is the same from macOS 11 (the app's floor) on;
        // the discovery session's type for external cameras is macOS 14's
        #[allow(deprecated)]
        // SAFETY: a media type constant
        let devices = unsafe { AVCaptureDevice::devicesWithMediaType(video) };
        // SAFETY: as above
        let default = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(video) }
            .map(|device| unsafe { device.uniqueID() }.to_string());
        let mut listed: Vec<CameraDevice> = devices
            .iter()
            .map(|device| {
                // SAFETY: plain properties of a listed device
                let (id, name) = unsafe { (device.uniqueID(), device.localizedName()) };
                let id = id.to_string();
                CameraDevice {
                    device: Device {
                        default: default.as_ref() == Some(&id),
                        name: name.to_string(),
                        id: id.clone(),
                    },
                    facing: facing(&device),
                    handle: id,
                }
            })
            .collect();
        listed.sort_by_key(|camera| !camera.device.default);
        Ok(listed)
    })
}

/// Which way `device` looks: where the system says it sits, else a Mac's
/// own camera (the built-in wide angle type) at the person and any other
/// plugged in.
// ponytail: an iPhone as a Continuity Camera lists as the built-in type
// unless the app opts in to its own type (macOS 14); it lists as Front.
fn facing(device: &AVCaptureDevice) -> Facing {
    // SAFETY: plain properties of a listed device; the type constant is
    // there since macOS 10.15
    let (position, kind) = unsafe { (device.position(), device.deviceType()) };
    match position {
        AVCaptureDevicePosition::Front => Facing::Front,
        AVCaptureDevicePosition::Back => Facing::Back,
        _ if *kind == *unsafe { AVCaptureDeviceTypeBuiltInWideAngleCamera } => Facing::Front,
        _ => Facing::External,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Core Audio's transport and data source, as what a device is: only
    /// what is sure.
    #[test]
    fn a_device_is_what_its_transport_says() {
        let jack = Some(u32::from_be_bytes(*b"hdpn"));
        let speakers = Some(u32::from_be_bytes(*b"ispk"));
        let built_in = Some(kAudioDeviceTransportTypeBuiltIn);
        assert_eq!(kind_of(built_in, speakers, false), AudioKind::Speaker);
        assert_eq!(kind_of(built_in, None, false), AudioKind::Speaker);
        assert_eq!(kind_of(built_in, jack, false), AudioKind::Headphones);
        assert_eq!(kind_of(built_in, None, true), AudioKind::Other);
        let bluetooth = Some(kAudioDeviceTransportTypeBluetooth);
        assert_eq!(kind_of(bluetooth, None, true), AudioKind::Headphones);
        assert_eq!(kind_of(bluetooth, None, false), AudioKind::Other);
        let hdmi = Some(kAudioDeviceTransportTypeHDMI);
        assert_eq!(kind_of(hdmi, None, false), AudioKind::Speaker);
        assert_eq!(kind_of(None, None, false), AudioKind::Other);
    }

    /// The microphones and speakers Core Audio lists, each with what it
    /// is; listing opens nothing.
    #[test]
    fn the_audio_devices_say_what_they_are() {
        let (inputs, outputs) = crate::audio_devices().unwrap();
        eprintln!("inputs: {inputs:?}\noutputs: {outputs:?}");
        // a Mac's own speakers, where it has them, are a speaker
        let own = outputs
            .iter()
            .find(|output| output.device.id.contains("BuiltIn"));
        assert!(
            own.is_none_or(|own| own.kind != AudioKind::Other),
            "{own:?}"
        );
    }

    /// Listing runs and opens nothing.
    #[test]
    fn the_cameras_are_listed() {
        let listed = cameras().unwrap();
        eprintln!("cameras: {listed:?}");
        assert!(listed.iter().skip(1).all(|camera| !camera.device.default));
    }
}
