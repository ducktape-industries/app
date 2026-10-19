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
//! - picture: a camera's BGRA picture is scaled to the encoded size once,
//!   in [`VideoEncoder::encode`], and handed to the OS's own H.264, the one
//!   its vendor licenses: VideoToolbox on macOS and iOS
//!   (`videotoolbox.rs`), MediaCodec on Android (`mediacodec.rs`), and on
//!   Linux Cisco's prebuilt OpenH264, fetched onto the device by the app
//!   ([`cisco`]) and loaded at run time (`openh264.rs`). Nothing here
//!   compiles H.264 code. One stream shape on every OS, so each decodes
//!   the others': Constrained Baseline, no frame reordering, Annex B
//!   start codes, the parameter sets in front of every keyframe. A
//!   keyframe comes first, after a size change and when asked
//!   ([`VideoEncoder::keyframe`]); a receiver that joins, or lost a
//!   packet, asks, and a periodic one (the host's bound on how long a
//!   receiver waits) is the host's timer asking. A backend may add its own
//!   at that same bound (MediaCodec takes no "never").
use std::time::Duration;

use crate::Picture;

#[cfg(target_os = "android")]
mod mediacodec;
#[cfg(target_os = "linux")]
mod openh264;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod videotoolbox;

#[cfg(target_os = "android")]
use mediacodec as h264;
#[cfg(target_os = "linux")]
use openh264 as h264;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use videotoolbox as h264;

/// Where Cisco's OpenH264 binary is, on an OS whose H.264 it is (Linux).
/// Cisco's patent licence covers only the build Cisco made,
/// fetched by the user's own device, which the user can turn off: the app
/// owns that (the download, the switch) and [`supply`]s the path; an
/// encoder or decoder asks for it when made, so a switch turned off holds
/// from the next one on. Nothing supplied, or an error, is a build that
/// sends and shows no picture: the call goes on by voice.
pub mod cisco {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    type Provider = dyn Fn() -> Result<PathBuf, String> + Send + Sync;

    static PROVIDER: OnceLock<Box<Provider>> = OnceLock::new();

    /// Where the library is, or why it is not (turned off, not fetched):
    /// asked on every encoder's or decoder's making. The first supplier
    /// holds.
    pub fn supply(provider: impl Fn() -> Result<PathBuf, String> + Send + Sync + 'static) {
        let _ = PROVIDER.set(Box::new(provider));
    }

    #[cfg(target_os = "linux")]
    pub(super) fn library() -> Result<PathBuf, String> {
        PROVIDER
            .get()
            .ok_or_else(|| "no OpenH264 library was supplied".to_string())?()
    }
}

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
/// The largest packet one frame codes to: its TOC byte and at most 1275
/// bytes of frame (RFC 6716, 3.2.1).
const MAX_PACKET: usize = 1276;
/// The packet loss, in percent, FEC is coded for until
/// [`AudioEncoder::set_loss`] says otherwise.
const LOSS: u8 = 10;

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

    /// From the next packet on. libopus keeps it within 500 to 300 000
    /// (mono's cap) and refuses 0.
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
    /// before `next` itself is decoded. When it fails (`next` is not a
    /// packet libopus reads), `lost(None)` conceals the frame instead.
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
    encoder: h264::Encoder,
    width: u32,
    height: u32,
    bitrate: u32,
    /// The pictures a second the camera gives: what the rate control
    /// spreads the bitrate over.
    fps: u32,
    /// A picture scaled to the encoded size, kept for the next.
    scaled: Vec<u8>,
}

impl VideoEncoder {
    /// Encodes pictures at `width` by `height` (each rounded down to
    /// even) and `bitrate` bits a second, from a camera giving `fps`
    /// pictures a second. An error is a build, or a device, that encodes
    /// no picture: the caller goes on by voice.
    pub fn new(width: u32, height: u32, bitrate: u32, fps: u32) -> Result<Self, String> {
        let (width, height) = even(width, height)?;
        if fps == 0 {
            return Err("a camera giving no pictures a second".into());
        }
        Ok(Self {
            encoder: h264::Encoder::new(width, height, bitrate, fps)?,
            width,
            height,
            bitrate,
            fps,
            scaled: Vec::new(),
        })
    }

