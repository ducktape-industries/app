//! Linux and macOS: the microphones and the speakers are the system's
//! own, as cpal lists them: each one opens by itself, nothing routes them
//! and nothing is held while one plays. Each of `v4l2` and `mac` takes
//! these as its share of the seam; iOS routes its one audio session
//! instead (`ios`).
use cpal::traits::{DeviceTrait as _, HostTrait as _};

use crate::backend::{AudioDevice, Device, MicFeed, Status, nameable};

pub(crate) const ROUTED_IN_PLACE: bool = false;

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
    let inputs = kinds(true)?;
    #[cfg(target_os = "linux")]
    let inputs = with_headsets(&host, inputs);
    Ok((inputs, kinds(false)?))
}

#[cfg(target_os = "linux")]
fn with_headsets(host: &cpal::Host, mut inputs: Vec<AudioDevice>) -> Vec<AudioDevice> {
    use crate::pulse::Server as _;
    if host.id() != cpal::HostId::PulseAudio {
        return inputs;
    }
    let graph =
        crate::pulse::Pulse::connect().and_then(|mut pulse| Ok((pulse.cards()?, pulse.sources()?)));
    let Ok((cards, sources)) = graph else {
        return inputs;
    };
    let headsets = crate::pulse::silent_headsets(&cards, &sources)
        .into_iter()
        .map(|(id, name)| AudioDevice {
            device: Device {
                id,
                name,
                default: false,
            },
            kind: crate::backend::AudioKind::Headphones,
        })
        .filter(|headset| nameable(&headset.device));
    inputs.extend(headsets);
    inputs
}

pub(crate) struct Prepared {
    device: Option<String>,
    #[cfg(target_os = "linux")]
    back: Option<(String, String)>,
}

impl Prepared {
    pub(crate) fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }
}

pub(crate) fn prepare(device: Option<&str>) -> Result<Prepared, Status> {
    #[cfg(target_os = "linux")]
    if let Some(id) = device.filter(|id| id.starts_with(crate::pulse::CARD)) {
        let switched = crate::pulse::Pulse::connect()
            .and_then(|mut pulse| crate::pulse::switch(&mut pulse, id))
            .map_err(Status::Failed)?;
        return Ok(Prepared {
            device: Some(format!("pulseaudio:{}", switched.source)),
            back: switched.back,
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = device;
    Ok(Prepared {
        device: None,
        #[cfg(target_os = "linux")]
        back: None,
    })
}

#[cfg(target_os = "linux")]
impl Drop for Prepared {
    fn drop(&mut self) {
        use crate::pulse::Server as _;
        if let Some((card, profile)) = self.back.take() {
            let put_back = crate::pulse::Pulse::connect()
                .and_then(|mut pulse| pulse.set_profile(&card, &profile));
            if let Err(error) = put_back {
                tracing::warn!(target: "ducktape::av", %error, %card, %profile, "the headset's profile was not put back");
            }
        }
    }
}

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

#[cfg(target_os = "linux")]
fn recognizable<T>(devices: Vec<(Device, T)>) -> Vec<(Device, T)> {
    let mut seen = std::collections::HashSet::new();
    devices
        .into_iter()
        .filter_map(|(mut device, found)| {
            if let Some(node) = device.id.strip_prefix("pulseaudio:") {
                return (!node.ends_with(".monitor")).then_some((device, found));
            }
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

pub(crate) fn find(host: &cpal::Host, id: Option<&str>, input: bool) -> Option<cpal::Device> {
    match id {
        Some(id) => picked(listed(host, input).ok()?, id),
        None if input => host.default_input_device(),
        None => host.default_output_device(),
    }
}

fn picked<T>(listed: Vec<(Device, T)>, id: &str) -> Option<T> {
    listed
        .into_iter()
        .find(|(device, _)| device.id == id)
        .map(|(_, found)| found)
}

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

pub(crate) fn heard() -> Option<Device> {
    let listed = listed(&cpal::default_host(), true).ok()?;
    listed
        .into_iter()
        .map(|(device, _)| device)
        .find(|device| device.default)
}

pub(crate) struct Active;

#[cfg(feature = "call")]
pub(crate) fn call(_on: bool) {}

pub(crate) fn activate(
    _microphone: Option<(&std::sync::Arc<MicFeed>, Option<&str>)>,
) -> Result<Active, Status> {
    Ok(Active)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_sound_servers_nodes_are_listed_by_name_and_its_monitors_are_not() {
        let node = |id: &str, name: &str| Device {
            id: format!("pulseaudio:{id}"),
            name: name.into(),
            default: false,
        };
        let listed = vec![
            node(
                "alsa_input.pci-0000_00_1f.3.analog-stereo",
                "Built-in Audio Analog Stereo",
            ),
            node("bluez_input.AA_BB_CC_DD_EE_FF.0", "WH-1000XM4"),
            node("bluez_output.AA_BB_CC_DD_EE_FF.1", "WH-1000XM4"),
            node(
                "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
                "Monitor of Built-in Audio Analog Stereo",
            ),
        ];
        let listed = listed.into_iter().map(|device| (device, ())).collect();
        let kept: Vec<String> = recognizable(listed)
            .into_iter()
            .map(|(device, ())| device.name)
            .collect();
        assert_eq!(
            kept,
            ["Built-in Audio Analog Stereo", "WH-1000XM4", "WH-1000XM4"]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_default_microphone_is_named_as_listed() {
        let input = find(&cpal::default_host(), None, true).expect("ALSA's default");
        assert_eq!(input_name(&input), "System default");
    }

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
