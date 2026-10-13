//! THE SEAM: every call into a capture or playback library, and nothing of
//! gpui. Cameras through nokhwa, microphones and speakers through cpal,
//! each piece on a thread of its own that leaves what it has in a feed
//! ([`CameraFeed`], [`MicFeed`]) for the gpui side to read. Another backend
//! replaces this file: the feeds, [`Device`], [`Status`] and the five
//! functions are the whole surface.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};

/// One camera, microphone or speaker: `id` opens it, `name` is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Where a piece's device is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Starting,
    Live,
    Denied,
    Busy,
    Gone,
    NoDevice,
    Failed(String),
}

/// A microphone piece's recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    #[default]
    Idle,
    Recording,
    Playing,
}

/// One camera picture: BGRA rows, top to bottom, mirrored if asked.
/// ponytail: a copy per frame; a platform buffer (macOS `CVPixelBuffer`
/// into gpui's `surface`) is the zero-copy upgrade.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What a camera's thread leaves for its piece.
#[derive(Default)]
pub struct CameraFeed {
    /// The status and the device's name.
    pub status: Mutex<(Status, String)>,
    /// The newest picture, not yet shown: the latest wins.
    pub picture: Mutex<Option<Picture>>,
    pub mirror: AtomicBool,
    /// Set when the piece is gone: the thread releases the camera.
    pub stop: AtomicBool,
}

/// What a microphone's thread leaves for its piece, and what it asks.
#[derive(Default)]
pub struct MicFeed {
    pub status: Mutex<(Status, String)>,
    /// The level, 0 to 1 (-60 to 0 dBFS), as `f32` bits.
    pub level: AtomicU32,
    pub playback: Mutex<Playback>,
    pub monitor: AtomicBool,
    /// `Some(seconds)`: record, then play back; `Some(0)`: stop.
    pub ask: Mutex<Option<u8>>,
    pub stop: AtomicBool,
}

