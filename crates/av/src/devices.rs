//! Linux and macOS: the microphones and the speakers are the system's
//! own, as cpal lists them: each one opens by itself, nothing routes them
//! and nothing is held while one plays. Each of `v4l2` and `mac` takes
//! these as its share of the seam; iOS routes its one audio session
//! instead (`ios`).
use cpal::traits::{DeviceTrait as _, HostTrait as _};

use crate::backend::{AudioDevice, Device, MicFeed, Status, nameable};

/// A stream here is opened on its device and ends with it: an invalidated
/// one is gone (iOS routes its one stream in place).
pub(crate) const ROUTED_IN_PLACE: bool = false;

/// The microphones and the speakers the system lists now.
pub(crate) fn audio_devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    let host = cpal::default_host();
    let kinds = |input: bool| {
        listed(&host, input).map(|listed| {
            listed
                .into_iter()
                .map(|(device, _)| AudioDevice {
                    kind: crate::platform::audio_kind(&device, input),
                    device,
                })
                .collect()
        })
    };
    Ok((kinds(true)?, kinds(false)?))
}

/// The microphones (`input`) or the speakers the system lists now, as this
/// crate names them, each with the device it is: the one list
/// [`audio_devices`] answers from and a piece opens from ([`find`]).
fn listed(host: &cpal::Host, input: bool) -> Result<Vec<(Device, cpal::Device)>, String> {
    let id = |device: &cpal::Device| device.id().map(|id| id.to_string()).ok();
    let (devices, default): (Vec<cpal::Device>, _) = match input {
        true => (
            host.input_devices()
                .map_err(|error| error.to_string())?
                .collect(),
            host.default_input_device(),
        ),
        false => (
            host.output_devices()
                .map_err(|error| error.to_string())?
                .collect(),
            host.default_output_device(),
        ),
    };
    let default = default.as_ref().and_then(id);
    let rows = devices
        .into_iter()
        .filter_map(|device| {
            let id = id(&device)?;
            let row = Device {
                name: device
                    .description()
                    .map(|d| d.name().to_owned())
                    .unwrap_or_else(|_| id.clone()),
                default: default.as_ref() == Some(&id),
                id,
            };
            nameable(&row).then_some((row, device))
        })
        .collect();
    Ok(recognizable(rows))
}

