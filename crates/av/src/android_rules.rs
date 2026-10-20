//! Android's rules, apart from Android: how a camera's YUV picture turns
//! into an upright BGRA one, which size a camera is asked for and what its
//! cameras are called, and which of the audio devices Android lists a
//! person picks, by what Android says of them, and how a picture is laid
//! out for MediaCodec's H.264 encoder. Nothing here calls the system, so
//! every OS builds it for its tests (they run on Linux); `android` hands
//! it what Camera2 and cpal's AAudio host say.
use crate::backend::{AudioDevice, AudioKind, Device, Facing, Picture, nameable};

pub(crate) const YUV: i32 = 0x23;

pub(crate) fn size(configurations: &[i32]) -> Option<(i32, i32)> {
    const WANT: (i32, i32) = (1280, 720);
    let offered: Vec<(i32, i32)> = configurations
        .chunks_exact(4)
        .filter(|four| four[0] == YUV && four[3] == 0)
        .map(|four| (four[1], four[2]))
        .collect();
    let area = |(width, height): (i32, i32)| i64::from(width) * i64::from(height);
    if offered.contains(&WANT) {
        return Some(WANT);
    }
    offered
        .iter()
        .copied()
        .filter(|&size| area(size) <= area(WANT))
        .max_by_key(|&size| area(size))
        .or_else(|| offered.iter().copied().min_by_key(|&size| area(size)))
}

pub(crate) fn facing_of(lens: Option<u8>) -> Facing {
    match lens {
        Some(0) => Facing::Front,
        Some(1) => Facing::Back,
        _ => Facing::External,
    }
}

pub(crate) fn names(facings: &[Facing]) -> Vec<String> {
    let word = |facing| match facing {
        Facing::Front => "Front camera",
        Facing::Back => "Back camera",
        Facing::External => "Camera",
    };
    facings
        .iter()
        .enumerate()
        .map(|(at, &facing)| {
            let same = facings.iter().filter(|&&other| other == facing).count();
            let nth = facings[..at]
                .iter()
                .filter(|&&other| other == facing)
                .count()
                + 1;
            match same {
                1 => word(facing).to_owned(),
                _ => format!("{} {nth}", word(facing)),
            }
        })
        .collect()
}

pub(crate) fn upright(sensor: u32, display: u32, front: bool) -> u32 {
    match front {
        true => (sensor + display) % 360,
        false => (sensor + 360 - display % 360) % 360,
    }
}

pub(crate) struct Plane<'a> {
    pub data: &'a [u8],
    pub row: usize,
    pub pixel: usize,
}

