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
#[cfg(feature = "call")]
pub(crate) use crate::devices::{Active, Prepared, call};
pub(crate) use crate::devices::{
    ROUTED_IN_PLACE, activate, audio_devices, find, heard, input_name, prepare,
};

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
                    facing: facing_from_capture_device_type(&device),
                    handle: id,
                }
            })
            .collect();
        listed.sort_by_key(|camera| !camera.device.default);
        Ok(listed)
    })
}

fn facing_from_capture_device_type(device: &AVCaptureDevice) -> Facing {
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

    #[test]
    fn the_audio_devices_say_what_they_are() {
        let (inputs, outputs) = crate::audio_devices().unwrap();
        eprintln!("inputs: {inputs:?}\noutputs: {outputs:?}");
        let own = outputs
            .iter()
            .find(|output| output.device.id.contains("BuiltIn"));
        assert!(
            own.is_none_or(|own| own.kind != AudioKind::Other),
            "{own:?}"
        );
    }

    #[test]
    fn the_cameras_are_listed() {
        let listed = cameras().unwrap();
        eprintln!("cameras: {listed:?}");
        assert!(listed.iter().skip(1).all(|camera| !camera.device.default));
    }
}
