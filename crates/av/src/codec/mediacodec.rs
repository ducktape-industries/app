//! Android: MediaCodec's `video/avc` through the NDK, under the device
//! maker's licence. Synchronous buffers: a picture goes in, the packet (or
//! the one before it, on a codec that holds one) comes out within the
//! same call, or none this time. The encoder's parameter sets come once,
//! flagged `CODEC_CONFIG`, and are put in front of every keyframe here;
//! the decoder takes the wire's Annex B as is, configured from the first
//! keyframe's sets.
//!
//! ponytail: the decoder reads planar (19) and semi-planar (21) YUV 4:2:0
//! output; a device's default decoder giving a vendor's tiled format (some
//! Qualcomms and MediaTeks) is replaced, from the next keyframe, by
//! Android's own software decoder, whose output is one of the two. The
//! general path is decoding to an ImageReader surface, whose planes
//! describe themselves: add it when the hardware decoder matters.
use std::time::Duration;

use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;
use ndk_sys::{AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG, AMEDIACODEC_BUFFER_FLAG_KEY_FRAME};

use super::{NAL_PPS, NAL_SPS, VideoPacket, nal_type, nal_units};
use crate::Picture;
use crate::android_rules::{I420, Input, NV12, bgra_to_yuv};

const MIME: &str = "video/avc";
/// AVCProfileBaseline.
const BASELINE: i32 = 1;
/// BITRATE_MODE_CBR.
const CBR: i32 = 2;
/// How long one call waits on a buffer.
const WAIT: Duration = Duration::from_millis(100);

pub(super) struct Encoder {
    codec: Raw,
    width: usize,
    height: usize,
    input: Input,
    /// The parameter sets the codec gave once, as Annex B.
    sets: Vec<u8>,
    yuv: Vec<u8>,
}

impl Encoder {
    pub(super) fn new(width: u32, height: u32, bitrate: u32, fps: u32) -> Result<Self, String> {
        let format = |color: i32, mode: Option<i32>| {
            let mut format = MediaFormat::new();
            format.set_str("mime", MIME);
            format.set_i32("width", width as i32);
            format.set_i32("height", height as i32);
            format.set_i32("color-format", color);
            format.set_i32("bitrate", bitrate as i32);
            format.set_i32("frame-rate", fps as i32);
            // the host asks for a keyframe at this bound anyway; MediaCodec
            // has no "never" that every encoder honours
            format.set_i32("i-frame-interval", 2);
            format.set_i32("profile", BASELINE);
            // no reordering, whatever the profile key did
            format.set_i32("max-bframes", 0);
            // ignored by a codec that lacks it (API 30)
            format.set_i32("latency", 1);
            if let Some(mode) = mode {
                format.set_i32("bitrate-mode", mode);
            }
            format
        };
        // semi-planar at constant bitrate first; a codec that refused one
        // is thrown away (a failed configure leaves it unusable) and the
        // next is asked of a fresh one
        let mut last = String::new();
        for (color, mode) in [
            (NV12, Some(CBR)),
            (NV12, None),
            (I420, Some(CBR)),
            (I420, None),
        ] {
            let codec = Raw::encoder()?;
            if let Err(e) = codec.configure(&format(color, mode)) {
                last = e;
                continue;
            }
            // what the codec echoes back: its stride and slice height, when
            // it has them (and the call exists: API 28)
            let took = codec.input_format();
            let get = |key: &str| {
                took.as_ref()
                    .and_then(|took| took.i32(key))
                    .filter(|v| *v > 0)
                    .map(|v| v as usize)
            };
            let input = Input {
                color: get("color-format").map_or(color, |v| v as i32),
                stride: get("stride").unwrap_or(width as usize).max(width as usize),
                slice: get("slice-height")
                    .unwrap_or(height as usize)
                    .max(height as usize),
            };
            codec.start()?;
            return Ok(Self {
                codec,
                width: width as usize,
                height: height as usize,
                input,
                sets: Vec::new(),
                yuv: Vec::new(),
            });
        }
        Err(format!("H.264 encoder configure: {last}"))
    }