    /// From the next picture on.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        self.encoder.set_bitrate(bitrate)?;
        self.bitrate = bitrate;
        Ok(())
    }

    /// The encoded size and the pictures a second the camera now gives;
    /// a change holds from the next picture on, a keyframe. The picture
    /// is scaled to the size, its proportions not kept: a size of the
    /// camera's proportions keeps them.
    pub fn set_size(&mut self, width: u32, height: u32, fps: u32) -> Result<(), String> {
        let (width, height) = even(width, height)?;
        if (width, height, fps) != (self.width, self.height, self.fps) {
            // a new encoder: a session is made at one size
            *self = Self::new(width, height, self.bitrate, fps)?;
        }
        Ok(())
    }

    /// The next picture is a keyframe.
    pub fn keyframe(&mut self) {
        self.encoder.keyframe();
    }

    /// `picture` (any size; scaled to the encoded one) taken `at` (from
    /// the call's first picture on: the rate control's clock). A camera's
    /// picture of a person facing it may be mirrored for them: the one
    /// sent should not be. `None`: the rate control skipped the picture
    /// to keep to the bitrate, or the encoder holds it for the next call.
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
        self.encoder.encode(bgra, at)
    }
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
pub struct VideoDecoder(h264::Decoder);

impl VideoDecoder {
    pub fn new() -> Result<Self, String> {
        h264::Decoder::new().map(Self)
    }

    /// A packet's picture, BGRA as a camera's. `None`: the packet ended no
    /// picture (or the decoder holds it for the next call). The pictures
    /// after a lost packet are wrong until the next keyframe: the sender
    /// is asked for one ([`VideoEncoder::keyframe`]).
    pub fn decode(&mut self, packet: &[u8]) -> Result<Option<Picture>, String> {
        self.0.decode(packet)
    }
}

/// The NAL units of an Annex B stream, each without its start code, in
/// order. A peer chose the bytes: anything is read, nothing panics.
/// (OpenH264 reads its own: unused on Linux.)
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|&next| {
            // the start code's own zeros (and a 4-byte code's leading one)
            // belong to no unit
            let mut end = next - 3;
            while end > 0 && stream[end - 1] == 0 {
                end -= 1;
            }
            end
        })
        .chain(std::iter::once(stream.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .filter(|(start, end)| end > start)
        .map(move |(start, end)| &stream[start..end])
}

/// A NAL unit's type (its first byte's low five bits).
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn nal_type(unit: &[u8]) -> u8 {
    unit.first().map_or(0, |byte| byte & 0x1f)
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) const NAL_SPS: u8 = 7;
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) const NAL_PPS: u8 = 8;

#[cfg(test)]
pub(crate) mod tests {
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

