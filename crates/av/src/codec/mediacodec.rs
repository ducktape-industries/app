//! Android: MediaCodec's `video/avc` through the NDK, under the device
//! maker's licence. Synchronous buffers: a picture goes in, the packet (or
//! the one before it, on a codec that holds one) comes out within the
//! same call, or none this time. The encoder's parameter sets come once,
//! flagged `CODEC_CONFIG`, and are put in front of every keyframe here;
//! the decoder takes the wire's Annex B as is, configured from the first
//! keyframe's sets.
//!
//! ponytail: the decoder reads planar (19) and semi-planar (21) YUV 4:2:0
//! output; a vendor's own tiled format (Qualcomm's, some MediaTeks') is
//! refused and the call shows no picture. The general path is decoding
//! to an ImageReader surface, whose planes describe themselves: add it when
//! a device shows up with one.
use std::time::Duration;

use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;
use ndk_sys::{AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG, AMEDIACODEC_BUFFER_FLAG_KEY_FRAME};

use super::{NAL_PPS, NAL_SPS, VideoPacket, nal_type, nal_units};
use crate::Picture;

const MIME: &str = "video/avc";
/// COLOR_FormatYUV420Planar, COLOR_FormatYUV420SemiPlanar.
const I420: i32 = 19;
const NV12: i32 = 21;
/// AVCProfileBaseline.
const BASELINE: i32 = 1;
/// BITRATE_MODE_CBR.
const CBR: i32 = 2;
/// How long one call waits on a buffer.
const WAIT: Duration = Duration::from_millis(100);

pub(super) struct Encoder {
    codec: MediaCodec,
    width: usize,
    height: usize,
    /// The parameter sets the codec gave once, as Annex B.
    sets: Vec<u8>,
    nv12: Vec<u8>,
}

impl Encoder {
    pub(super) fn new(width: u32, height: u32, bitrate: u32, fps: u32) -> Result<Self, String> {
        let codec = MediaCodec::from_encoder_type(MIME).ok_or("no H.264 encoder on this device")?;
        let format = |mode: Option<i32>| {
            let mut format = MediaFormat::new();
            format.set_str("mime", MIME);
            format.set_i32("width", width as i32);
            format.set_i32("height", height as i32);
            format.set_i32("color-format", NV12);
            format.set_i32("bitrate", bitrate as i32);
            format.set_i32("frame-rate", fps as i32);
            // the host asks for a keyframe at this bound anyway; MediaCodec
            // has no "never" that every encoder honours
            format.set_i32("i-frame-interval", 2);
            format.set_i32("profile", BASELINE);
            // ignored by a codec that lacks it (API 30)
            format.set_i32("latency", 1);
            if let Some(mode) = mode {
                format.set_i32("bitrate-mode", mode);
            }
            format
        };
        if codec
            .configure(&format(Some(CBR)), None, MediaCodecDirection::Encoder)
            .is_err()
        {
            // an encoder without constant bitrate: its own mode
            codec
                .configure(&format(None), None, MediaCodecDirection::Encoder)
                .map_err(|e| format!("H.264 encoder configure: {e}"))?;
        }
        codec
            .start()
            .map_err(|e| format!("H.264 encoder start: {e}"))?;
        Ok(Self {
            codec,
            width: width as usize,
            height: height as usize,
            sets: Vec::new(),
            nv12: Vec::new(),
        })
    }

    pub(super) fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        let mut params = MediaFormat::new();
        params.set_i32("video-bitrate", bitrate as i32);
        self.codec
            .set_parameters(params)
            .map_err(|e| format!("H.264 bitrate: {e}"))
    }

    pub(super) fn keyframe(&mut self) {
        let mut params = MediaFormat::new();
        params.set_i32("request-sync", 0);
        let _ = self.codec.set_parameters(params);
    }

    pub(super) fn encode(
        &mut self,
        bgra: &[u8],
        at: Duration,
    ) -> Result<Option<VideoPacket>, String> {
        let input = match self.codec.dequeue_input_buffer(WAIT) {
            Ok(DequeuedInputBufferResult::Buffer(buffer)) => buffer,
            // the codec is full: this picture is dropped, as the rate
            // control would
            Ok(DequeuedInputBufferResult::TryAgainLater) => return Ok(None),
            Err(e) => return Err(format!("H.264 input: {e}")),
        };
        bgra_to_nv12(bgra, self.width, self.height, &mut self.nv12);
        let mut input = input;
        let room = input.buffer_mut();
        if room.len() < self.nv12.len() {
            return Err(format!(
                "an input buffer of {} bytes for {}",
                room.len(),
                self.nv12.len()
            ));
        }
        for (slot, byte) in room.iter_mut().zip(&self.nv12) {
            slot.write(*byte);
        }
        self.codec
            .queue_input_buffer(input, 0, self.nv12.len(), at.as_micros() as u64, 0)
            .map_err(|e| format!("H.264 queue: {e}"))?;
        loop {
            let output = match self.codec.dequeue_output_buffer(WAIT) {
                Ok(DequeuedOutputBufferInfoResult::Buffer(buffer)) => buffer,
                Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => return Ok(None),
                Ok(_) => continue,
                Err(e) => return Err(format!("H.264 output: {e}")),
            };
            let info = *output.info();
            let (offset, size) = (info.offset().max(0) as usize, info.size().max(0) as usize);
            let bytes = output
                .buffer()
                .get(offset..offset + size)
                .unwrap_or(&[])
                .to_vec();
            let flags = info.flags();
            self.codec
                .release_output_buffer(output, false)
                .map_err(|e| format!("H.264 release: {e}"))?;
            if flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG != 0 {
                self.sets = bytes;
                continue;
            }
            if bytes.is_empty() {
                continue;
            }
            let keyframe = flags & AMEDIACODEC_BUFFER_FLAG_KEY_FRAME != 0;
            let has_sets = nal_units(&bytes).any(|unit| nal_type(unit) == NAL_SPS);
            let data = match keyframe && !has_sets {
                true => [&self.sets[..], &bytes[..]].concat(),
                false => bytes,
            };
            return Ok(Some(VideoPacket { data, keyframe }));
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.codec.stop();
    }
}