    pub(super) fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        let mut params = MediaFormat::new();
        params.set_i32("video-bitrate", bitrate as i32);
        self.codec.set_parameters(&params)
    }

    pub(super) fn keyframe(&mut self) {
        let mut params = MediaFormat::new();
        params.set_i32("request-sync", 0);
        let _ = self.codec.set_parameters(&params);
    }

    pub(super) fn encode(
        &mut self,
        bgra: &[u8],
        at: Duration,
    ) -> Result<Option<VideoPacket>, String> {
        // the codec is full: this picture is dropped, as the rate control
        // would
        let Some(index) = self.codec.dequeue_input(WAIT)? else {
            return Ok(None);
        };
        bgra_to_yuv(bgra, self.width, self.height, self.input, &mut self.yuv);
        let room = self.codec.input_buffer(index)?;
        if room.len() < self.yuv.len() {
            return Err(format!(
                "an input buffer of {} bytes for {}",
                room.len(),
                self.yuv.len()
            ));
        }
        room[..self.yuv.len()].copy_from_slice(&self.yuv);
        self.codec
            .queue_input(index, self.yuv.len(), at.as_micros() as u64)?;
        loop {
            let (index, info) = match self.codec.dequeue_output(WAIT)? {
                Output::Buffer(index, info) => (index, info),
                Output::TryAgainLater => return Ok(None),
                Output::Changed => continue,
            };
            let (offset, size) = (info.offset.max(0) as usize, info.size.max(0) as usize);
            let bytes = self
                .codec
                .output_buffer(index)?
                .get(offset..offset + size)
                .unwrap_or(&[])
                .to_vec();
            self.codec.release_output(index)?;
            if info.flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG != 0 {
                self.sets = bytes;
                continue;
            }
            if bytes.is_empty() {
                continue;
            }
            let keyframe = info.flags & AMEDIACODEC_BUFFER_FLAG_KEY_FRAME != 0;
            let has_sets = nal_units(&bytes).any(|unit| nal_type(unit) == NAL_SPS);
            let data = match keyframe && !has_sets {
                true => [&self.sets[..], &bytes[..]].concat(),
                false => bytes,
            };
            return Ok(Some(VideoPacket { data, keyframe }));
        }
    }
}

/// What a dequeue of the output gives.
enum Output {
    Buffer(usize, ndk_sys::AMediaCodecBufferInfo),
    TryAgainLater,
    /// The format or the buffers changed: nothing to read this time.
    Changed,
}

/// The encoder's `AMediaCodec` by the NDK's own calls: the `ndk` crate's
/// wrapper keeps its pointer to itself, and the input format (the
/// stride and slice height the codec wants) is a call that exists from
/// API 28, looked up at run time since the app runs from 26.
struct Raw(std::ptr::NonNull<ndk_sys::AMediaCodec>);

impl Raw {
    fn encoder() -> Result<Self, String> {
        // SAFETY: a static C string
        let codec = unsafe { ndk_sys::AMediaCodec_createEncoderByType(c"video/avc".as_ptr()) };
        std::ptr::NonNull::new(codec)
            .map(Self)
            .ok_or_else(|| "no H.264 encoder on this device".into())
    }

    fn status(code: ndk_sys::media_status_t, what: &str) -> Result<(), String> {
        match code {
            ndk_sys::media_status_t::AMEDIA_OK => Ok(()),
            code => Err(format!("H.264 encoder {what}: {}", code.0)),
        }
    }

    fn configure(&self, format: &MediaFormat) -> Result<(), String> {
        // SAFETY: a live codec and format; no surface, no crypto
        let code = unsafe {
            ndk_sys::AMediaCodec_configure(
                self.0.as_ptr(),
                format.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                ndk_sys::AMEDIACODEC_CONFIGURE_FLAG_ENCODE as u32,
            )
        };
        Self::status(code, "configure")
    }

    fn start(&self) -> Result<(), String> {
        // SAFETY: a configured codec
        Self::status(
            unsafe { ndk_sys::AMediaCodec_start(self.0.as_ptr()) },
            "start",
        )
    }

    fn set_parameters(&self, params: &MediaFormat) -> Result<(), String> {
        // SAFETY: a started codec and a live format
        let code = unsafe { ndk_sys::AMediaCodec_setParameters(self.0.as_ptr(), params.as_ptr()) };
        Self::status(code, "parameters")
    }

    fn input_format(&self) -> Option<MediaFormat> {
        use std::ffi::{c_char, c_void};
        unsafe extern "C" {
            fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        }
        type GetInputFormat =
            unsafe extern "C" fn(*mut ndk_sys::AMediaCodec) -> *mut ndk_sys::AMediaFormat;
        // SAFETY: RTLD_DEFAULT (null) and a static name; the symbol, when
        // there, has this signature (the NDK's media/NdkMediaCodec.h);
        // the format returned is ours to free (`MediaFormat` does)
        unsafe {
            let symbol = dlsym(std::ptr::null_mut(), c"AMediaCodec_getInputFormat".as_ptr());
            let get: GetInputFormat = std::mem::transmute(std::ptr::NonNull::new(symbol)?);
            let format = std::ptr::NonNull::new(get(self.0.as_ptr()))?;
            Some(MediaFormat::from_ptr(format))
        }
    }

    fn dequeue_input(&self, wait: Duration) -> Result<Option<usize>, String> {
        // SAFETY: a started codec
        let index = unsafe {
            ndk_sys::AMediaCodec_dequeueInputBuffer(self.0.as_ptr(), wait.as_micros() as i64)
        };
        match index {
            i if i >= 0 => Ok(Some(i as usize)),
            i if i == ndk_sys::AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize => Ok(None),
            i => Err(format!("H.264 encoder input: {i}")),
        }
    }

