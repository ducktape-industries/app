//! A call's codecs: a person's voice as Opus and their camera as H.264,
//! and back. Pure computation over what the capture side gives (mono
//! `f32` samples, BGRA [`Picture`]s) and what a camera's painter draws (a
//! BGRA `Picture`): no device, no thread, no gpui, nothing of the wire.
//! The host's call pipeline feeds captured samples and pictures in and
//! puts the packets on the wire, and the reverse.
//!
//! - voice: 48 kHz mono in 20 ms frames ([`FRAME`] samples), in-band FEC
//!   on: each packet also carries a coarse copy of the frame before it, so
//!   a lost packet is rebuilt from the one after it, and concealed from
//!   what came before when that has not come either
//!   ([`AudioDecoder::lost`]).
//! - picture: a camera's BGRA picture is scaled to the encoded size and
//!   turned into I420 once, in [`VideoEncoder::encode`]. A keyframe comes
//!   first, after a size change and when asked ([`VideoEncoder::keyframe`]),
//!   never on a timer: a receiver that joins, or lost a packet, asks.
//!
//! ponytail: software codecs on every OS (libopus and OpenH264 built from
//! source). The hardware H.264 of the phones and macOS (VideoToolbox,
//! MediaCodec) is the upgrade when a phone's battery or heat says so; it
//! also brings the OS vendor's patent licence, which a source build of
//! OpenH264 lacks.
use std::time::Duration;

use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode};
use openh264::formats::{BgraSliceU8, YUVBuffer, YUVSource};
use openh264::{OpenH264API, Timestamp, decoder::Decoder};
use openh264_sys2::{
    ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SBitrateInfo, SPATIAL_LAYER_0,
};

use crate::Picture;

/// The part of libopus (vendored in `opus/`, built by `build.rs`) this
/// file calls, as `include/opus.h` and `opus_defines.h` declare it.
mod opus {
    use std::ffi::{c_char, c_int, c_uchar};

    #[repr(C)]
    pub struct OpusEncoder {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    pub struct OpusDecoder {
        _opaque: [u8; 0],
    }

    pub const OPUS_OK: c_int = 0;
    pub const OPUS_APPLICATION_VOIP: c_int = 2048;
    pub const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
    pub const OPUS_SET_INBAND_FEC_REQUEST: c_int = 4012;
    pub const OPUS_SET_PACKET_LOSS_PERC_REQUEST: c_int = 4014;

    unsafe extern "C" {
        pub fn opus_encoder_create(
            fs: i32,
            channels: c_int,
            application: c_int,
            error: *mut c_int,
        ) -> *mut OpusEncoder;
        pub fn opus_encode_float(
            st: *mut OpusEncoder,
            pcm: *const f32,
            frame_size: c_int,
            data: *mut c_uchar,
            max_data_bytes: i32,
        ) -> i32;
        pub fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
        pub fn opus_encoder_destroy(st: *mut OpusEncoder);
        pub fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int)
        -> *mut OpusDecoder;
        pub fn opus_decode_float(
            st: *mut OpusDecoder,
            data: *const c_uchar,
            len: i32,
            pcm: *mut f32,
            frame_size: c_int,
            decode_fec: c_int,
        ) -> c_int;
        pub fn opus_decoder_destroy(st: *mut OpusDecoder);
        pub fn opus_strerror(error: c_int) -> *const c_char;
    }
}

/// Samples a second, in and out.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples in one packet: 20 ms.
pub const FRAME: usize = 960;
/// The largest packet one frame codes to (RFC 6716, 3.4).
const MAX_PACKET: usize = 1275;
/// The packet loss, in percent, FEC is coded for until
/// [`AudioEncoder::set_loss`] says otherwise.
const LOSS: u8 = 10;
/// The pictures a second a camera gives at most: what the video rate
/// control spreads the bitrate over.
const FPS: f32 = 30.0;

/// A person's voice into packets, one per 20 ms.
pub struct AudioEncoder(*mut opus::OpusEncoder);

// SAFETY: libopus's state is memory the encoder alone owns, with no tie to
// the thread that made it; `&mut self` keeps it to one caller at a time.
unsafe impl Send for AudioEncoder {}

