//! macOS and iOS: VideoToolbox's H.264, under Apple's licence. The
//! encoder takes the BGRA picture as is (a pixel buffer; the system
//! converts) and gives AVCC (4-byte lengths) with the parameter sets kept
//! apart in the format description: turned into the wire's Annex B with
//! the sets in front of each keyframe. The decoder takes Annex B back
//! apart: the sets make a format description and a session, the slices go
//! in as AVCC. Both run synchronous: a frame is complete when the call
//! returns (`complete_frames`; no asynchronous flag on decode).
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::time::Duration;

use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMTime,
    CMVideoFormatDescriptionCreateFromH264ParameterSets,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex, kCMBlockBufferAssureMemoryNowFlag,
    kCMSampleAttachmentKey_NotSync, kCMTimeInvalid, kCMVideoCodecType_H264,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTDecodeFrameFlags, VTDecodeInfoFlags,
    VTDecompressionOutputCallbackRecord, VTDecompressionSession, VTEncodeInfoFlags, VTSession,
    VTSessionSetProperty, kVTCompressionPropertyKey_AllowFrameReordering,
    kVTCompressionPropertyKey_AverageBitRate, kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_Baseline_AutoLevel,
    kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
};

use super::{NAL_PPS, NAL_SPS, VideoPacket, nal_type, nal_units};
use crate::Picture;

/// The clock the pictures are stamped in: milliseconds.
const TIMESCALE: i32 = 1000;
const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// What the encoder's callback leaves: the packets of the frames completed
/// (one a call, in practice), or the first error.
#[derive(Default)]
struct Encoded {
    packets: Vec<VideoPacket>,
    error: Option<String>,
}

pub(super) struct Encoder {
    session: CFRetained<VTCompressionSession>,
    /// What the callback writes into; freed after the session is gone.
    out: *mut Mutex<Encoded>,
    width: u32,
    height: u32,
    force_key: bool,
}