/// BGRA into NV12 (BT.601, studio range), `width` by `height` (even).
fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    out.clear();
    out.resize(width * height * 3 / 2, 0);
    let (y_plane, uv_plane) = out.split_at_mut(width * height);
    for (y, row) in bgra.chunks_exact(width * 4).take(height).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            let (b, g, r) = (
                i32::from(pixel[0]),
                i32::from(pixel[1]),
                i32::from(pixel[2]),
            );
            y_plane[y * width + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
            if y % 2 == 0 && x % 2 == 0 {
                let at = (y / 2) * width + x;
                uv_plane[at] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                uv_plane[at + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
            }
        }
    }
}

/// How the decoder lays its output out, from its output format.
#[derive(Clone, Copy)]
struct Layout {
    color: i32,
    stride: usize,
    slice: usize,
    crop: (usize, usize, usize, usize),
}

impl Layout {
    fn of(format: &MediaFormat) -> Result<Self, String> {
        let get = |key: &str| format.i32(key).map(|v| v.max(0) as usize);
        let width = get("width").ok_or("an output format without a width")?;
        let height = get("height").ok_or("an output format without a height")?;
        let color = format
            .i32("color-format")
            .ok_or("an output format without a colour format")?;
        let (left, top) = (get("crop-left").unwrap_or(0), get("crop-top").unwrap_or(0));
        let right = get("crop-right").unwrap_or(width - 1);
        let bottom = get("crop-bottom").unwrap_or(height - 1);
        Ok(Self {
            color,
            stride: get("stride").unwrap_or(width).max(width),
            slice: get("slice-height").unwrap_or(height).max(height),
            crop: (left, top, right + 1, bottom + 1),
        })
    }