impl MicFeed {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

fn set(status: &Mutex<(Status, String)>, to: Status) {
    status.lock().expect("feed status").0 = to;
}

// ---------- cameras ----------

/// The cameras the system lists now; the first is the default.
pub fn cameras() -> Result<Vec<Device>, String> {
    // listing asks for no permission (AVFoundation's discovery lists
    // without it); opening does, in `run_camera`
    let found = nokhwa::query(ApiBackend::Auto).map_err(|error| error.to_string())?;
    Ok(found
        .iter()
        .enumerate()
        .map(|(at, info)| Device {
            id: info.index().to_string(),
            name: info.human_name(),
            default: at == 0,
        })
        .collect())
}

/// Opens `device` (the first camera when `None`) on a thread of its own
/// and feeds its pictures until `feed.stop`.
pub fn start_camera(device: Option<String>, feed: Arc<CameraFeed>) {
    let spawned = std::thread::Builder::new()
        .name("av-camera".into())
        .spawn(move || run_camera(device, &feed));
    if let Err(error) = spawned {
        tracing_error("camera thread", &error);
    }
}

fn run_camera(device: Option<String>, feed: &CameraFeed) {
    #[cfg(target_os = "macos")]
    if !crate::mac::allowed(crate::mac::VIDEO) {
        return set(&feed.status, Status::Denied);
    }
    let listed = cameras().unwrap_or_default();
    let index = match &device {
        Some(id) => listed.iter().find(|d| &d.id == id).map(|d| d.id.clone()),
        None => listed.first().map(|d| d.id.clone()),
    };
    let Some(index) = index else {
        return set(&feed.status, Status::NoDevice);
    };
    let name = listed
        .iter()
        .find(|d| d.id == index)
        .map(|d| d.name.clone())
        .unwrap_or_default();
    feed.status.lock().expect("feed status").1 = name;
    let index = match index.parse::<u32>() {
        Ok(at) => CameraIndex::Index(at),
        Err(_) => CameraIndex::String(index),
    };
    // YUYV first on every OS (no decoder, and nokhwa's macOS path asks
    // AVFoundation for packed 4:2:2 then), MJPEG where a camera has no YUYV
    let mut camera = None;
    let mut last = String::new();
    for format in [FrameFormat::YUYV, FrameFormat::MJPEG] {
        let wanted = RequestedFormat::new::<nokhwa::pixel_format::RgbFormat>(
            RequestedFormatType::Closest(CameraFormat::new(Resolution::new(1280, 720), format, 30)),
        );
        match nokhwa::Camera::new(index.clone(), wanted) {
            Ok(opened) => {
                camera = Some(opened);
                break;
            }
            Err(error) => last = error.to_string(),
        }
    }
    let Some(mut camera) = camera else {
        return set(&feed.status, fault(&last, false));
    };
    if let Err(error) = camera.open_stream() {
        return set(&feed.status, fault(&error.to_string(), false));
    }
    let mut live = false;
    while !feed.stop.load(Ordering::Relaxed) {
        let buffer = match camera.frame() {
            Ok(buffer) => buffer,
            Err(error) => {
                set(&feed.status, fault(&error.to_string(), true));
                break;
            }
        };
        let resolution = buffer.resolution();
        let mirror = feed.mirror.load(Ordering::Relaxed);
        let picture = match buffer.source_frame_format() {
            FrameFormat::YUYV => yuyv_to_bgra(
                buffer.buffer(),
                resolution.width(),
                resolution.height(),
                mirror,
            ),
            FrameFormat::MJPEG => jpeg_to_bgra(buffer.buffer(), mirror),
            other => {
                set(
                    &feed.status,
                    Status::Failed(format!("the camera sends {other}")),
                );
                break;
            }
        };
        let Some(picture) = picture else { continue };
        *feed.picture.lock().expect("feed picture") = Some(picture);
        if !live {
            live = true;
            set(&feed.status, Status::Live);
        }
    }
    // the light off now, not when the last reference goes
    let _ = camera.stop_stream();
}

/// What a failure's words say, as a status: nokhwa reports every failure
/// as text. `open`: the device was open (a vanished one is gone).
/// ponytail: reads the OS's error text; nokhwa 0.11's typed errors are the
/// upgrade.
fn fault(error: &str, open: bool) -> Status {
    let lower = error.to_lowercase();
    if lower.contains("busy") {
        Status::Busy
    } else if lower.contains("permission") || lower.contains("denied") {
        Status::Denied
    } else if lower.contains("no such") || lower.contains("not found") {
        if open { Status::Gone } else { Status::NoDevice }
    } else if open {
        Status::Gone
    } else {
        Status::Failed(
            error
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect(),
        )
    }
}

/// Packed 4:2:2 (Y0 U Y1 V, BT.601 video range) to BGRA. A buffer whose
/// rows are padded is read by its stride.
pub(crate) fn yuyv_to_bgra(src: &[u8], width: u32, height: u32, mirror: bool) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    if w < 2 || h == 0 || src.len() < w * 2 * h {
        return None;
    }
    let stride = src.len() / h;
    let mut bgra = vec![0u8; w * h * 4];
    for (y, out) in bgra.chunks_exact_mut(w * 4).enumerate() {
        let row = &src[y * stride..y * stride + w * 2];
        for x in 0..w {
            let pair = &row[(x & !1) * 2..(x & !1) * 2 + 4];
            let luma = pair[if x & 1 == 0 { 0 } else { 2 }];
            let (b, g, r) = yuv(luma, pair[1], pair[3]);
            let at = if mirror { w - 1 - x } else { x } * 4;
            out[at..at + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    Some(Picture {
        width,
        height,
        bgra,
    })
}

fn yuv(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = (i32::from(y) - 16) * 298;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let clamp = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u8;
    (
        clamp(c + 516 * d),
        clamp(c - 100 * d - 208 * e),
        clamp(c + 409 * e),
    )
}

fn jpeg_to_bgra(src: &[u8], mirror: bool) -> Option<Picture> {
    let decoded = image::load_from_memory_with_format(src, image::ImageFormat::Jpeg).ok()?;
    let mut rgba = decoded.to_rgba8();
    if mirror {
        image::imageops::flip_horizontal_in_place(&mut rgba);
    }
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Picture {
        width,
        height,
        bgra,
    })
}

// ---------- microphones and speakers ----------

/// The microphones and the speakers the system lists now.
pub fn audio_devices() -> Result<(Vec<Device>, Vec<Device>), String> {
    let host = cpal::default_host();
    let id = |device: &cpal::Device| device.id().map(|id| id.to_string()).ok();
    let list = |devices: Vec<cpal::Device>, default: Option<String>| {
        devices
            .iter()
            .filter_map(|device| {
                let id = id(device)?;
                Some(Device {
                    name: device
                        .description()
                        .map(|d| d.name().to_owned())
                        .unwrap_or_else(|_| id.clone()),
                    default: default.as_ref() == Some(&id),
                    id,
                })
            })
            .collect::<Vec<_>>()
    };
    let inputs = host
        .input_devices()
        .map_err(|error| error.to_string())?
        .collect();
    let outputs = host
        .output_devices()
        .map_err(|error| error.to_string())?
        .collect();
    Ok((
        list(inputs, host.default_input_device().as_ref().and_then(id)),
        list(outputs, host.default_output_device().as_ref().and_then(id)),
    ))
}

/// Opens `device` (the default microphone when `None`) on a thread of its
/// own: its level into `feed`, a recording asked through `feed.ask`
/// played back on `output`, and the monitor while `feed.monitor`, until
/// `feed.stop`.
pub fn start_microphone(device: Option<String>, output: Option<String>, feed: Arc<MicFeed>) {
    let spawned = std::thread::Builder::new()
        .name("av-microphone".into())
        .spawn(move || run_microphone(device, output, &feed));
    if let Err(error) = spawned {
        tracing_error("microphone thread", &error);
    }
}

/// The recording being made, mono: the samples so far and how many are
/// wanted.
type Recording = Arc<Mutex<Option<(Vec<f32>, usize)>>>;
/// Mono samples on their way to a speaker.
type Queue = Arc<Mutex<VecDeque<f32>>>;

fn find(host: &cpal::Host, id: Option<&str>, input: bool) -> Option<cpal::Device> {
    match id {
        Some(id) => host.device_by_id(&id.parse().ok()?),
        None if input => host.default_input_device(),
        None => host.default_output_device(),
    }
}

fn audio_fault(error: &cpal::Error, open: bool) -> Status {
    match error.kind() {
        cpal::ErrorKind::DeviceBusy => Status::Busy,
        cpal::ErrorKind::PermissionDenied => Status::Denied,
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated if open => {
            Status::Gone
        }
        cpal::ErrorKind::DeviceNotAvailable => Status::NoDevice,
        _ => Status::Failed(error.to_string().chars().take(200).collect()),
    }
}

fn run_microphone(device: Option<String>, output: Option<String>, feed: &Arc<MicFeed>) {
    #[cfg(target_os = "macos")]
    if !crate::mac::allowed(crate::mac::AUDIO) {
        return set(&feed.status, Status::Denied);
    }
    let host = cpal::default_host();
    let Some(input) = find(&host, device.as_deref(), true) else {
        return set(&feed.status, Status::NoDevice);
    };
    let name = input
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_default();
    feed.status.lock().expect("feed status").1 = name;
    let config = match input.default_input_config() {
        Ok(config) => config,
        Err(error) => return set(&feed.status, audio_fault(&error, false)),
    };
    let rate = config.sample_rate();
    let recording = Recording::default();
    // what the microphone hears, for the monitor's speaker while it runs
    let heard = Queue::default();
    let stream = match capture(
        &input,
        &config,
        feed.clone(),
        recording.clone(),
        heard.clone(),
    ) {
        Ok(stream) => stream,
        Err(error) => return set(&feed.status, audio_fault(&error, false)),
    };
    if let Err(error) = stream.play() {
        return set(&feed.status, audio_fault(&error, false));
    }
    set(&feed.status, Status::Live);
    // the speaker's streams: a recording's playback, the monitor
    let mut playing: Option<(cpal::Stream, Queue)> = None;
    let mut monitoring: Option<cpal::Stream> = None;
    let playback = |to: Playback| *feed.playback.lock().expect("feed playback") = to;
    while !feed.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        if let Some(seconds) = feed.ask.lock().expect("feed ask").take() {
            playing = None;
            *recording.lock().expect("recording") = match seconds {
                0 => None,
                seconds => Some((Vec::new(), rate as usize * usize::from(seconds))),
            };
            playback(match seconds {
                0 => Playback::Idle,
                _ => Playback::Recording,
            });
        }
        let recorded = {
            let mut recording = recording.lock().expect("recording");
            match &*recording {
                Some((samples, wanted)) if samples.len() >= *wanted => {
                    recording.take().map(|(samples, _)| samples)
                }
                _ => None,
            }
        };
        if let Some(samples) = recorded {
            let queue = Queue::new(Mutex::new(VecDeque::from(samples)));
            match speaker(&host, output.as_deref(), rate, queue.clone()) {
                Ok(stream) => {
                    playing = Some((stream, queue));
                    playback(Playback::Playing);
                }
                Err(status) => {
                    set(&feed.status, status);
                    playback(Playback::Idle);
                }
            }
        }
        if playing
            .as_ref()
            .is_some_and(|(_, queue)| queue.lock().expect("playback").is_empty())
        {
            // ponytail: the last callback's buffer may still be sounding;
            // a short tail is cut rather than waited on
            std::thread::sleep(Duration::from_millis(100));
            playing = None;
            playback(Playback::Idle);
        }
        let monitor = feed.monitor.load(Ordering::Relaxed);
        if monitor && monitoring.is_none() {
            heard.lock().expect("heard").clear();
            match speaker(&host, output.as_deref(), rate, heard.clone()) {
                Ok(stream) => monitoring = Some(stream),
                Err(status) => {
                    set(&feed.status, status);
                    feed.monitor.store(false, Ordering::Relaxed);
                }
            }
        } else if !monitor {
            monitoring = None;
        }
    }
    drop((stream, playing, monitoring));
    feed.level.store(0f32.to_bits(), Ordering::Relaxed);
}

/// `config` with a 20 ms buffer where the device takes one: a live loop
/// through the defaults (ALSA's run to a second) queues and drops.
fn short(config: &cpal::SupportedStreamConfig) -> cpal::StreamConfig {
    let mut short = config.config();
    if let cpal::SupportedBufferSize::Range { min, max } = config.buffer_size() {
        let frames = config.sample_rate() / 50;
        short.buffer_size = cpal::BufferSize::Fixed(frames.clamp(*min, *max));
    }
    short
}

/// The microphone's stream: its level into `feed`, its samples (mono)
/// into the recording being made and, while the monitor runs, `heard`.
fn capture(
    input: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    feed: Arc<MicFeed>,
    recording: Recording,
    heard: Queue,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = usize::from(config.channels()).max(1);
    let rate = config.sample_rate() as usize;
    let errors = feed.clone();
    // ponytail: a Vec per callback on the audio thread; a preallocated
    // buffer if a glitch is ever heard
    let on_samples = move |mono: &mut dyn Iterator<Item = f32>| {
        let mono: Vec<f32> = mono.collect();
        let power = mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32;
        let db = 10.0 * power.max(1e-10).log10();
        let level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        feed.level.store(level.to_bits(), Ordering::Relaxed);
        if let Some((samples, wanted)) = &mut *recording.lock().expect("recording") {
            let room = wanted.saturating_sub(samples.len());
            samples.extend(mono.iter().take(room));
        }
        if feed.monitor.load(Ordering::Relaxed) {
            // ponytail: at most a fifth of a second behind; drift between
            // the two clocks is dropped here, not resampled
            let mut heard = heard.lock().expect("heard");
            heard.extend(&mono);
            let over = heard.len().saturating_sub(rate / 5);
            heard.drain(..over);
        }
    };
    let on_error = move |error: cpal::Error| {
        if matches!(
            error.kind(),
            cpal::ErrorKind::DeviceNotAvailable
                | cpal::ErrorKind::StreamInvalidated
                | cpal::ErrorKind::BackendError
        ) {
            set(&errors.status, audio_fault(&error, true));
        }
    };
    macro_rules! typed {
        ($t:ty) => {
            input.build_input_stream::<$t, _, _>(
                short(config),
                move |data: &[$t], _| {
                    on_samples(&mut data.chunks(channels).map(|frame| {
                        frame
                            .iter()
                            .map(|s| <f32 as cpal::FromSample<$t>>::from_sample_(*s))
                            .sum::<f32>()
                            / frame.len() as f32
                    }))
                },
                on_error,
                None,
            )
        };
    }
    match config.sample_format() {
        cpal::SampleFormat::F32 => typed!(f32),
        cpal::SampleFormat::I16 => typed!(i16),
        cpal::SampleFormat::I32 => typed!(i32),
        cpal::SampleFormat::U16 => typed!(u16),
        other => Err(cpal::Error::with_message(
            cpal::ErrorKind::UnsupportedConfig,
            format!("the microphone sends {other}"),
        )),
    }
}

/// A speaker stream playing `queue` (mono at `rate`) on every channel,
/// silent once it is empty.
fn speaker(
    host: &cpal::Host,
    output: Option<&str>,
    rate: u32,
    queue: Queue,
) -> Result<cpal::Stream, Status> {
    let speaker = find(host, output, false).ok_or(Status::Failed("no speaker".into()))?;
    let config = speaker
        .default_output_config()
        .map_err(|error| audio_fault(&error, false))?;
    let channels = usize::from(config.channels()).max(1);
    // ponytail: nearest-sample rate conversion; a resampler when the two
    // rates differ and it is heard
    let step = f64::from(rate) / f64::from(config.sample_rate());
    let mut due = 0.0f64;
    let mut sample = 0.0f32;
    // a jitter buffer: silence until 60 ms wait, and again after the queue
    // ran dry, so a live loop fed in bursts plays in runs, not in crumbs
    let primed_at = rate as usize * 6 / 100;
    let mut primed = false;
    let mut next = move || {
        let mut queue = queue.lock().expect("speaker queue");
        primed = primed && !queue.is_empty() || queue.len() >= primed_at;
        if !primed {
            return 0.0;
        }
        due += step;
        while due >= 1.0 {
            due -= 1.0;
            sample = queue.pop_front().unwrap_or(0.0);
        }
        sample
    };
    macro_rules! typed {
        ($t:ty) => {
            speaker.build_output_stream::<$t, _, _>(
                short(&config),
                move |data: &mut [$t], _| {
                    for frame in data.chunks_mut(channels) {
                        let value = <$t as cpal::FromSample<f32>>::from_sample_(next());
                        frame.fill(value);
                    }
                },
                |_| {},
                None,
            )
        };
    }
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => typed!(f32),
        cpal::SampleFormat::I16 => typed!(i16),
        cpal::SampleFormat::I32 => typed!(i32),
        cpal::SampleFormat::U16 => typed!(u16),
        other => {
            return Err(Status::Failed(format!("the speaker takes {other}")));
        }
    }
    .map_err(|error| audio_fault(&error, false))?;
    stream.play().map_err(|error| audio_fault(&error, false))?;
    Ok(stream)
}