impl Encoder {
    pub(super) fn new(width: u32, height: u32, bitrate: u32, fps: u32) -> Result<Self, String> {
        let out = Box::into_raw(Box::new(Mutex::new(Encoded::default())));
        let bgra = CFNumber::new_i64(i64::from(kCVPixelFormatType_32BGRA));
        // SAFETY: a static key
        let source = dictionary(&[(unsafe { kCVPixelBufferPixelFormatTypeKey }, &**bgra)]);
        let mut raw = std::ptr::null_mut();
        // SAFETY: the arguments VideoToolbox documents; `out` outlives the
        // session (freed in `drop`, after `invalidate`)
        let made = unsafe {
            VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                None,
                Some(source.as_opaque()),
                None,
                Some(encoded),
                out.cast(),
                NonNull::from(&mut raw),
            )
        };
        let Some(session) = NonNull::new(raw).filter(|_| made == 0) else {
            // SAFETY: nothing else holds `out`
            drop(unsafe { Box::from_raw(out) });
            return Err(format!("VTCompressionSessionCreate: {made}"));
        };
        // SAFETY: a session `create` returned, retained once for us
        let session = unsafe { CFRetained::from_raw(session) };
        let mut encoder = Self {
            session,
            out,
            width,
            height,
            force_key: true,
        };
        // SAFETY: static keys
        unsafe {
            encoder.set(kVTCompressionPropertyKey_RealTime, CFBoolean::new(true))?;
            encoder.set(
                kVTCompressionPropertyKey_AllowFrameReordering,
                CFBoolean::new(false),
            )?;
            // Constrained Baseline is the wire's profile; an encoder
            // without it (older hardware) gives Baseline
            if encoder
                .set(
                    kVTCompressionPropertyKey_ProfileLevel,
                    kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
                )
                .is_err()
            {
                encoder.set(
                    kVTCompressionPropertyKey_ProfileLevel,
                    kVTProfileLevel_H264_Baseline_AutoLevel,
                )?;
            }
            encoder.set(
                kVTCompressionPropertyKey_ExpectedFrameRate,
                &CFNumber::new_i32(fps as i32),
            )?;
        }
        encoder.set_bitrate(bitrate)?;
        // SAFETY: a session not yet encoding
        let prepared = unsafe { encoder.session.prepare_to_encode_frames() };
        if prepared != 0 {
            return Err(format!(
                "VTCompressionSessionPrepareToEncodeFrames: {prepared}"
            ));
        }
        Ok(encoder)
    }

    /// The average the rate control keeps to. No `DataRateLimits` cap
    /// with it: a cap set once held the encoder under it after the
    /// average was raised again (proven on macOS 27: 8 kB/s at 1 Mb/s).
    pub(super) fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        let average = CFNumber::new_i64(i64::from(bitrate));
        // SAFETY: a static key
        unsafe { self.set(kVTCompressionPropertyKey_AverageBitRate, &average) }
    }

    pub(super) fn keyframe(&mut self) {
        self.force_key = true;
    }

    pub(super) fn encode(
        &mut self,
        bgra: &[u8],
        at: Duration,
    ) -> Result<Option<VideoPacket>, String> {
        let image = pixel_buffer(bgra, self.width, self.height)?;
        let options = self.force_key.then(|| {
            // SAFETY: a static key
            dictionary(&[(
                unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame },
                &**CFBoolean::new(true),
            )])
        });
        // SAFETY: a BGRA buffer of the session's size; the options are
        // VideoToolbox's own
        let at = unsafe { CMTime::new(at.as_millis() as i64, TIMESCALE) };
        let frame = unsafe {
            self.session.encode_frame(
                &image,
                at,
                kCMTimeInvalid,
                options.as_deref().map(CFDictionary::as_opaque),
                std::ptr::null_mut(),
                std::ptr::null_mut::<VTEncodeInfoFlags>(),
            )
        };
        if frame != 0 {
            return Err(format!("VTCompressionSessionEncodeFrame: {frame}"));
        }
        self.force_key = false;
        // SAFETY: the session is live; the callback has run when this returns
        let done = unsafe { self.session.complete_frames(kCMTimeInvalid) };
        if done != 0 {
            return Err(format!("VTCompressionSessionCompleteFrames: {done}"));
        }
        // SAFETY: `out` lives as long as `self`
        let mut out = lock(unsafe { &*self.out });
        if let Some(error) = out.error.take() {
            return Err(error);
        }
        // a frame the rate control dropped leaves none; two would be a
        // frame held from before, which VideoToolbox does not do with
        // reordering off: the last is the newest
        Ok(out.packets.drain(..).next_back())
    }

    /// # Safety
    /// `key` is a VideoToolbox compression property key.
    unsafe fn set(&self, key: &CFString, value: &CFType) -> Result<(), String> {
        // SAFETY: the same object, VTSession being every session's type
        let session: &VTSession =
            unsafe { &*(&*self.session as *const VTCompressionSession).cast() };
        match unsafe { VTSessionSetProperty(session, key, Some(value)) } {
            0 => Ok(()),
            code => Err(format!("VTSessionSetProperty {key}: {code}")),
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: invalidated, the callback runs no more; then `out` goes
        unsafe {
            self.session.invalidate();
            drop(Box::from_raw(self.out));
        }
    }
}

/// The compression callback: the sample buffer's NAL units as Annex B,
/// the parameter sets in front when it is a keyframe.
unsafe extern "C-unwind" fn encoded(
    out: *mut c_void,
    _frame: *mut c_void,
    status: i32,
    _flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    // SAFETY: `out` is the encoder's `Mutex<Encoded>`, live while the
    // session is
    let out = unsafe { &*out.cast::<Mutex<Encoded>>() };
    let mut out = lock(out);
    if status != 0 {
        out.error
            .get_or_insert_with(|| format!("VideoToolbox encode: {status}"));
        return;
    }
    // SAFETY: a sample buffer VideoToolbox handed over for this call
    let Some(sample) = (unsafe { sample.as_ref() }) else {
        return;
    };
    match unsafe { packet(sample) } {
        Ok(Some(packet)) => out.packets.push(packet),
        Ok(None) => {}
        Err(error) => {
            out.error.get_or_insert(error);
        }
    }
}