pub(crate) fn yuv420_to_bgra(
    [y, u, v]: [Plane; 3],
    width: usize,
    height: usize,
    turn: u32,
    mirror: bool,
) -> Option<Picture> {
    let fits = |plane: &Plane, w: usize, h: usize| {
        w > 0
            && h > 0
            && plane.pixel > 0
            && plane.data.len() > (h - 1) * plane.row + (w - 1) * plane.pixel
    };
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    if !(fits(&y, width, height) && fits(&u, cw, ch) && fits(&v, cw, ch)) {
        return None;
    }
    let sideways = turn % 180 == 90;
    let (out_w, out_h) = match sideways {
        true => (height, width),
        false => (width, height),
    };
    let mut bgra = vec![0u8; out_w * out_h * 4];
    for (oy, line) in bgra.chunks_exact_mut(out_w * 4).enumerate() {
        for ox in 0..out_w {
            let (sx, sy) = match turn {
                90 => (oy, height - 1 - ox),
                180 => (width - 1 - ox, height - 1 - oy),
                270 => (width - 1 - oy, ox),
                _ => (ox, oy),
            };
            let luma = y.data[sy * y.row + sx * y.pixel];
            let chroma = (sy / 2) * u.row + (sx / 2) * u.pixel;
            let red = (sy / 2) * v.row + (sx / 2) * v.pixel;
            let (b, g, r) = bt601_full_range_bgr(luma, u.data[chroma], v.data[red]);
            let at = if mirror { out_w - 1 - ox } else { ox } * 4;
            line[at..at + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    Some(Picture {
        width: out_w as u32,
        height: out_h as u32,
        bgra,
    })
}

pub(crate) fn bt601_full_range_bgr(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = i32::from(y) << 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let clamp = |value: i32| ((value + (1 << 15)) >> 16).clamp(0, 255) as u8;
    (
        clamp(y + 116_130 * d),
        clamp(y - 22_554 * d - 46_802 * e),
        clamp(y + 91_881 * e),
    )
}

#[cfg(feature = "call")]
pub(crate) const I420: i32 = 19;
#[cfg(feature = "call")]
pub(crate) const NV12: i32 = 21;

#[cfg(feature = "call")]
#[derive(Clone, Copy)]
pub(crate) struct Input {
    pub color: i32,
    pub stride: usize,
    pub slice: usize,
}

#[cfg(feature = "call")]
pub(crate) fn bgra_to_yuv(
    bgra: &[u8],
    width: usize,
    height: usize,
    input: Input,
    out: &mut Vec<u8>,
) {
    let (stride, slice) = (input.stride, input.slice);
    out.clear();
    out.resize(stride * slice * 3 / 2, 0);
    let (y_plane, chroma) = out.split_at_mut(stride * slice);
    let at = |x: usize, y: usize| match input.color {
        I420 => {
            let u = (y / 2) * (stride / 2) + x / 2;
            (u, stride * slice / 4 + u)
        }
        _ => {
            let u = (y / 2) * stride + x;
            (u, u + 1)
        }
    };
    for (y, row) in bgra.chunks_exact(width * 4).take(height).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            let (b, g, r) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );
            y_plane[y * stride + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
            if y % 2 == 0 && x % 2 == 0 {
                let (u, v) = at(x, y);
                chroma[u] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                chroma[v] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
            }
        }
    }
}

pub(crate) fn route(description: &cpal::DeviceDescription) -> Option<(AudioKind, String)> {
    use cpal::{DeviceType as Type, InterfaceType as Via};
    let kind = description.device_type();
    let suffix = format!(" ({kind})");
    let product = || {
        let name = description.name();
        name.strip_suffix(&suffix).unwrap_or(name).to_owned()
    };
    Some(match (kind, description.interface_type()) {
        (Type::Speaker, Via::BuiltIn) => (AudioKind::Speaker, "Speaker".into()),
        (Type::Speaker, _) | (Type::Unknown, Via::Hdmi) => (AudioKind::Speaker, product()),
        (Type::Earpiece, _) => (AudioKind::Earpiece, "Earpiece".into()),
        (Type::Microphone, _) => (AudioKind::Other, "Phone microphone".into()),
        (Type::Headphones, _) => (AudioKind::Headphones, "Wired headphones".into()),
        (Type::Headset, Via::Bluetooth | Via::Usb) | (Type::HearingAid, _) => {
            (AudioKind::Headphones, product())
        }
        (Type::Headset, _) => (AudioKind::Headphones, "Wired headset".into()),
        (Type::Unknown, Via::Bluetooth) => (AudioKind::Headphones, product()),
        (Type::Unknown, Via::Usb) => (AudioKind::Other, product()),
        _ => return None,
    })
}

pub(crate) fn recognizable(
    listed: &[(String, cpal::DeviceDescription)],
    input: bool,
) -> Vec<(AudioDevice, usize)> {
    let mut kept: Vec<(AudioDevice, usize, cpal::InterfaceType)> = Vec::new();
    for (at, (id, description)) in listed.iter().enumerate() {
        let Some((kind, name)) = route(description) else {
            continue;
        };
        let device = Device {
            id: id.clone(),
            name,
            default: false,
        };
        if !nameable(&device) {
            continue;
        }
        let via = description.interface_type();
        let stereo = description.device_type() == cpal::DeviceType::Unknown
            && via == cpal::InterfaceType::Bluetooth;
        let row = (AudioDevice { device, kind }, at, via);
        match kept
            .iter()
            .position(|(kept, ..)| kept.device.name == row.0.device.name)
        {
            Some(same) if stereo => kept[same] = row,
            Some(_) => {}
            None => kept.push(row),
        }
    }
    let rows: Vec<_> = kept.iter().map(|(row, _, via)| (row.kind, *via)).collect();
    if let Some(at) = default(&rows, input) {
        kept[at].0.device.default = true;
    }
    kept.into_iter().map(|(row, at, _)| (row, at)).collect()
}

pub(crate) fn default(rows: &[(AudioKind, cpal::InterfaceType)], input: bool) -> Option<usize> {
    use cpal::InterfaceType as Via;
    let plugged = rows.iter().position(|&(kind, via)| match input {
        true => kind == AudioKind::Headphones && !matches!(via, Via::BuiltIn | Via::Bluetooth),
        false => via != Via::BuiltIn,
    });
    plugged.or_else(|| {
        rows.iter()
            .position(|&(kind, via)| via == Via::BuiltIn && (input || kind == AudioKind::Speaker))
    })
}

pub(crate) fn plays_loud(output: Option<&str>, outputs: &[AudioDevice]) -> bool {
    outputs
        .iter()
        .find(|row| match output {
            Some(id) => row.device.id == id,
            None => row.device.default,
        })
        .is_some_and(|row| row.kind == AudioKind::Speaker)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered_luma_with_grey_chroma() -> ([u8; 6], [u8; 4], [u8; 4]) {
        ([10, 20, 30, 40, 50, 60], [128; 4], [128; 4])
    }

    fn turned(turn: u32, mirror: bool) -> (u32, u32, Vec<u8>) {
        let (luma, u, v) = numbered_luma_with_grey_chroma();
        let planes = [
            Plane {
                data: &luma,
                row: 3,
                pixel: 1,
            },
            Plane {
                data: &u,
                row: 2,
                pixel: 1,
            },
            Plane {
                data: &v,
                row: 2,
                pixel: 1,
            },
        ];
        let picture = yuv420_to_bgra(planes, 3, 2, turn, mirror).unwrap();
        let shown = picture.bgra.chunks_exact(4).map(|pixel| pixel[0]).collect();
        (picture.width, picture.height, shown)
    }

    #[test]
    fn a_picture_turns_clockwise_then_mirrors() {
        assert_eq!(turned(0, false), (3, 2, vec![10, 20, 30, 40, 50, 60]));
        assert_eq!(turned(90, false), (2, 3, vec![40, 10, 50, 20, 60, 30]));
        assert_eq!(turned(180, false), (3, 2, vec![60, 50, 40, 30, 20, 10]));
        assert_eq!(turned(270, false), (2, 3, vec![30, 60, 20, 50, 10, 40]));
        assert_eq!(turned(0, true), (3, 2, vec![30, 20, 10, 60, 50, 40]));
        assert_eq!(turned(90, true), (2, 3, vec![10, 40, 20, 50, 30, 60]));
    }

    #[test]
    fn chroma_is_read_by_its_stride() {
        let planes = |luma: &'static [u8], vu: &'static [u8]| {
            [
                Plane {
                    data: luma,
                    row: 2,
                    pixel: 1,
                },
                Plane {
                    data: &vu[1..],
                    row: 2,
                    pixel: 2,
                },
                Plane {
                    data: vu,
                    row: 2,
                    pixel: 2,
                },
            ]
        };
        let (luma, vu): (&[u8], &[u8]) = (&[76; 4], &[255, 85, 255, 85]);
        let picture = yuv420_to_bgra(planes(luma, vu), 2, 2, 0, false).unwrap();
        let [b, g, r, a] = picture.bgra[..4] else {
            unreachable!()
        };
        assert!(
            r > 250 && g < 5 && b < 5 && a == 255,
            "{:?}",
            &picture.bgra[..4]
        );
        assert!(yuv420_to_bgra(planes(&luma[..3], vu), 2, 2, 0, false).is_none());
    }

    #[cfg(feature = "call")]
    #[test]
    fn a_picture_fills_the_encoders_input_at_any_stride() {
        let (width, height) = (330, 270);
        let colour = |x: usize, y: usize| ((x / 2) as i32, (y / 2) as i32);
        let bgra: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let (b, r) = colour(x, y);
                [b as u8, 0, r as u8, 255]
            })
            .collect();
        let luma = |(b, r): (i32, i32)| (((66 * r + 25 * b + 128) >> 8) + 16) as u8;
        let chroma = |(b, r): (i32, i32)| {
            (
                (((-38 * r + 112 * b + 128) >> 8) + 128) as u8,
                (((112 * r - 18 * b + 128) >> 8) + 128) as u8,
            )
        };
        for color in [NV12, I420] {
            for (stride, slice) in [(330, 270), (334, 270), (331, 271), (333, 275), (352, 272)] {
                let mut out = Vec::new();
                let input = Input {
                    color,
                    stride,
                    slice,
                };
                bgra_to_yuv(&bgra, width, height, input, &mut out);
                assert_eq!(out.len(), stride * slice * 3 / 2);
                let (y_plane, uv) = out.split_at(stride * slice);
                let place = |x, y| format!("{color} at {stride} x {slice}, ({x}, {y})");
                for y in 0..height {
                    for x in 0..width {
                        let got = y_plane[y * stride + x];
                        assert_eq!(got, luma(colour(x, y)), "{}", place(x, y));
                    }
                }
                for y in (0..height).step_by(2) {
                    for x in (0..width).step_by(2) {
                        let (u, v) = match color {
                            NV12 => (y / 2 * stride + x, y / 2 * stride + x + 1),
                            _ => {
                                let u = y / 2 * (stride / 2) + x / 2;
                                (u, stride * slice / 4 + u)
                            }
                        };
                        let got = (uv[u], uv[v]);
                        assert_eq!(got, chroma(colour(x, y)), "{}", place(x, y));
                    }
                }
            }
        }
    }

    #[test]
    fn a_camera_is_turned_upright_for_the_display() {
        assert_eq!(upright(90, 0, false), 90);
        assert_eq!(upright(90, 90, false), 0);
        assert_eq!(upright(90, 270, false), 180);
        assert_eq!(upright(270, 0, true), 270);
        assert_eq!(upright(270, 90, true), 0);
        assert_eq!(upright(270, 180, true), 90);
    }

    #[test]
    fn a_camera_is_asked_for_720p_or_the_nearest_below() {
        let configurations = |sizes: &[(i32, i32)]| -> Vec<i32> {
            sizes
                .iter()
                .flat_map(|&(width, height)| [YUV, width, height, 0])
                .collect()
        };
        let wide = configurations(&[(1920, 1080), (1280, 720), (640, 480)]);
        assert_eq!(size(&wide), Some((1280, 720)));
        let old = configurations(&[(1600, 1200), (1024, 768), (640, 480)]);
        assert_eq!(size(&old), Some((1024, 768)));
        let big = configurations(&[(4000, 3000), (1920, 1080)]);
        assert_eq!(size(&big), Some((1920, 1080)));
        let other = [256, 1280, 720, 0, YUV, 1280, 720, 1];
        assert_eq!(size(&other), None);
    }

    #[test]
    fn cameras_are_named_by_which_way_they_look() {
        assert_eq!(
            names(&[Facing::Front, Facing::Back]),
            ["Front camera", "Back camera"]
        );
        assert_eq!(
            names(&[Facing::Front, Facing::Back, Facing::Back, Facing::External]),
            ["Front camera", "Back camera 1", "Back camera 2", "Camera"]
        );
        assert_eq!(facing_of(Some(0)), Facing::Front);
        assert_eq!(facing_of(Some(1)), Facing::Back);
        assert_eq!(facing_of(Some(2)), Facing::External);
        assert_eq!(facing_of(None), Facing::External);
    }

    fn described(
        name: &str,
        kind: cpal::DeviceType,
        via: cpal::InterfaceType,
    ) -> cpal::DeviceDescription {
        cpal::DeviceDescriptionBuilder::new(name)
            .device_type(kind)
            .interface_type(via)
            .build()
    }

    #[test]
    fn a_route_is_what_android_says_of_it() {
        use cpal::{DeviceType as Type, InterfaceType as Via};
        let route_of = |name: &str, kind, via| route(&described(name, kind, via));
        let speaker = route_of("Pixel 7 (Speaker)", Type::Speaker, Via::BuiltIn);
        assert_eq!(speaker, Some((AudioKind::Speaker, "Speaker".into())));
        let earpiece = route_of("Pixel 7 (Earpiece)", Type::Earpiece, Via::BuiltIn);
        assert_eq!(earpiece, Some((AudioKind::Earpiece, "Earpiece".into())));
        let mic = route_of("Pixel 7 (Microphone)", Type::Microphone, Via::BuiltIn);
        assert_eq!(mic, Some((AudioKind::Other, "Phone microphone".into())));
        let buds = route_of("Buds (Headset)", Type::Headset, Via::Bluetooth);
        assert_eq!(buds, Some((AudioKind::Headphones, "Buds".into())));
        let stereo = route_of("Buds", Type::Unknown, Via::Bluetooth);
        assert_eq!(stereo, Some((AudioKind::Headphones, "Buds".into())));
        let wired = route_of("Pixel 7 (Headset)", Type::Headset, Via::Unknown);
        assert_eq!(wired, Some((AudioKind::Headphones, "Wired headset".into())));
        for (kind, via) in [
            (Type::Unknown, Via::Unknown),
            (Type::Virtual, Via::Virtual),
            (Type::Tuner, Via::Unknown),
            (Type::Dock, Via::Unknown),
        ] {
            assert_eq!(route_of("Pixel 7", kind, via), None, "{kind:?} {via:?}");
        }
    }

    #[test]
    fn a_phone_lists_one_row_a_device() {
        use cpal::{DeviceType as Type, InterfaceType as Via};
        let outputs = [
            (
                "aaudio:2",
                described("Pixel (Speaker)", Type::Speaker, Via::BuiltIn),
            ),
            (
                "aaudio:3",
                described("Pixel (Speaker)", Type::Speaker, Via::BuiltIn),
            ),
            (
                "aaudio:4",
                described("Pixel (Earpiece)", Type::Earpiece, Via::BuiltIn),
            ),
            ("aaudio:5", described("Pixel", Type::Unknown, Via::Unknown)),
            (
                "aaudio:6",
                described("Buds (Headset)", Type::Headset, Via::Bluetooth),
            ),
            ("aaudio:7", described("Buds", Type::Unknown, Via::Bluetooth)),
        ]
        .map(|(id, description)| (id.to_owned(), description));
        let kept: Vec<_> = recognizable(&outputs, false)
            .into_iter()
            .map(|(row, at)| (row.device.name, row.device.default, at))
            .collect();
        assert_eq!(
            kept,
            [
                ("Speaker".into(), false, 0),
                ("Earpiece".into(), false, 2),
                ("Buds".into(), true, 5),
            ]
        );
        let inputs = [
            (
                "aaudio:8",
                described("Pixel (Microphone)", Type::Microphone, Via::BuiltIn),
            ),
            (
                "aaudio:9",
                described("Pixel (Microphone)", Type::Microphone, Via::BuiltIn),
            ),
        ]
        .map(|(id, description)| (id.to_owned(), description));
        let kept: Vec<_> = recognizable(&inputs, true)
            .into_iter()
            .map(|(row, at)| (row.device.name, row.device.default, at))
            .collect();
        assert_eq!(kept, [("Phone microphone".into(), true, 0)]);
    }

    #[test]
    fn the_default_is_what_android_plays_and_records_on() {
        use cpal::InterfaceType as Via;
        let speaker = (AudioKind::Speaker, Via::BuiltIn);
        let earpiece = (AudioKind::Earpiece, Via::BuiltIn);
        let buds = (AudioKind::Headphones, Via::Bluetooth);
        assert_eq!(default(&[earpiece, speaker], false), Some(1));
        assert_eq!(default(&[earpiece, speaker, buds], false), Some(2));
        let phone = (AudioKind::Other, Via::BuiltIn);
        let wired = (AudioKind::Headphones, Via::Unknown);
        let paired = (AudioKind::Headphones, Via::Bluetooth);
        assert_eq!(default(&[phone], true), Some(0));
        assert_eq!(default(&[phone, paired], true), Some(0));
        assert_eq!(default(&[phone, paired, wired], true), Some(2));
        assert_eq!(default(&[], true), None);
    }

    #[test]
    fn the_loudspeaker_is_the_speaker_picked_or_taken() {
        let row = |id: &str, kind, default| AudioDevice {
            device: Device {
                id: id.into(),
                name: id.into(),
                default,
            },
            kind,
        };
        let plugged = [
            row("speaker", AudioKind::Speaker, false),
            row("headset", AudioKind::Headphones, true),
        ];
        let pulled = [row("speaker", AudioKind::Speaker, true)];
        assert!(!plays_loud(None, &plugged));
        assert!(plays_loud(None, &pulled));
        assert!(!plays_loud(Some("headset"), &plugged));
        assert!(plays_loud(Some("speaker"), &plugged));
        assert!(!plays_loud(Some("headset"), &pulled), "plays nowhere");
        assert!(!plays_loud(None, &[]));
    }
}