    fn input_buffer(&mut self, index: usize) -> Result<&mut [u8], String> {
        let mut size = 0usize;
        // SAFETY: an index the codec just dequeued; the buffer is the
        // codec's, `size` bytes, ours until queued
        let buffer =
            unsafe { ndk_sys::AMediaCodec_getInputBuffer(self.0.as_ptr(), index, &mut size) };
        if buffer.is_null() {
            return Err("H.264 encoder: no input buffer".into());
        }
        Ok(unsafe { std::slice::from_raw_parts_mut(buffer, size) })
    }

    fn queue_input(&self, index: usize, size: usize, at_us: u64) -> Result<(), String> {
        // SAFETY: an index dequeued and filled
        let code = unsafe {
            ndk_sys::AMediaCodec_queueInputBuffer(self.0.as_ptr(), index, 0, size, at_us, 0)
        };
        Self::status(code, "queue")
    }

    fn dequeue_output(&self, wait: Duration) -> Result<Output, String> {
        let mut info = std::mem::MaybeUninit::uninit();
        // SAFETY: a started codec; `info` is written when a buffer comes
        let index = unsafe {
            ndk_sys::AMediaCodec_dequeueOutputBuffer(
                self.0.as_ptr(),
                info.as_mut_ptr(),
                wait.as_micros() as i64,
            )
        };
        match index {
            i if i >= 0 => Ok(Output::Buffer(i as usize, unsafe { info.assume_init() })),
            i if i == ndk_sys::AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize => {
                Ok(Output::TryAgainLater)
            }
            i if i == ndk_sys::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize
                || i == ndk_sys::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize =>
            {
                Ok(Output::Changed)
            }
            i => Err(format!("H.264 encoder output: {i}")),
        }
    }

    fn output_buffer(&self, index: usize) -> Result<&[u8], String> {
        let mut size = 0usize;
        // SAFETY: an index the codec just dequeued; read until released
        let buffer =
            unsafe { ndk_sys::AMediaCodec_getOutputBuffer(self.0.as_ptr(), index, &mut size) };
        if buffer.is_null() {
            return Err("H.264 encoder: no output buffer".into());
        }
        Ok(unsafe { std::slice::from_raw_parts(buffer, size) })
    }

    fn release_output(&self, index: usize) -> Result<(), String> {
        // SAFETY: an index dequeued, not rendered anywhere
        let code =
            unsafe { ndk_sys::AMediaCodec_releaseOutputBuffer(self.0.as_ptr(), index, false) };
        Self::status(code, "release")
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: made by `createEncoderByType`, stopped and freed once
        unsafe {
            ndk_sys::AMediaCodec_stop(self.0.as_ptr());
            ndk_sys::AMediaCodec_delete(self.0.as_ptr());
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
                // V's plane a quarter of the luma's size after U's
                let half = self.stride / 2;
                let (u, v) = chroma.split_at(self.stride * self.slice / 4);
                Box::new(move |x, y| (u[(y / 2) * half + x / 2], v[(y / 2) * half + x / 2]))
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
    /// The device's default decoder gave a layout this build does not
    /// read: Android's own software decoder from the next keyframe on.
    software: bool,
    frames: u64,
}

impl Decoder {
    pub(super) fn new() -> Result<Self, String> {
        // a decoder is made per stream (its sets); only that one exists
        // is checked here
        MediaCodec::from_decoder_type(MIME).ok_or("no H.264 decoder on this device")?;
        Ok(Self {
            stream: None,
            software: false,
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
                codec: open(sps, pps, self.software)?,
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
            let picture = layout.picture(&bytes);
            if picture.is_err() && !self.software {
                // a layout this build does not read: the software decoder
                // from the next keyframe on
                self.software = true;
                self.stream = None;
            }
            return picture.map(Some);
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

/// A decoder started on the parameter sets: the device's default, or
/// Android's own software one (`software`; its output is a layout read
/// here). The size is a hint the decoder corrects from the sets (its
/// output format says the real one).
fn open(sps: &[u8], pps: &[u8], software: bool) -> Result<MediaCodec, String> {
    let codec = match software {
        true => MediaCodec::from_codec_name("c2.android.avc.decoder")
            .or_else(|| MediaCodec::from_codec_name("OMX.google.h264.decoder")),
        false => None,
    }
    .or_else(|| MediaCodec::from_decoder_type(MIME))
    .ok_or("no H.264 decoder on this device")?;
    let mut format = MediaFormat::new();
    format.set_str("mime", MIME);
    format.set_i32("width", 640);
    format.set_i32("height", 360);
    // a keyframe at the ladder's top is well under this
    format.set_i32("max-input-size", 1 << 20);
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