/// # Safety
/// `sample` is a compressed H.264 sample buffer.
unsafe fn packet(sample: &CMSampleBuffer) -> Result<Option<VideoPacket>, String> {
    // SAFETY: the sample's own attachments, read only
    let keyframe = unsafe {
        let attachments = sample.sample_attachments_array(false);
        match attachments.filter(|array| array.count() > 0) {
            None => true,
            Some(array) => {
                let first = array.value_at_index(0).cast::<CFDictionary>();
                let not_sync =
                    (*first).value((kCMSampleAttachmentKey_NotSync as *const CFString).cast());
                not_sync.is_null() || !(*not_sync.cast::<CFBoolean>()).as_bool()
            }
        }
    };
    let mut data = Vec::new();
    // SAFETY: the sample's format description and block buffer, read only
    unsafe {
        if keyframe {
            let format = sample
                .format_description()
                .ok_or("a compressed sample without a format description")?;
            for index in 0..2 {
                let mut set: *const u8 = std::ptr::null();
                let mut size = 0usize;
                let got = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    &format,
                    index,
                    &mut set,
                    &mut size,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if got != 0 || set.is_null() {
                    return Err(format!("parameter set {index}: {got}"));
                }
                data.extend(START_CODE);
                data.extend(std::slice::from_raw_parts(set, size));
            }
        }
        let Some(block) = sample.data_buffer() else {
            return Ok(None);
        };
        let total = block.data_length();
        let mut avcc = vec![0u8; total];
        let copied =
            block.copy_data_bytes(0, total, NonNull::new_unchecked(avcc.as_mut_ptr().cast()));
        if copied != 0 {
            return Err(format!("CMBlockBufferCopyDataBytes: {copied}"));
        }
        // AVCC: each unit after its 4-byte big-endian length
        let mut rest = &avcc[..];
        while rest.len() >= 4 {
            let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
            rest = &rest[4..];
            let unit = rest.get(..len).ok_or("an AVCC length past the sample")?;
            data.extend(START_CODE);
            data.extend(unit);
            rest = &rest[len..];
        }
    }
    Ok(Some(VideoPacket { data, keyframe }))
}

/// A BGRA pixel buffer of `bgra` at `width` by `height`.
fn pixel_buffer(bgra: &[u8], width: u32, height: u32) -> Result<CFRetained<CVPixelBuffer>, String> {
    let (w, h) = (width as usize, height as usize);
    let mut raw = std::ptr::null_mut();
    // SAFETY: a size and a format CoreVideo takes; `raw` outlives the call
    let made = unsafe {
        CVPixelBufferCreate(
            None,
            w,
            h,
            kCVPixelFormatType_32BGRA,
            None,
            NonNull::from(&mut raw),
        )
    };
    let Some(image) = NonNull::new(raw).filter(|_| made == 0) else {
        return Err(format!("CVPixelBufferCreate: {made}"));
    };
    // SAFETY: a buffer `create` returned, retained once for us
    let image = unsafe { CFRetained::from_raw(image) };
    // SAFETY: locked around the write; rows `stride` bytes apart, each
    // of `w * 4` bytes of ours
    unsafe {
        if CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::empty()) != 0 {
            return Err("a pixel buffer that does not lock".into());
        }
        let stride = CVPixelBufferGetBytesPerRow(&image);
        let base = CVPixelBufferGetBaseAddress(&image) as *mut u8;
        for (y, row) in bgra.chunks_exact(w * 4).take(h).enumerate() {
            std::ptr::copy_nonoverlapping(row.as_ptr(), base.add(y * stride), w * 4);
        }
        CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::empty());
    }
    Ok(image)
}