impl AudioEncoder {
    /// An encoder at `bitrate` bits a second, coding FEC for 10% loss.
    pub fn new(bitrate: u32) -> Result<Self, String> {
        let mut error = 0;
        // SAFETY: a rate, a channel count and an application libopus
        // takes; `error` outlives the call
        let state = unsafe {
            opus::opus_encoder_create(
                SAMPLE_RATE as i32,
                1,
                opus::OPUS_APPLICATION_VOIP,
                &mut error,
            )
        };
        if state.is_null() {
            return Err(opus_error(error));
        }
        let mut encoder = Self(state);
        encoder.ctl(opus::OPUS_SET_INBAND_FEC_REQUEST, 1)?;
        encoder.set_loss(LOSS)?;
        encoder.set_bitrate(bitrate)?;
        Ok(encoder)
    }

    /// From the next packet on (libopus keeps it within 500 to 512 000).
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        self.ctl(
            opus::OPUS_SET_BITRATE_REQUEST,
            i32::try_from(bitrate).unwrap_or(i32::MAX),
        )
    }

    /// The packet loss, in percent (0 to 100), the FEC is coded for: the
    /// more, the more of each packet is the copy of the frame before it.
    /// 0 codes none.
    pub fn set_loss(&mut self, percent: u8) -> Result<(), String> {
        self.ctl(opus::OPUS_SET_PACKET_LOSS_PERC_REQUEST, i32::from(percent))
    }

    /// One 20 ms frame of samples, -1 to 1, into a packet.
    pub fn encode(&mut self, pcm: &[f32; FRAME]) -> Result<Vec<u8>, String> {
        let mut packet = vec![0; MAX_PACKET];
        // SAFETY: the state is live; `pcm` holds FRAME samples and `packet`
        // MAX_PACKET bytes
        let written = unsafe {
            opus::opus_encode_float(
                self.0,
                pcm.as_ptr(),
                FRAME as i32,
                packet.as_mut_ptr(),
                MAX_PACKET as i32,
            )
        };
        packet.truncate(counted(written)?);
        Ok(packet)
    }

    fn ctl(&mut self, request: i32, value: i32) -> Result<(), String> {
        // SAFETY: the state is live; every request sent here takes one i32
        match unsafe { opus::opus_encoder_ctl(self.0, request, value) } {
            opus::OPUS_OK => Ok(()),
            error => Err(opus_error(error)),
        }
    }
}

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        // SAFETY: made by `opus_encoder_create`, freed once
        unsafe { opus::opus_encoder_destroy(self.0) }
    }
}

/// Packets back into a person's voice, 20 ms each.
pub struct AudioDecoder(*mut opus::OpusDecoder);

// SAFETY: as the encoder's
unsafe impl Send for AudioDecoder {}

impl AudioDecoder {
    pub fn new() -> Result<Self, String> {
        let mut error = 0;
        // SAFETY: a rate and a channel count libopus takes; `error`
        // outlives the call
        let state = unsafe { opus::opus_decoder_create(SAMPLE_RATE as i32, 1, &mut error) };
        if state.is_null() {
            return Err(opus_error(error));
        }
        Ok(Self(state))
    }

    /// A packet's 20 ms. A peer chose its bytes: what libopus cannot
    /// decode, or decodes to other than 20 ms, is an error.
    pub fn decode(&mut self, packet: &[u8]) -> Result<[f32; FRAME], String> {
        self.run(Some(packet), false)
    }

    /// The 20 ms of a packet that never came: rebuilt from the copy `next`
    /// (the packet after it) carries, when that has come; concealed from
    /// what came before when not. Asked once per lost packet, in order,
    /// before `next` itself is decoded.
    pub fn lost(&mut self, next: Option<&[u8]>) -> Result<[f32; FRAME], String> {
        self.run(next, true)
    }

    fn run(&mut self, packet: Option<&[u8]>, fec: bool) -> Result<[f32; FRAME], String> {
        let (data, len) = match packet {
            // no bytes would read as no packet: concealed
            Some([]) => return Err("an empty packet".into()),
            Some(packet) => {
                let len = i32::try_from(packet.len()).map_err(|_| "a packet past 2 GB")?;
                (packet.as_ptr(), len)
            }
            // libopus conceals a frame it is given no packet for
            None => (std::ptr::null(), 0),
        };
        let mut pcm = [0.; FRAME];
        // SAFETY: the state is live; `data` is null or `len` bytes;
        // `pcm` holds FRAME samples
        let decoded = unsafe {
            opus::opus_decode_float(
                self.0,
                data,
                len,
                pcm.as_mut_ptr(),
                FRAME as i32,
                i32::from(fec),
            )
        };
        match counted(decoded)? {
            FRAME => Ok(pcm),
            other => Err(format!("{other} samples decoded, not {FRAME}")),
        }
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        // SAFETY: made by `opus_decoder_create`, freed once
        unsafe { opus::opus_decoder_destroy(self.0) }
    }
}

