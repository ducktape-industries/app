//! Linux: Cisco's prebuilt OpenH264, loaded from where the app put it
//! ([`super::cisco`]). The `openh264` crate checks the file's sha256
//! against the builds it knows before it loads, so a file that is not
//! Cisco's is refused. Loaded again for each encoder or decoder made (a
//! few ms: a rung change is rare), so the switch holds from the next one.
use std::time::Duration;

use openh264::OpenH264API;
use openh264::decoder::{DecodedYUV, Decoder as Inner, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder as Session, EncoderConfig, FrameRate, FrameType, RateControlMode,
};
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource};
use openh264_sys2::{
    ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SBitrateInfo, SPATIAL_LAYER_0,
};

use super::{VideoPacket, cisco};
use crate::Picture;

fn api() -> Result<OpenH264API, String> {
    let path = cisco::library()?;
    OpenH264API::from_blob_path(&path)
        .map_err(|error| format!("openh264 at {}: {error}", path.display()))
}

pub(super) struct Encoder {
    session: Session,
    bitrate: u32,
    fps: u32,
    /// Nothing encoded yet: OpenH264 starts on its first picture and
    /// takes no option before.
    fresh: bool,
    size: (usize, usize),
    yuv: YUVBuffer,
}

impl Encoder {
    pub(super) fn new(width: u32, height: u32, bitrate: u32, fps: u32) -> Result<Self, String> {
        Ok(Self {
            session: session(bitrate, fps)?,
            bitrate,
            fps,
            fresh: true,
            size: (width as usize, height as usize),
            yuv: YUVBuffer::new(width as usize, height as usize),
        })
    }

    pub(super) fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        if self.fresh {
            self.session = session(bitrate, self.fps)?;
            self.bitrate = bitrate;
            return Ok(());
        }
        let bps = i32::try_from(bitrate).unwrap_or(i32::MAX);
        // OpenH264 refuses a cap under the target: the cap moves first up,
        // last down
        let order = match bitrate > self.bitrate {
            true => [ENCODER_OPTION_MAX_BITRATE, ENCODER_OPTION_BITRATE],
            false => [ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE],
        };
        for option in order {
            let mut info = SBitrateInfo {
                iLayer: SPATIAL_LAYER_0,
                iBitrate: bps,
            };
            // SAFETY: the encoder has started (not `fresh`); both options
            // read one `SBitrateInfo`, live for the call
            let code = unsafe {
                self.session
                    .raw_api()
                    .set_option(option, (&raw mut info).cast())
            };
            if code != 0 {
                return Err(format!("openh264 refused the bitrate {bitrate} ({code})"));
            }
        }
        self.bitrate = bitrate;
        Ok(())
    }

    pub(super) fn keyframe(&mut self) {
        // a fresh encoder's first picture is one anyway
        if !self.fresh {
            self.session.force_intra_frame();
        }
    }

    pub(super) fn encode(
        &mut self,
        bgra: &[u8],
        at: Duration,
    ) -> Result<Option<VideoPacket>, String> {
        self.yuv.read_bgra8(BgraSliceU8::new(bgra, self.size));
        let at =
            openh264::Timestamp::from_millis(u64::try_from(at.as_millis()).unwrap_or(u64::MAX));
        let stream = self
            .session
            .encode_at(&self.yuv, at)
            .map_err(|error| format!("openh264: {error}"))?;
        self.fresh = false;
        match stream.frame_type() {
            FrameType::Skip | FrameType::Invalid => Ok(None),
            kind => Ok(Some(VideoPacket {
                data: stream.to_vec(),
                keyframe: kind == FrameType::IDR,
            })),
        }
    }
}

/// A session for a call: the bitrate kept (frames skipped when over it),
/// spread over `fps` pictures a second (each picture's share is `bitrate /
/// fps`). OpenH264's default profile is Constrained Baseline with the
/// parameter sets in front of every IDR picture: the wire's shape.
fn session(bitrate: u32, fps: u32) -> Result<Session, String> {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate))
        .rate_control_mode(RateControlMode::Bitrate)
        .max_frame_rate(FrameRate::from_hz(fps as f32));
    Session::with_api_config(api()?, config).map_err(|error| format!("openh264: {error}"))
}

pub(super) struct Decoder(Inner);

impl Decoder {
    pub(super) fn new() -> Result<Self, String> {
        Inner::with_api_config(api()?, DecoderConfig::new())
            .map(Self)
            .map_err(|error| format!("openh264: {error}"))
    }

    pub(super) fn decode(&mut self, packet: &[u8]) -> Result<Option<Picture>, String> {
        let Some(yuv) = self
            .0
            .decode(packet)
            .map_err(|error| format!("openh264: {error}"))?
        else {
            return Ok(None);
        };
        Ok(Some(picture(&yuv)))
    }
}

fn picture(yuv: &DecodedYUV) -> Picture {
    let (width, height) = yuv.dimensions();
    let mut bgra = vec![0; width * height * 4];
    yuv.write_rgba8(&mut bgra);
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Picture {
        width: width as u32,
        height: height as u32,
        bgra,
    }
}