fn tracing_error(what: &str, error: &dyn std::fmt::Display) {
    eprintln!("ducktape-av: {what} did not start: {error}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grey, white and a mirrored pair come out of 4:2:2 as BGRA.
    #[test]
    fn yuyv_turns_into_bgra_and_mirrors() {
        // two pixels: black (Y 16) then white (Y 235), no colour
        let src = [16, 128, 235, 128];
        let plain = yuyv_to_bgra(&src, 2, 1, false).unwrap();
        assert_eq!(plain.bgra, [0, 0, 0, 255, 255, 255, 255, 255]);
        let mirrored = yuyv_to_bgra(&src, 2, 1, true).unwrap();
        assert_eq!(mirrored.bgra, [255, 255, 255, 255, 0, 0, 0, 255]);
        // pure red in BT.601 video range: Y 81, U 90, V 240
        let red = yuyv_to_bgra(&[81, 90, 81, 240], 2, 1, false).unwrap();
        let [b, g, r, _] = red.bgra[..4] else {
            unreachable!()
        };
        assert!(r > 250 && g < 5 && b < 5, "{:?}", &red.bgra[..4]);
        // a short buffer is no picture; a padded row is read by its stride
        assert!(yuyv_to_bgra(&src[..3], 2, 1, false).is_none());
        let padded = [16, 128, 235, 128, 0, 0, 16, 128, 235, 128, 0, 0];
        assert_eq!(yuyv_to_bgra(&padded, 2, 2, false).unwrap().bgra.len(), 16);
    }

    #[test]
    fn a_failure_reads_as_a_status() {
        assert_eq!(
            fault("Could not open device: Device or resource busy", false),
            Status::Busy
        );
        assert_eq!(
            fault("open: Permission denied (os error 13)", false),
            Status::Denied
        );
        assert_eq!(fault("No such device (os error 19)", true), Status::Gone);
        assert_eq!(fault("No such file or directory", false), Status::NoDevice);
    }
}