    /// The H.264 backend ready to be made. On Linux that is Cisco's binary:
    /// the one `DUCKTAPE_OPENH264` names, else the one for this machine
    /// fetched once from Cisco into the temp dir (curl and bzip2, as the
    /// app does in Rust).
    pub(crate) fn ready() {
        #[cfg(target_os = "linux")]
        super::cisco::supply(|| {
            use std::path::PathBuf;
            use std::process::Command;
            if let Some(path) = std::env::var_os("DUCKTAPE_OPENH264") {
                return Ok(PathBuf::from(path));
            }
            let name = match std::env::consts::ARCH {
                "x86_64" => "libopenh264-2.6.0-linux64.8.so",
                "aarch64" => "libopenh264-2.6.0-linux-arm64.8.so",
                other => return Err(format!("no Cisco build for {other}")),
            };
            // one fetch, however many tests ask at once
            static FETCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _one = FETCH
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let dir = std::env::temp_dir().join("ducktape-openh264");
            let path = dir.join(name);
            if path.exists() {
                return Ok(path);
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let packed = dir.join(format!("{name}.part.bz2"));
            let url = format!("https://ciscobinary.openh264.org/{name}.bz2");
            let run = |program: &str, args: &[&str]| -> Result<(), String> {
                let status = Command::new(program)
                    .args(args)
                    .status()
                    .map_err(|e| format!("{program}: {e}"))?;
                status.success().then_some(()).ok_or(format!(
                    "{program} failed ({status}); set DUCKTAPE_OPENH264 to Cisco's libopenh264"
                ))
            };
            run("curl", &["-sSfo", &packed.display().to_string(), &url])?;
            run("bzip2", &["-df", &packed.display().to_string()])?;
            std::fs::rename(dir.join(format!("{name}.part")), &path).map_err(|e| e.to_string())?;
            Ok(path)
        });
    }

    #[test]
    fn a_picture_comes_back_through_h264() {
        ready();
        let mut encoder = VideoEncoder::new(320, 240, 1_000_000, 30).unwrap();
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

    /// The first packet is a keyframe, and the one asked for at the
    /// seventh picture comes then (or one call later, on a codec that
    /// holds a picture), and no other.
    #[test]
    fn a_keyframe_comes_first_and_when_asked() {
        ready();
        let mut encoder = VideoEncoder::new(160, 120, 500_000, 30).unwrap();
        let mut keys = Vec::new();
        let mut first = None;
        for n in 0..12 {
            if n == 6 {
                encoder.keyframe();
            }
            let packet = encoder
                .encode(&pattern(160, 120, n as u32), frame(n))
                .unwrap();
            if let Some(packet) = packet {
                first.get_or_insert(n);
                if packet.keyframe {
                    keys.push(n);
                }
            }
        }
        assert_eq!(keys.len(), 2, "keyframes at {keys:?}");
        assert_eq!(Some(keys[0]), first, "the first packet is a keyframe");
        assert!(keys[1] == 6 || keys[1] == 7, "keyframes at {keys:?}");
    }

    #[test]
    fn a_video_bitrate_change_takes_effect_down_and_up() {
        ready();
        let mut encoder = VideoEncoder::new(160, 120, 1_000_000, 30).unwrap();
        let mut sizes = Vec::new();
        for n in 0..180u64 {
            match n {
                60 => encoder.set_bitrate(100_000).unwrap(),
                120 => encoder.set_bitrate(1_000_000).unwrap(),
                _ => {}
            }
            let packet = encoder
                .encode(&grainy(160, 120, n as u32), frame(n))
                .unwrap();
            sizes.push(packet.map_or(0, |packet| packet.data.len()));
        }
        // the last second of each two, past the first's settling (a
        // hardware rate control moves over a second or so)
        let spent = |from: usize| -> usize { sizes[from..from + 30].iter().sum() };
        let (high, low, again) = (spent(30), spent(90), spent(150));
        assert!(low * 4 < high, "{high} bytes at 1 Mb/s, {low} at 100 kb/s");
        assert!(
            low * 4 < again,
            "{low} bytes at 100 kb/s, {again} at 1 Mb/s again"
        );
        // and near 100 kb/s: a second of it is 12.5 kB
        assert!(low < 24_000, "{low} bytes in 1 s at 100 kb/s");
    }

    /// A camera giving 15 pictures a second gets its whole bitrate: each
    /// picture's share is the bitrate over 15, not over 30.
    #[test]
    fn a_slower_camera_lands_near_the_bitrate() {
        ready();
        let bitrate = 300_000;
        for fps in [15u64, 30] {
            let mut encoder = VideoEncoder::new(160, 120, bitrate, fps as u32).unwrap();
            let mut bytes = 0;
            for n in 0..fps * 3 {
                let at = Duration::from_millis(n * 1000 / fps);
                let packet = encoder.encode(&grainy(160, 120, n as u32), at).unwrap();
                // the last two seconds, past the first one's settling
                if n >= fps {
                    bytes += packet.map_or(0, |packet| packet.data.len());
                }
            }
            let rate = bytes * 8 / 2;
            assert!(
                rate < bitrate as usize * 6 / 5,
                "{rate} b/s at {fps} pictures a second, for {bitrate}"
            );
            // OpenH264 spends what it is given; a hardware encoder may
            // stop short of it on a small, easy picture
            #[cfg(target_os = "linux")]
            assert!(
                rate > bitrate as usize * 4 / 5,
                "{rate} b/s at {fps} pictures a second, for {bitrate}"
            );
        }
    }

    #[test]
    fn a_picture_is_scaled_to_the_size_set_then() {
        ready();
        let mut encoder = VideoEncoder::new(161, 121, 500_000, 30).unwrap();
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
        encoder.set_size(320, 240, 30).unwrap();
        let packet = encoder.encode(&camera, frame(2)).unwrap().unwrap();
        assert!(packet.keyframe);
        let full = decoder.decode(&packet.data).unwrap().unwrap();
        assert_eq!((full.width, full.height), (320, 240));
        assert!(VideoEncoder::new(1, 240, 500_000, 30).is_err());
        assert!(VideoEncoder::new(320, 240, 500_000, 0).is_err());
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

    /// Each backend's stream, as its encoder wrote it (`write_golden`):
    /// 8 pictures of `pattern(160, 120, n)` at 2 Mb/s from a keyframe,
    /// each packet after its length. What every other backend's decoder
    /// must read.
    const GOLDENS: [(&str, &[u8]); 3] = [
        ("openh264", include_bytes!("codec/golden/openh264.h264")),
        (
            "videotoolbox",
            include_bytes!("codec/golden/videotoolbox.h264"),
        ),
        ("mediacodec", include_bytes!("codec/golden/mediacodec.h264")),
    ];
    const GOLDEN_PICTURES: u32 = 8;

    fn golden_packets(stream: &[u8]) -> Vec<&[u8]> {
        let mut packets = Vec::new();
        let mut rest = stream;
        while rest.len() >= 4 {
            let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
            packets.push(&rest[4..4 + len]);
            rest = &rest[4 + len..];
        }
        packets
    }

    /// This backend's golden stream to the file `DUCKTAPE_H264_GOLDEN`
    /// names: run on each OS when the encoder's output changes.
    #[test]
    #[ignore]
    fn write_golden() {
        ready();
        let path = std::env::var("DUCKTAPE_H264_GOLDEN").expect("DUCKTAPE_H264_GOLDEN");
        let mut encoder = VideoEncoder::new(160, 120, 2_000_000, 30).unwrap();
        let mut stream = Vec::new();
        for n in 0..GOLDEN_PICTURES {
            let packet = encoder
                .encode(&pattern(160, 120, n), frame(n.into()))
                .unwrap();
            if let Some(packet) = packet {
                stream.extend((packet.data.len() as u32).to_be_bytes());
                stream.extend(packet.data);
            }
        }
        std::fs::write(path, stream).unwrap();
    }

    /// Every backend's stream decodes here: at least six of its eight
    /// pictures come back (a codec may hold the last), each one of the
    /// pictures sent, the first the first.
    #[test]
    fn every_backends_stream_decodes_here() {
        ready();
        for (name, stream) in GOLDENS {
            let mut decoder = VideoDecoder::new().unwrap();
            let mut decoded = Vec::new();
            for packet in golden_packets(stream) {
                if let Some(picture) = decoder.decode(packet).unwrap() {
                    decoded.push(picture);
                }
            }
            assert!(
                decoded.len() >= 6,
                "{name}: {} pictures decoded",
                decoded.len()
            );
            for (i, picture) in decoded.iter().enumerate() {
                assert_eq!(
                    (picture.width, picture.height),
                    (160, 120),
                    "{name}: picture {i}"
                );
                let best = (0..GOLDEN_PICTURES)
                    .map(|n| psnr(&pattern(160, 120, n), picture))
                    .fold(f64::MIN, f64::max);
                assert!(
                    best > 28.,
                    "{name}: picture {i} is none sent (PSNR {best:.1} dB)"
                );
            }
            let first = psnr(&pattern(160, 120, 0), &decoded[0]);
            assert!(
                first > 28.,
                "{name}: the first picture (PSNR {first:.1} dB)"
            );
        }
    }

    /// Start codes of 3 and 4 bytes, a unit's trailing zeros kept out of
    /// the next, nothing before the first code, nothing for garbage.
    #[test]
    fn annex_b_units_are_split_at_their_start_codes() {
        let stream = [
            9, 0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 0, 1, 0x65, 4, 5,
        ];
        let units: Vec<&[u8]> = nal_units(&stream).collect();
        assert_eq!(units, [&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 4, 5]]);
        assert_eq!(
            units.iter().map(|u| nal_type(u)).collect::<Vec<_>>(),
            [7, 8, 5]
        );
        assert_eq!(nal_units(&[0, 0, 1]).count(), 0);
        assert_eq!(nal_units(&[1, 2, 3]).count(), 0);
        assert_eq!(nal_type(&[]), 0);
    }

    /// A slice of a peer's garbage after a good keyframe: whatever the
    /// decoder says of it, the next keyframe decodes.
    #[test]
    fn a_bad_slice_does_not_end_the_decoder() {
        ready();
        let (_, stream) = GOLDENS[0];
        let packets = golden_packets(stream);
        let mut decoder = VideoDecoder::new().unwrap();
        assert!(decoder.decode(packets[0]).unwrap().is_some());
        let mut bad = vec![0, 0, 0, 1, 0x41];
        bad.extend(noise(16, 16, 3).bgra);
        let _ = decoder.decode(&bad);
        let _ = decoder.decode(packets[1]);
        let mut again = None;
        for packet in &packets[..2] {
            again = decoder.decode(packet).unwrap().or(again);
        }
        assert!(again.is_some(), "no picture after a bad slice");
    }

    #[test]
    fn no_garbage_breaks_the_video_decoder() {
        ready();
        let mut decoder = VideoDecoder::new().unwrap();
        for seed in 0..64 {
            let mut bytes = noise(16, 16, seed).bgra;
            // some behind a start code, as a NAL unit would be
            if seed.is_multiple_of(2) {
                bytes[..4].copy_from_slice(&[0, 0, 0, 1]);
            }
            let _ = decoder.decode(&bytes);
        }
        let mut encoder = VideoEncoder::new(160, 120, 500_000, 30).unwrap();
        let packet = encoder
            .encode(&pattern(160, 120, 0), frame(0))
            .unwrap()
            .unwrap();
        assert!(decoder.decode(&packet.data).unwrap().is_some());
    }
}