    /// The picture in `bytes`, BT.601 studio range to BGRA.
    fn picture(&self, bytes: &[u8]) -> Result<Picture, String> {
        let (left, top, right, bottom) = self.crop;
        let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
        let needed = match self.color {
            I420 => self.stride * self.slice * 3 / 2,
            NV12 => self.stride * self.slice + self.stride * bottom.div_ceil(2),
            other => {
                return Err(format!(
                    "a decoder colour format {other:#x} this build does not read"
                ));
            }
        };
        if bytes.len() < needed || bottom > self.slice || right > self.stride {
            return Err(format!(
                "{} bytes of decoded picture, {needed} needed",
                bytes.len()
            ));
        }
        let (y_plane, chroma) = bytes.split_at(self.stride * self.slice);
        // U and V at (x, y) of the picture, for the two layouts
        let uv: Box<dyn Fn(usize, usize) -> (u8, u8)> = match self.color {
            I420 => {
                let half = (self.stride / 2, self.slice / 2);
                let (u, v) = chroma.split_at(half.0 * half.1);
                Box::new(move |x, y| (u[(y / 2) * half.0 + x / 2], v[(y / 2) * half.0 + x / 2]))
            }
            _ => {
                let stride = self.stride;
                Box::new(move |x, y| {
                    let at = (y / 2) * stride + (x / 2) * 2;
                    (chroma[at], chroma[at + 1])
                })
            }
        };
        let mut bgra = Vec::with_capacity(width * height * 4);
        for y in top..bottom {
            for x in left..right {
                let luma = 298 * (i32::from(y_plane[y * self.stride + x]) - 16);
                let (u, v) = uv(x, y);
                let (u, v) = (i32::from(u) - 128, i32::from(v) - 128);
                let clamp = |c: i32| ((c + 128) >> 8).clamp(0, 255) as u8;
                bgra.extend([
                    clamp(luma + 516 * u),
                    clamp(luma - 100 * u - 208 * v),
                    clamp(luma + 409 * v),
                    255,
                ]);
            }
        }
        Ok(Picture {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }
}

/// A decoder configured at one stream's parameter sets.
struct Stream {
    sps: Vec<u8>,
    pps: Vec<u8>,
    codec: MediaCodec,
    /// How its output is laid out, once it said.
    layout: Option<Layout>,
}

pub(super) struct Decoder {
    /// None before the first keyframe.
    stream: Option<Stream>,
    frames: u64,
}

impl Decoder {
    pub(super) fn new() -> Result<Self, String> {
        // a decoder is made per stream (its sets); only that one exists
        // is checked here
        MediaCodec::from_decoder_type(MIME).ok_or("no H.264 decoder on this device")?;
        Ok(Self {
            stream: None,
            frames: 0,
        })
    }

    pub(super) fn decode(&mut self, packet: &[u8]) -> Result<Option<Picture>, String> {
        let (mut sps, mut pps) = (None, None);
        for unit in nal_units(packet) {
            match nal_type(unit) {
                NAL_SPS => sps = Some(unit),
                NAL_PPS => pps = Some(unit),
                _ => {}
            }
        }
        if let (Some(sps), Some(pps)) = (sps, pps)
            && self
                .stream
                .as_ref()
                .is_none_or(|had| (&had.sps[..], &had.pps[..]) != (sps, pps))
        {
            if let Some(old) = self.stream.take() {
                let _ = old.codec.stop();
            }
            self.stream = Some(Stream {
                sps: sps.to_vec(),
                pps: pps.to_vec(),
                codec: open(sps, pps)?,
                layout: None,
            });
        }
        let Some(Stream { codec, layout, .. }) = &mut self.stream else {
            return Ok(None);
        };
        let mut input = match codec.dequeue_input_buffer(WAIT) {
            Ok(DequeuedInputBufferResult::Buffer(buffer)) => buffer,
            Ok(DequeuedInputBufferResult::TryAgainLater) => return Ok(None),
            Err(e) => return Err(format!("H.264 decoder input: {e}")),
        };
        let room = input.buffer_mut();
        if room.len() < packet.len() {
            return Err(format!(
                "an input buffer of {} bytes for {}",
                room.len(),
                packet.len()
            ));
        }
        for (slot, byte) in room.iter_mut().zip(packet) {
            slot.write(*byte);
        }
        self.frames += 1;
        codec
            .queue_input_buffer(input, 0, packet.len(), self.frames * 33_333, 0)
            .map_err(|e| format!("H.264 decoder queue: {e}"))?;
        loop {
            let output = match codec.dequeue_output_buffer(WAIT) {
                Ok(DequeuedOutputBufferInfoResult::Buffer(buffer)) => buffer,
                Ok(DequeuedOutputBufferInfoResult::TryAgainLater) => return Ok(None),
                Ok(DequeuedOutputBufferInfoResult::OutputFormatChanged) => {
                    *layout = Some(Layout::of(&codec.output_format())?);
                    continue;
                }
                Ok(DequeuedOutputBufferInfoResult::OutputBuffersChanged) => continue,
                Err(e) => return Err(format!("H.264 decoder output: {e}")),
            };
            let info = *output.info();
            let (offset, size) = (info.offset().max(0) as usize, info.size().max(0) as usize);
            let bytes = output
                .buffer()
                .get(offset..offset + size)
                .unwrap_or(&[])
                .to_vec();
            codec
                .release_output_buffer(output, false)
                .map_err(|e| format!("H.264 decoder release: {e}"))?;
            if bytes.is_empty() {
                continue;
            }
            let layout = match layout {
                Some(layout) => *layout,
                None => *layout.insert(Layout::of(&codec.output_format())?),
            };
            return layout.picture(&bytes).map(Some);
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.codec.stop();
        }
    }
}

/// A decoder started on the parameter sets. The size is a hint the
/// decoder corrects from the sets (its output format says the real one).
fn open(sps: &[u8], pps: &[u8]) -> Result<MediaCodec, String> {
    let codec = MediaCodec::from_decoder_type(MIME).ok_or("no H.264 decoder on this device")?;
    let mut format = MediaFormat::new();
    format.set_str("mime", MIME);
    format.set_i32("width", 640);
    format.set_i32("height", 360);
    format.set_buffer("csd-0", &[&[0, 0, 0, 1], sps].concat());
    format.set_buffer("csd-1", &[&[0, 0, 0, 1], pps].concat());
    format.set_i32("low-latency", 1);
    codec
        .configure(&format, None, MediaCodecDirection::Decoder)
        .map_err(|e| format!("H.264 decoder configure: {e}"))?;
    codec
        .start()
        .map_err(|e| format!("H.264 decoder start: {e}"))?;
    Ok(codec)
}