/// A dictionary of `pairs`, keyed by CoreFoundation strings.
fn dictionary(pairs: &[(&CFString, &CFType)]) -> CFRetained<CFDictionary<CFString, CFType>> {
    let keys: Vec<&CFString> = pairs.iter().map(|(key, _)| *key).collect();
    let values: Vec<&CFType> = pairs.iter().map(|(_, value)| *value).collect();
    CFDictionary::from_slices(&keys, &values)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the decoder's callback leaves: the picture of the frame decoded,
/// or the first error.
#[derive(Default)]
struct Decoded {
    picture: Option<Picture>,
    error: Option<String>,
}

/// A session at one stream's parameter sets.
struct Stream {
    sps: Vec<u8>,
    pps: Vec<u8>,
    format: CFRetained<CMFormatDescription>,
    session: CFRetained<VTDecompressionSession>,
}

pub(super) struct Decoder {
    /// None before the first keyframe.
    stream: Option<Stream>,
    out: *mut Mutex<Decoded>,
}

impl Decoder {
    pub(super) fn new() -> Result<Self, String> {
        Ok(Self {
            stream: None,
            out: Box::into_raw(Box::new(Mutex::new(Decoded::default()))),
        })
    }

    pub(super) fn decode(&mut self, packet: &[u8]) -> Result<Option<Picture>, String> {
        let (mut sps, mut pps) = (None, None);
        let mut avcc = Vec::with_capacity(packet.len() + 16);
        for unit in nal_units(packet) {
            match nal_type(unit) {
                NAL_SPS => sps = Some(unit),
                NAL_PPS => pps = Some(unit),
                // slices: 1 to 5; the rest (SEI, delimiters) carry no picture
                1..=5 => {
                    avcc.extend((unit.len() as u32).to_be_bytes());
                    avcc.extend(unit);
                }
                _ => {}
            }
        }
        if let (Some(sps), Some(pps)) = (sps, pps)
            && self
                .stream
                .as_ref()
                .is_none_or(|had| (&had.sps[..], &had.pps[..]) != (sps, pps))
        {
            self.stream = None;
            let format = format(sps, pps)?;
            let session = self.open(&format)?;
            self.stream = Some(Stream {
                sps: sps.to_vec(),
                pps: pps.to_vec(),
                format,
                session,
            });
        }
        let Some(Stream {
            format, session, ..
        }) = &self.stream
        else {
            // before the first keyframe: nothing to decode with
            return Ok(None);
        };
        if avcc.is_empty() {
            return Ok(None);
        }
        let sample = sample(&avcc, format)?;
        // SAFETY: a sample of the session's format; no asynchronous flag,
        // so the callback has run when this returns
        let decoded = unsafe {
            session.decode_frame(
                &sample,
                VTDecodeFrameFlags::empty(),
                std::ptr::null_mut(),
                std::ptr::null_mut::<VTDecodeInfoFlags>(),
            )
        };
        // SAFETY: `out` lives as long as `self`
        let mut out = lock(unsafe { &*self.out });
        if let Some(error) = out.error.take() {
            return Err(error);
        }
        if decoded != 0 {
            // a slice the decoder refused (a peer's bytes): no picture,
            // the next keyframe starts afresh
            out.picture = None;
            return Err(format!("VTDecompressionSessionDecodeFrame: {decoded}"));
        }
        Ok(out.picture.take())
    }

    fn open(
        &self,
        format: &CMFormatDescription,
    ) -> Result<CFRetained<VTDecompressionSession>, String> {
        let bgra = CFNumber::new_i64(i64::from(kCVPixelFormatType_32BGRA));
        // SAFETY: a static key
        let destination = dictionary(&[(unsafe { kCVPixelBufferPixelFormatTypeKey }, &**bgra)]);
        let record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decoded),
            decompressionOutputRefCon: self.out.cast(),
        };
        let mut raw = std::ptr::null_mut();
        // SAFETY: the arguments VideoToolbox documents; `out` outlives the
        // session (freed in `drop`, after `invalidate`)
        let made = unsafe {
            VTDecompressionSession::create(
                None,
                format,
                None,
                Some(destination.as_opaque()),
                &record,
                NonNull::from(&mut raw),
            )
        };
        let session = NonNull::new(raw)
            .filter(|_| made == 0)
            .ok_or_else(|| format!("VTDecompressionSessionCreate: {made}"))?;
        // SAFETY: a session `create` returned, retained once for us
        Ok(unsafe { CFRetained::from_raw(session) })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            // SAFETY: invalidated, the callback runs no more
            unsafe { stream.session.invalidate() };
        }
        // SAFETY: no session holds `out` any more
        drop(unsafe { Box::from_raw(self.out) });
    }
}