/// The devices a person recognizes, one entry per device. cpal's ALSA host
/// lists every PCM name the configuration defines, each a way to one card
/// (`hw`, `plughw`, `sysdefault`, `front`, `surround51`, `dmix`, `dsnoop`,
/// their numbered twins) or a plugin (`null`, `pulse`, `pipewire`): one
/// loopback card was twenty rows. Kept: ALSA's `default` (the system's own
/// choice, a desktop's sound server where one runs), shown as "System
/// default", and one `plughw` per card and device (the device with the
/// rate and format conversions an app needs), the card's own name first,
/// so its numbered twin is dropped. Elsewhere the system lists devices.
#[cfg(target_os = "linux")]
fn recognizable<T>(devices: Vec<(Device, T)>) -> Vec<(Device, T)> {
    let mut seen = std::collections::HashSet::new();
    devices
        .into_iter()
        .filter_map(|(mut device, found)| {
            let pcm = device.id.strip_prefix("alsa:").unwrap_or(&device.id);
            if pcm == "default" {
                device.name = "System default".into();
                return Some((device, found));
            }
            let dev = pcm.strip_prefix("plughw:")?.split("DEV=").nth(1)?;
            seen.insert((device.name.clone(), dev.to_owned()))
                .then_some((device, found))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn recognizable<T>(devices: Vec<(Device, T)>) -> Vec<(Device, T)> {
    devices
}

/// The microphone (`input`) or speaker a piece names: `None` the system's
/// default, else only one this crate listed ([`listed`]), its id compared
/// whole. A view's string never reaches the system: on Linux an id is an
/// ALSA PCM name, and such a name can name a plugin that writes a file or
/// runs a program.
pub(crate) fn find(host: &cpal::Host, id: Option<&str>, input: bool) -> Option<cpal::Device> {
    match id {
        Some(id) => picked(listed(host, input).ok()?, id),
        None if input => host.default_input_device(),
        None => host.default_output_device(),
    }
}

/// What `listed` holds for the device with id `id`, if it lists one.
fn picked<T>(listed: Vec<(Device, T)>, id: &str) -> Option<T> {
    listed
        .into_iter()
        .find(|(device, _)| device.id == id)
        .map(|(_, found)| found)
}

/// The name a person knows the opened microphone `input` by: its row's,
/// as it would be listed to pick from ("System default" for ALSA's
/// default), else the system's own.
pub(crate) fn input_name(input: &cpal::Device) -> String {
    let name = input
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_default();
    let Ok(id) = input.id() else {
        return name;
    };
    let row = Device {
        id: id.to_string(),
        name: name.clone(),
        default: false,
    };
    recognizable(vec![(row, ())])
        .pop()
        .map_or(name, |(row, ())| row.name)
}

/// The microphone the system hears by default now, as listed.
pub(crate) fn heard() -> Option<Device> {
    let listed = listed(&cpal::default_host(), true).ok()?;
    listed
        .into_iter()
        .map(|(device, _)| device)
        .find(|device| device.default)
}

/// Nothing to hold here while a microphone or the chime plays
/// ([`activate`]).
pub(crate) struct Active;

/// Nothing to activate: each device opens by itself, and its stream
/// tells its own moves and ends.
pub(crate) fn activate(
    _microphone: Option<(&std::sync::Arc<MicFeed>, Option<&str>)>,
) -> Result<Active, Status> {
    Ok(Active)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One loopback card's ALSA names come down to the default and one
    /// row per card and device.
    #[cfg(target_os = "linux")]
    #[test]
    fn alsa_names_come_down_to_one_row_a_device() {
        let device = |id: &str, name: &str| Device {
            id: format!("alsa:{id}"),
            name: name.into(),
            default: id == "default",
        };
        let card = "Loopback, Loopback PCM";
        let listed = vec![
            device(
                "null",
                "Discard all samples (playback) or generate zero samples (capture)",
            ),
            device("default", "default"),
            device("pipewire", "PipeWire Sound Server"),
            device("sysdefault:CARD=Loopback", card),
            device("front:CARD=Loopback,DEV=0", card),
            device("surround51:CARD=Loopback,DEV=0", card),
            device("dmix:CARD=Loopback,DEV=0", card),
            device("dsnoop:CARD=Loopback,DEV=1", card),
            device("hw:CARD=Loopback,DEV=0", card),
            device("plughw:CARD=Loopback,DEV=0", card),
            device("hw:CARD=Loopback,DEV=1", card),
            device("plughw:CARD=Loopback,DEV=1", card),
            device("hw:CARD=0,DEV=0", card),
            device("plughw:CARD=0,DEV=0", card),
            device("plughw:CARD=0,DEV=1", card),
            device("plughw:CARD=1,DEV=0", "USB Audio, USB Audio"),
        ];
        let listed = listed.into_iter().map(|device| (device, ())).collect();
        let kept: Vec<_> = recognizable(listed)
            .into_iter()
            .map(|(device, ())| (device.id, device.name, device.default))
            .collect();
        assert_eq!(
            kept,
            [
                ("alsa:default".into(), "System default".into(), true),
                ("alsa:plughw:CARD=Loopback,DEV=0".into(), card.into(), false),
                ("alsa:plughw:CARD=Loopback,DEV=1".into(), card.into(), false),
                (
                    "alsa:plughw:CARD=1,DEV=0".into(),
                    "USB Audio, USB Audio".into(),
                    false
                ),
            ]
        );
    }

    /// The default microphone is named as its row is listed: on Linux,
    /// ALSA's default is "System default", not the PCM's own "default".
    #[cfg(target_os = "linux")]
    #[test]
    fn the_default_microphone_is_named_as_listed() {
        let input = find(&cpal::default_host(), None, true).expect("ALSA's default");
        assert_eq!(input_name(&input), "System default");
    }

    /// A piece opens only a device this crate listed, by its whole id: a
    /// name it did not list (an ALSA plugin that writes a file or pipes
    /// into a program, a card it left out, a listed id with more after it)
    /// opens nothing.
    #[test]
    fn a_device_opens_only_by_an_id_this_crate_listed() {
        let row = |id: &str| {
            let device = Device {
                id: id.into(),
                name: "Card".into(),
                default: false,
            };
            (device, id.to_owned())
        };
        let listed = || vec![row("alsa:default"), row("alsa:plughw:CARD=PCH,DEV=0")];
        let card = "alsa:plughw:CARD=PCH,DEV=0";
        assert_eq!(picked(listed(), card).as_deref(), Some(card));
        for id in [
            "alsa:file:'/tmp/x',raw",
            "alsa:file:'|touch /tmp/x',raw",
            "alsa:plughw:CARD=X,DEV=0",
            "alsa:hw:CARD=PCH,DEV=0",
            "alsa:plughw:CARD=PCH,DEV=0 ",
            "plughw:CARD=PCH,DEV=0",
            "",
        ] {
            assert_eq!(picked(listed(), id), None, "{id:?}");
        }
    }
}