/// A libopus count, or its error.
fn counted(result: i32) -> Result<usize, String> {
    usize::try_from(result).map_err(|_| opus_error(result))
}

fn opus_error(code: i32) -> String {
    // SAFETY: libopus answers every code with a static C string
    let text = unsafe { std::ffi::CStr::from_ptr(opus::opus_strerror(code)) };
    format!("opus: {}", text.to_string_lossy())
}

/// One encoded picture.
#[derive(Clone, Debug)]
pub struct VideoPacket {
    /// H.264 in Annex B: the picture's NAL units, each after a start code
    /// (a keyframe's led by the parameter sets).
    pub data: Vec<u8>,
    /// An IDR picture: a decoder can start here.
    pub keyframe: bool,
}

/// A camera's pictures into packets.
pub struct VideoEncoder {
    encoder: Encoder,
    width: u32,
    height: u32,
    bitrate: u32,
    /// Nothing encoded since `encoder` was made: OpenH264 starts on its
    /// first picture and takes no option before.
    fresh: bool,
    yuv: YUVBuffer,
    /// A picture scaled to the encoded size, kept for the next.
    scaled: Vec<u8>,
}

impl VideoEncoder {
    /// Encodes pictures at `width` by `height` (each rounded down to
    /// even) and `bitrate` bits a second.
    pub fn new(width: u32, height: u32, bitrate: u32) -> Result<Self, String> {
        let (width, height) = even(width, height)?;
        Ok(Self {
            encoder: encoder(bitrate)?,
            width,
            height,
            bitrate,
            fresh: true,
            yuv: YUVBuffer::new(width as usize, height as usize),
            scaled: Vec::new(),
        })
    }

    /// From the next picture on.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        if self.fresh {
            self.encoder = encoder(bitrate)?;
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
                self.encoder
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

    /// From the next picture on, a keyframe. The picture is scaled to it,
    /// its proportions not kept: a size of the camera's proportions keeps
    /// them.
    pub fn set_size(&mut self, width: u32, height: u32) -> Result<(), String> {
        let (width, height) = even(width, height)?;
        if (width, height) != (self.width, self.height) {
            // a new encoder: OpenH264's own resize would drop a bitrate
            // set since it started
            *self = Self::new(width, height, self.bitrate)?;
        }
        Ok(())
    }

    /// The next picture is a keyframe.
    pub fn keyframe(&mut self) {
        // a fresh encoder's first picture is one anyway
        if !self.fresh {
            self.encoder.force_intra_frame();
        }
    }

    /// `picture` (any size; scaled to the encoded one) taken `at` (from
    /// the call's first picture on: the rate control's clock). A camera's
    /// picture of a person facing it may be mirrored for them: the one
    /// sent should not be. `None`: the rate control skipped the picture
    /// to keep to the bitrate.
    pub fn encode(
        &mut self,
        picture: &Picture,
        at: Duration,
    ) -> Result<Option<VideoPacket>, String> {
        let from = (picture.width as usize, picture.height as usize);
        if from.0 == 0 || from.1 == 0 || picture.bgra.len() != from.0 * from.1 * 4 {
            return Err(format!(
                "a {}x{} picture of {} bytes",
                from.0,
                from.1,
                picture.bgra.len()
            ));
        }
        let to = (self.width as usize, self.height as usize);
        let bgra = match from == to {
            true => &picture.bgra[..],
            false => {
                scale(picture, to, &mut self.scaled);
                &self.scaled[..]
            }
        };
        self.yuv.read_bgra8(BgraSliceU8::new(bgra, to));
        let at = Timestamp::from_millis(u64::try_from(at.as_millis()).unwrap_or(u64::MAX));
        let stream = self
            .encoder
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

/// An OpenH264 encoder for a call: the bitrate kept (frames skipped when
/// over it), at most [`FPS`] pictures a second.
fn encoder(bitrate: u32) -> Result<Encoder, String> {
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate))
        .rate_control_mode(RateControlMode::Bitrate)
        .max_frame_rate(FrameRate::from_hz(FPS));
    Encoder::with_api_config(OpenH264API::from_source(), config)
        .map_err(|error| format!("openh264: {error}"))
}