/// The decompression callback: the frame's BGRA picture.
unsafe extern "C-unwind" fn decoded(
    out: *mut c_void,
    _frame: *mut c_void,
    status: i32,
    _flags: VTDecodeInfoFlags,
    image: *mut objc2_core_video::CVImageBuffer,
    _at: CMTime,
    _duration: CMTime,
) {
    // SAFETY: `out` is the decoder's `Mutex<Decoded>`, live while any
    // session is
    let out = unsafe { &*out.cast::<Mutex<Decoded>>() };
    let mut out = lock(out);
    if status != 0 {
        out.error
            .get_or_insert_with(|| format!("VideoToolbox decode: {status}"));
        return;
    }
    // SAFETY: a pixel buffer VideoToolbox handed over for this call
    if let Some(image) = unsafe { image.as_ref() } {
        out.picture = crate::apple::picture(image, false);
    }
}

/// A format description of the parameter sets, with 4-byte lengths.
fn format(sps: &[u8], pps: &[u8]) -> Result<CFRetained<CMFormatDescription>, String> {
    let mut pointers = [
        NonNull::from(sps).cast::<u8>(),
        NonNull::from(pps).cast::<u8>(),
    ];
    let mut sizes = [sps.len(), pps.len()];
    let mut raw: *const CMFormatDescription = std::ptr::null();
    // SAFETY: two parameter sets, their sizes, live for the call
    let made = unsafe {
        CMVideoFormatDescriptionCreateFromH264ParameterSets(
            None,
            2,
            NonNull::from(&mut pointers).cast(),
            NonNull::from(&mut sizes).cast(),
            4,
            NonNull::from(&mut raw),
        )
    };
    let format = NonNull::new(raw.cast_mut())
        .filter(|_| made == 0)
        .ok_or_else(|| format!("CMVideoFormatDescriptionCreateFromH264ParameterSets: {made}"))?;
    // SAFETY: a description `create` returned, retained once for us
    Ok(unsafe { CFRetained::from_raw(format) })
}

/// One sample of `avcc` in `format`.
fn sample(avcc: &[u8], format: &CMFormatDescription) -> Result<CFRetained<CMSampleBuffer>, String> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: a block CoreMedia allocates (no memory block given, the
    // default allocator), then filled with our bytes
    let block = unsafe {
        let made = CMBlockBuffer::create_with_memory_block(
            None,
            std::ptr::null_mut(),
            avcc.len(),
            None,
            std::ptr::null(),
            0,
            avcc.len(),
            kCMBlockBufferAssureMemoryNowFlag,
            NonNull::from(&mut raw),
        );
        let block = NonNull::new(raw)
            .filter(|_| made == 0)
            .ok_or_else(|| format!("CMBlockBufferCreateWithMemoryBlock: {made}"))?;
        let block = CFRetained::from_raw(block);
        let filled =
            CMBlockBuffer::replace_data_bytes(NonNull::from(avcc).cast(), &block, 0, avcc.len());
        if filled != 0 {
            return Err(format!("CMBlockBufferReplaceDataBytes: {filled}"));
        }
        block
    };
    let size = avcc.len();
    let mut raw = std::ptr::null_mut();
    // SAFETY: one sample of `size` bytes, no timing (none needed to decode
    // with reordering off)
    let made = unsafe {
        CMSampleBuffer::create_ready(
            None,
            Some(&block),
            Some(format),
            1,
            0,
            std::ptr::null(),
            1,
            &size,
            NonNull::from(&mut raw),
        )
    };
    let sample = NonNull::new(raw)
        .filter(|_| made == 0)
        .ok_or_else(|| format!("CMSampleBufferCreateReady: {made}"))?;
    // SAFETY: a sample `create` returned, retained once for us
    Ok(unsafe { CFRetained::from_raw(sample) })
}