/// I420's sizes are even.
fn even(width: u32, height: u32) -> Result<(u32, u32), String> {
    match (width & !1, height & !1) {
        (0, _) | (_, 0) => Err(format!("a {width}x{height} picture is too small to encode")),
        size => Ok(size),
    }
}

/// `picture` at `to`'s size into `out`: each pixel the average of the
/// ones it covers (one, when enlarging).
fn scale(picture: &Picture, to: (usize, usize), out: &mut Vec<u8>) {
    let from = (picture.width as usize, picture.height as usize);
    out.clear();
    out.reserve(to.0 * to.1 * 4);
    for y in 0..to.1 {
        let rows = covered(y, to.1, from.1);
        for x in 0..to.0 {
            let columns = covered(x, to.0, from.0);
            let mut sum = [0u32; 4];
            for row in rows.clone() {
                let line = &picture.bgra[(row * from.0 + columns.start) * 4..][..columns.len() * 4];
                for pixel in line.chunks_exact(4) {
                    for (sum, &channel) in sum.iter_mut().zip(pixel) {
                        *sum += u32::from(channel);
                    }
                }
            }
            let count = (rows.len() * columns.len()) as u32;
            out.extend(sum.map(|sum| ((sum + count / 2) / count) as u8));
        }
    }
}

/// The source pixels target pixel `at` of `to` covers along a side of
/// `from`.
fn covered(at: usize, to: usize, from: usize) -> std::ops::Range<usize> {
    let start = at * from / to;
    start..((at + 1) * from / to).max(start + 1)
}

/// Packets back into pictures.
pub struct VideoDecoder(Decoder);

impl VideoDecoder {
    pub fn new() -> Result<Self, String> {
        Decoder::new()
            .map(Self)
            .map_err(|error| format!("openh264: {error}"))
    }

    /// A packet's picture, BGRA as a camera's. `None`: the packet ended no
    /// picture. The pictures after a lost packet are wrong until the next
    /// keyframe: the sender is asked for one ([`VideoEncoder::keyframe`]).
    pub fn decode(&mut self, packet: &[u8]) -> Result<Option<Picture>, String> {
        let Some(yuv) = self
            .0
            .decode(packet)
            .map_err(|error| format!("openh264: {error}"))?
        else {
            return Ok(None);
        };
        let (width, height) = yuv.dimensions();
        let mut bgra = vec![0; width * height * 4];
        yuv.write_rgba8(&mut bgra);
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Ok(Some(Picture {
            width: width as u32,
            height: height as u32,
            bgra,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frames` 20 ms frames of a `hz` tone at half scale, from sample
    /// `start` on.
    fn tone(hz: f32, start: usize, frames: usize) -> Vec<[f32; FRAME]> {
        (0..frames)
            .map(|frame| {
                std::array::from_fn(|i| {
                    let t = (start + frame * FRAME + i) as f32 / SAMPLE_RATE as f32;
                    0.5 * (t * hz * std::f32::consts::TAU).sin()
                })
            })
            .collect()
    }

    /// Pearson's correlation of two equal-length signals.
    fn correlation(a: &[f32], b: &[f32]) -> f32 {
        let mean = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
        let (ma, mb) = (mean(a), mean(b));
        let (mut ab, mut aa, mut bb) = (0., 0., 0.);
        for (x, y) in a.iter().zip(b) {
            ab += (x - ma) * (y - mb);
            aa += (x - ma) * (x - ma);
            bb += (y - mb) * (y - mb);
        }
        ab / (aa * bb).sqrt().max(f32::EPSILON)
    }

    #[test]
    fn a_tone_comes_back_through_opus() {
        let mut encoder = AudioEncoder::new(32_000).unwrap();
        let mut decoder = AudioDecoder::new().unwrap();
        let sent: Vec<f32> = tone(440., 0, 50).concat();
        let mut heard = Vec::new();
        for frame in sent.chunks_exact(FRAME) {
            let packet = encoder.encode(frame.try_into().unwrap()).unwrap();
            assert!(packet.len() < MAX_PACKET);
            heard.extend(decoder.decode(&packet).unwrap());
        }
        // the codec's delay is a few ms: the best lag within one frame,
        // past the first frames' settling
        let best = (0..FRAME)
            .map(|lag| {
                correlation(
                    &sent[FRAME * 5..FRAME * 45],
                    &heard[FRAME * 5 + lag..FRAME * 45 + lag],
                )
            })
            .fold(f32::MIN, f32::max);
        assert!(best > 0.95, "correlation {best}");
    }

    /// A tone that changes at the lost frame: concealment can only carry
    /// on the old one, FEC brings the new one.
    #[test]
    fn a_lost_packet_is_rebuilt_from_the_next_and_concealed_without_it() {
        let lost = 20;
        let mut sent = tone(300., 0, lost);
        // 725 Hz: 14.5 periods a frame, so the packet after, decoded as
        // itself, is out of phase with the lost one
        sent.extend(tone(725., lost * FRAME, 10));
        let mut encoder = AudioEncoder::new(24_000).unwrap();
        let packets: Vec<Vec<u8>> = sent
            .iter()
            .map(|frame| encoder.encode(frame).unwrap())
            .collect();

        let mut whole = AudioDecoder::new().unwrap();
        let mut rebuilt = AudioDecoder::new().unwrap();
        let mut concealed = AudioDecoder::new().unwrap();
        for packet in &packets[..lost] {
            whole.decode(packet).unwrap();
            rebuilt.decode(packet).unwrap();
            concealed.decode(packet).unwrap();
        }
        let truth = whole.decode(&packets[lost]).unwrap();
        let from_next = rebuilt.lost(Some(&packets[lost + 1])).unwrap();
        let guessed = concealed.lost(None).unwrap();
        // the frame's second half is the new tone in every decoder's timing
        let half = FRAME / 2..FRAME;
        let fec = correlation(&truth[half.clone()], &from_next[half.clone()]);
        let plc = correlation(&truth[half.clone()], &guessed[half.clone()]);
        assert!(fec > 0.8, "rebuilt from the next packet: correlation {fec}");
        assert!(plc < 0.5, "concealed: correlation {plc}");
        // both carry on decoding the packet after
        rebuilt.decode(&packets[lost + 1]).unwrap();
        concealed.decode(&packets[lost + 1]).unwrap();
    }

    /// A steady tone's lost frame, with nothing after it come yet, is
    /// carried on from the frames before.
    #[test]
    fn a_lost_packet_of_a_steady_tone_is_concealed() {
        let sent = tone(440., 0, 21);
        let mut encoder = AudioEncoder::new(24_000).unwrap();
        let packets: Vec<Vec<u8>> = sent
            .iter()
            .map(|frame| encoder.encode(frame).unwrap())
            .collect();
        let mut whole = AudioDecoder::new().unwrap();
        let mut concealed = AudioDecoder::new().unwrap();
        for packet in &packets[..20] {
            whole.decode(packet).unwrap();
            concealed.decode(packet).unwrap();
        }
        let truth = whole.decode(&packets[20]).unwrap();
        let guessed = concealed.lost(None).unwrap();
        let carried = correlation(&truth, &guessed);
        assert!(carried > 0.9, "concealed: correlation {carried}");
    }

    #[test]
    fn a_lower_audio_bitrate_takes_effect() {
        let mut encoder = AudioEncoder::new(64_000).unwrap();
        let frames = tone(440., 0, 100);
        let bytes = |encoder: &mut AudioEncoder, frames: &[[f32; FRAME]]| -> usize {
            frames
                .iter()
                .map(|frame| encoder.encode(frame).unwrap().len())
                .sum()
        };
        let high = bytes(&mut encoder, &frames[..50]);
        encoder.set_bitrate(12_000).unwrap();
        let low = bytes(&mut encoder, &frames[50..]);
        assert!(low * 3 < high, "{high} bytes at 64 kb/s, {low} at 12 kb/s");
    }

    /// Nothing a peer puts in a short packet reaches a crash, and the
    /// decoder decodes on after.
    #[test]
    fn no_short_packet_breaks_the_decoder() {
        let mut decoder = AudioDecoder::new().unwrap();
        assert!(decoder.decode(&[]).is_err());
        for toc in 0..=u8::MAX {
            for len in 1..=6usize {
                for fill in [0x00, 0x5a, 0xff] {
                    let mut packet = vec![fill; len];
                    packet[0] = toc;
                    let _ = decoder.decode(&packet);
                    let _ = decoder.lost(Some(&packet));
                }
            }
        }
        let mut encoder = AudioEncoder::new(32_000).unwrap();
        let packet = encoder.encode(&tone(440., 0, 1)[0]).unwrap();
        decoder.decode(&packet).unwrap();
        // a 40 ms packet is refused, not cut: given two frames' samples,
        // libopus codes them as one
        let long = AudioEncoder::new(32_000).unwrap();
        let mut twice = [0u8; MAX_PACKET];
        let pcm: Vec<f32> = tone(440., 0, 2).concat();
        // SAFETY: as `encode`, with two frames of samples
        let written = unsafe {
            opus::opus_encode_float(
                long.0,
                pcm.as_ptr(),
                2 * FRAME as i32,
                twice.as_mut_ptr(),
                MAX_PACKET as i32,
            )
        };
        let written = counted(written).unwrap();
        assert!(decoder.decode(&twice[..written]).is_err());
    }

    /// A picture with detail everywhere: diagonal stripes over gradients,
    /// moved along with `shift`.
    fn pattern(width: u32, height: u32, shift: u32) -> Picture {
        let mut bgra = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let stripe = if ((x + y + shift) / 8).is_multiple_of(2) {
                    60
                } else {
                    0
                };
                bgra.extend([
                    (x * 255 / width) as u8 / 2 + stripe,
                    (y * 255 / height) as u8 / 2 + stripe,
                    ((x + y) * 255 / (width + height)) as u8 / 2 + stripe,
                    255,
                ]);
            }
        }
        Picture {
            width,
            height,
            bgra,
        }
    }

    /// Noise that changes every picture: what no bitrate holds well.
    fn noise(width: u32, height: u32, seed: u32) -> Picture {
        let mut state = seed.wrapping_mul(2_654_435_761) | 1;
        let bgra = (0..width * height * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        Picture {
            width,
            height,
            bgra,
        }
    }

    /// The moving pattern under fresh grain: what a bitrate buys more or
    /// less of.
    fn grainy(width: u32, height: u32, n: u32) -> Picture {
        let mut picture = pattern(width, height, n);
        for (channel, grain) in picture.bgra.iter_mut().zip(noise(width, height, n).bgra) {
            *channel = channel.saturating_add(grain & 31);
        }
        picture
    }

    /// Peak signal to noise, in dB, over the colour channels.
    fn psnr(a: &Picture, b: &Picture) -> f64 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        let (mut sum, mut count) = (0f64, 0f64);
        for (x, y) in a.bgra.chunks_exact(4).zip(b.bgra.chunks_exact(4)) {
            for c in 0..3 {
                let d = f64::from(x[c]) - f64::from(y[c]);
                sum += d * d;
                count += 1.;
            }
        }
        10. * (255. * 255. / (sum / count).max(1e-9)).log10()
    }

    fn frame(n: u64) -> Duration {
        Duration::from_millis(n * 33)
    }

    #[test]
    fn a_picture_comes_back_through_h264() {
        let mut encoder = VideoEncoder::new(320, 240, 1_000_000).unwrap();
        let mut decoder = VideoDecoder::new().unwrap();
        let sent = pattern(320, 240, 0);
        let mut last = None;
        for n in 0..10 {
            if let Some(packet) = encoder.encode(&sent, frame(n)).unwrap() {
                last = decoder.decode(&packet.data).unwrap().or(last);
            }
        }
        let back = last.expect("a decoded picture");
        let quality = psnr(&sent, &back);
        assert!(quality > 30., "PSNR {quality:.1} dB");
        // a picture the decoder never saw scores low: the floor means it
        assert!(psnr(&pattern(320, 240, 8), &back) < 25.);
    }

    #[test]
    fn a_keyframe_comes_first_and_when_asked() {
        let mut encoder = VideoEncoder::new(160, 120, 500_000).unwrap();
        let mut kinds = Vec::new();
        for n in 0..12 {
            if n == 6 {
                encoder.keyframe();
            }
            let packet = encoder
                .encode(&pattern(160, 120, n as u32), frame(n))
                .unwrap();
            kinds.push(packet.expect("no picture skipped at this bitrate").keyframe);
        }
        let expected: Vec<bool> = (0..12).map(|n| n == 0 || n == 6).collect();
        assert_eq!(kinds, expected);
    }

    #[test]
    fn a_video_bitrate_change_takes_effect_down_and_up() {
        let mut encoder = VideoEncoder::new(160, 120, 1_000_000).unwrap();
        let mut sizes = Vec::new();
        for n in 0..90u64 {
            match n {
                30 => encoder.set_bitrate(100_000).unwrap(),
                60 => encoder.set_bitrate(1_000_000).unwrap(),
                _ => {}
            }
            let packet = encoder
                .encode(&grainy(160, 120, n as u32), frame(n))
                .unwrap();
            sizes.push(packet.map_or(0, |packet| packet.data.len()));
        }
        // two thirds of a second of each, past a third's settling
        let spent = |from: usize| -> usize { sizes[from..from + 20].iter().sum() };
        let (high, low, again) = (spent(10), spent(40), spent(70));
        assert!(low * 4 < high, "{high} bytes at 1 Mb/s, {low} at 100 kb/s");
        assert!(
            low * 4 < again,
            "{low} bytes at 100 kb/s, {again} at 1 Mb/s again"
        );
        // and near 100 kb/s: 2/3 s of it is 8.3 kB
        assert!(low < 16_000, "{low} bytes in 2/3 s at 100 kb/s");
    }

    #[test]
    fn a_picture_is_scaled_to_the_size_set_then() {
        let mut encoder = VideoEncoder::new(161, 121, 500_000).unwrap();
        let mut decoder = VideoDecoder::new().unwrap();
        let camera = pattern(320, 240, 0);
        let packet = encoder.encode(&camera, frame(0)).unwrap().unwrap();
        let small = decoder.decode(&packet.data).unwrap().unwrap();
        assert_eq!((small.width, small.height), (160, 120));
        let mut halved = Vec::new();
        scale(&camera, (160, 120), &mut halved);
        let expected = Picture {
            width: 160,
            height: 120,
            bgra: halved,
        };
        assert!(psnr(&expected, &small) > 30.);
        // a size change starts on a keyframe
        encoder.encode(&camera, frame(1)).unwrap();
        encoder.set_size(320, 240).unwrap();
        let packet = encoder.encode(&camera, frame(2)).unwrap().unwrap();
        assert!(packet.keyframe);
        let full = decoder.decode(&packet.data).unwrap().unwrap();
        assert_eq!((full.width, full.height), (320, 240));
        assert!(VideoEncoder::new(1, 240, 500_000).is_err());
    }

    #[test]
    fn scaling_averages_what_each_pixel_covers() {
        // 4x2 of greys 0, 40, 80, 120 over 160, 200, 240, 255, to 2x1
        let levels = [0u8, 40, 80, 120, 160, 200, 240, 255];
        let picture = Picture {
            width: 4,
            height: 2,
            bgra: levels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        };
        let mut out = Vec::new();
        scale(&picture, (2, 1), &mut out);
        assert_eq!(out, [100, 100, 100, 255, 174, 174, 174, 255]);
        scale(&picture, (8, 4), &mut out);
        assert_eq!(out.len(), 8 * 4 * 4);
        assert_eq!(&out[..8], &[0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn no_garbage_breaks_the_video_decoder() {
        let mut decoder = VideoDecoder::new().unwrap();
        for seed in 0..64 {
            let mut bytes = noise(16, 16, seed).bgra;
            // some behind a start code, as a NAL unit would be
            if seed.is_multiple_of(2) {
                bytes[..4].copy_from_slice(&[0, 0, 0, 1]);
            }
            let _ = decoder.decode(&bytes);
        }
        let mut encoder = VideoEncoder::new(160, 120, 500_000).unwrap();
        let packet = encoder
            .encode(&pattern(160, 120, 0), frame(0))
            .unwrap()
            .unwrap();
        assert!(decoder.decode(&packet.data).unwrap().is_some());
    }
}
