//! THE SEAM: every call into a capture or playback library, and nothing of
//! gpui. Cameras through our own capture (V4L2 on Linux in `v4l2.rs`,
//! AVFoundation on macOS in `apple.rs`) driven by one stream loop here,
//! microphones and speakers through cpal, each piece on a thread of its
//! own that leaves what it has in a feed ([`CameraFeed`], [`MicFeed`]) for
//! the gpui side to read. Another backend replaces this file and those
//! two: the feeds, [`Device`], [`Status`] and the five functions are the
//! whole surface.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};

use crate::platform;

/// One camera, microphone or speaker: `id` opens it, `name` is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Which way a camera looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// at the person: a laptop's own camera, a phone's front one
    Front,
    /// away from the person: a phone's back camera
    Back,
    /// plugged in (a USB camera): which way it looks is the person's
    External,
}

/// One camera as listed: the device and which way it looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    pub device: Device,
    pub facing: Facing,
    /// The system's own name for it, which opens it (`platform::open`):
    /// the V4L2 node (`video0`) on Linux, the unique id on macOS. Only a
    /// listing fills it.
    pub(crate) handle: String,
}

/// What a microphone or speaker is, as the system says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioKind {
    /// a loudspeaker, which the microphone hears
    Speaker,
    /// a phone's earpiece
    Earpiece,
    /// headphones or a headset; an input: the headset's microphone
    Headphones,
    /// anything else, or what the system says nothing of
    Other,
}

/// One microphone or speaker as listed: the device and what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub device: Device,
    pub kind: AudioKind,
}

/// Where a piece's device is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Starting,
    Live,
    Denied,
    Busy,
    /// It went away while open: why, in a line, where the system says
    /// (a call took it), else empty.
    Gone(String),
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
    /// Whether the piece is being drawn: its pictures are wanted. Off, the
    /// thread still drains the camera but converts nothing (a hidden or
    /// minimised window costs no picture work).
    pub wanted: AtomicBool,
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
    /// The speaker a recording and the monitor play on (an id this crate
    /// listed; `None`: the default), read each time one starts: another
    /// moves them without the microphone closing.
    pub output: Mutex<Option<String>>,
    /// What the piece asked last, not yet done.
    pub ask: Mutex<Option<Ask>>,
    pub stop: AtomicBool,
}

/// What a microphone piece asks of its thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Record this many seconds, then play them back.
    Record(u8),
    /// End a recording or a playback.
    Stop,
}

impl MicFeed {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

fn set(status: &Mutex<(Status, String)>, to: Status) {
    status.lock().expect("feed status").0 = to;
}

// ---------- one holder of a device at a time ----------

/// A piece's feed, as its thread's hold on a device reads it: whether the
/// piece let the device go, and the piece letting it go.
pub(crate) trait Held: Send + Sync {
    fn let_go(&self) -> bool;
    fn stop(&self);
}

impl Held for CameraFeed {
    fn let_go(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Held for MicFeed {
    fn let_go(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A piece's device, let go the moment this drops: its thread closes the
/// device, and a piece made in its place opens only after ([`Holders`]).
/// A piece's own drop comes later: gpui drops a released entity once its
/// effects flush, by when the next piece may have started opening, and
/// found nothing let go to wait for. Whoever drops a piece holds this.
pub struct Release(pub(crate) Arc<dyn Held>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// The threads of one kind of piece that hold a device or are opening
/// one, by their feeds. A thread opens its device only once every thread
/// whose piece let go has closed its own: a piece made again (Try again,
/// its view's layout moving it to another place or window, another device
/// picked) never finds this process still holding the device, which an
/// exclusive one (ALSA's `hw`, a UVC camera) refuses as busy. A piece's
/// drop only asks its thread to stop (the window thread never waits on a
/// device), so the order is kept here, where the next one opens.
struct Holders {
    held: Mutex<Vec<Arc<dyn Held>>>,
    closed: Condvar,
}

static CAMERAS: Holders = Holders::new();
static MICROPHONES: Holders = Holders::new();

/// The longest a thread waits for the ones let go to close: past it, it
/// opens anyway, and the device says what it says.
const LET_GO: Duration = Duration::from_secs(2);

impl Holders {
    const fn new() -> Self {
        Self {
            held: Mutex::new(Vec::new()),
            closed: Condvar::new(),
        }
    }

    /// Once every holder whose piece let go has closed its device (at
    /// most [`LET_GO`]), `feed` holds one until the guard drops: after
    /// its own device has.
    fn hold(&'static self, feed: Arc<dyn Held>) -> Holding {
        let deadline = Instant::now() + LET_GO;
        let mut held = self.held.lock().expect("holders");
        while held.iter().any(|holder| holder.let_go()) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            held = self.closed.wait_timeout(held, left).expect("holders").0;
        }
        held.push(feed.clone());
        Holding {
            holders: self,
            feed,
        }
    }
}

/// A thread's hold on its device: dropped once the device is closed, it
/// lets the next opener go.
struct Holding {
    holders: &'static Holders,
    feed: Arc<dyn Held>,
}

impl Drop for Holding {
    fn drop(&mut self) {
        let mut held = self.holders.held.lock().expect("holders");
        held.retain(|holder| !Arc::ptr_eq(holder, &self.feed));
        self.holders.closed.notify_all();
    }
}

// ---------- cameras ----------

/// The longest device id this crate lists or opens, in bytes. A device
/// whose id is longer is not listed, so a longer id, which a view's frame
/// can carry, names no device.
const MAX_ID_BYTES: usize = 256;

/// Whether a piece can name `device` back: its id at most
/// [`MAX_ID_BYTES`].
fn nameable(device: &Device) -> bool {
    device.id.len() <= MAX_ID_BYTES
}

/// The cameras the system lists now; the first is the default.
pub fn cameras() -> Result<Vec<CameraDevice>, String> {
    // listing asks for no permission (AVFoundation lists without it);
    // opening does
    let found = platform::cameras()?;
    Ok(found
        .into_iter()
        .filter(|camera| nameable(&camera.device))
        .collect())
}

/// Opens `device` (the first camera when `None`) on a thread of its own
/// and feeds its pictures until `feed.stop`.
pub fn start_camera(device: Option<String>, feed: Arc<CameraFeed>) {
    let spawned = std::thread::Builder::new()
        .name("av-camera".into())
        .spawn(move || run_camera(device, feed));
    if let Err(error) = spawned {
        tracing_error("camera thread", &error);
    }
}

fn run_camera(device: Option<String>, feed: Arc<CameraFeed>) {
    // dropped last, after the camera
    let _held = CAMERAS.hold(feed.clone());
    // its piece went before this opened anything: nothing to open
    if feed.let_go() {
        return;
    }
    let feed = &*feed;
    // listed once: the one listed is the one opened, by the system's own
    // handle, never by the view's string
    let mut listed = cameras().unwrap_or_default().into_iter();
    let found = match &device {
        Some(id) => listed.find(|camera| &camera.device.id == id),
        None => listed.next(),
    };
    let Some(found) = found else {
        return set(&feed.status, Status::NoDevice);
    };
    if !platform::camera_allowed() {
        return set(&feed.status, Status::Denied);
    }
    // a permission prompt can take the person a while: a piece gone
    // meanwhile opens nothing (no light comes on)
    if feed.let_go() {
        return;
    }
    feed.status.lock().expect("feed status").1 = found.device.name.clone();
    match platform::open(&found.handle) {
        Ok(camera) => stream(camera, feed),
        Err(status) => set(&feed.status, status),
    }
}

/// One opened camera as the stream loop drives it: the platform's, or a
/// test's. Dropping it releases the device (the camera's light off).
pub(crate) trait Source {
    /// Waits at most `wait` for the camera's next frame. While `wanted`
    /// a frame comes back as a picture (mirrored if `mirror`); otherwise
    /// it is only taken from the camera, so it keeps delivering.
    fn next(&mut self, wait: Duration, wanted: bool, mirror: bool) -> Result<Got, Status>;
}

/// What a camera gave in one wait.
pub(crate) enum Got {
    /// No frame came.
    Nothing,
    /// A frame came and was let go (not wanted, or it did not decode).
    Frame,
    Picture(Picture),
}

/// How long one wait for a frame lasts: how soon the loop sees `stop`.
pub(crate) const SLICE: Duration = Duration::from_millis(100);
/// A live camera that sends nothing this long has gone away (unplugged,
/// its driver or its feeder stopped). A webcam in the dark still sends
/// several frames a second.
pub(crate) const QUIET: Duration = Duration::from_millis(1500);
/// How long an opened camera may take to send its first frame (a USB
/// camera settles its exposure first).
pub(crate) const FIRST: Duration = Duration::from_secs(5);

/// Feeds `camera`'s pictures into `feed` until `feed.stop`, an error, or
/// [`QUIET`] without a frame (Gone). The camera is released before its
/// end is told: a view that sees Gone finds the device free.
pub(crate) fn stream(mut camera: impl Source, feed: &CameraFeed) {
    let opened = Instant::now();
    let mut last: Option<Instant> = None;
    let ended = loop {
        if feed.stop.load(Ordering::Relaxed) {
            break None;
        }
        let wanted = feed.wanted.load(Ordering::Relaxed);
        let got = camera.next(SLICE, wanted, feed.mirror.load(Ordering::Relaxed));
        match got {
            Err(status) => break Some(status),
            Ok(Got::Nothing) => match last {
                Some(at) if at.elapsed() > QUIET => break Some(Status::Gone(String::new())),
                None if opened.elapsed() > FIRST => {
                    break Some(Status::Failed("the camera sent no picture".into()));
                }
                _ => {}
            },
            Ok(got) => {
                if last.is_none() {
                    set(&feed.status, Status::Live);
                }
                last = Some(Instant::now());
                if let Got::Picture(picture) = got {
                    *feed.picture.lock().expect("feed picture") = Some(picture);
                }
            }
        }
    };
    drop(camera);
    if let Some(status) = ended {
        set(&feed.status, status);
    }
}

// ---------- microphones and speakers ----------

/// The microphones and the speakers the system lists now.
pub fn audio_devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    let host = cpal::default_host();
    let kinds = |input: bool| {
        listed(&host, input).map(|listed| {
            listed
                .into_iter()
                .map(|(device, _)| AudioDevice {
                    kind: platform::audio_kind(&device, input),
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

#[cfg(not(target_os = "linux"))]
fn recognizable<T>(devices: Vec<(Device, T)>) -> Vec<(Device, T)> {
    devices
}

/// Opens `device` (the default microphone when `None`) on a thread of its
/// own: its level into `feed`, a recording asked through `feed.ask`
/// played back on `feed.output`, and the monitor while `feed.monitor`,
/// until `feed.stop`.
pub fn start_microphone(device: Option<String>, feed: Arc<MicFeed>) {
    let spawned = std::thread::Builder::new()
        .name("av-microphone".into())
        .spawn(move || run_microphone(device, &feed));
    if let Err(error) = spawned {
        tracing_error("microphone thread", &error);
    }
}

/// The recording being made, mono: the samples so far and how many are
/// wanted.
type Recording = Arc<Mutex<Option<(Vec<f32>, usize)>>>;

/// The longest recording a piece makes, in seconds: it waits whole in
/// memory for its playback, so a longer ask records this long.
pub(crate) const MAX_RECORD_SECONDS: u8 = 10;

/// The samples a recording of `seconds` holds at `rate`: one second at
/// least, [`MAX_RECORD_SECONDS`] at most.
fn recorded(rate: u32, seconds: u8) -> usize {
    rate as usize * usize::from(seconds.clamp(1, MAX_RECORD_SECONDS))
}
/// Mono samples on their way to a speaker.
type Queue = Arc<Mutex<VecDeque<f32>>>;

/// The microphone (`input`) or speaker a piece names: `None` the system's
/// default, else only one this crate listed ([`listed`]), its id compared
/// whole. A view's string never reaches the system: on Linux an id is an
/// ALSA PCM name, and such a name can name a plugin that writes a file or
/// runs a program.
fn find(host: &cpal::Host, id: Option<&str>, input: bool) -> Option<cpal::Device> {
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

fn audio_fault(error: &cpal::Error, open: bool) -> Status {
    match error.kind() {
        cpal::ErrorKind::DeviceBusy => Status::Busy,
        cpal::ErrorKind::PermissionDenied => Status::Denied,
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated if open => {
            Status::Gone(String::new())
        }
        cpal::ErrorKind::DeviceNotAvailable => Status::NoDevice,
        _ => Status::Failed(error.to_string().chars().take(200).collect()),
    }
}

fn run_microphone(device: Option<String>, feed: &Arc<MicFeed>) {
    // dropped last, after the microphone and the speaker
    let _held = MICROPHONES.hold(feed.clone());
    // its piece went before this opened anything: nothing to open
    if feed.let_go() {
        return;
    }
    let host = cpal::default_host();
    let Some(input) = find(&host, device.as_deref(), true) else {
        return set(&feed.status, Status::NoDevice);
    };
    if !platform::microphone_allowed() {
        return set(&feed.status, Status::Denied);
    }
    // a permission prompt can take the person a while: a piece gone
    // meanwhile opens nothing
    if feed.let_go() {
        return;
    }
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
    // the speaker's streams: a recording's playback, the monitor (with
    // the output it plays on)
    let mut playing: Option<(cpal::Stream, Queue)> = None;
    let mut monitoring: Option<(cpal::Stream, Option<String>)> = None;
    let output = || feed.output.lock().expect("feed output").clone();
    let playback = |to: Playback| *feed.playback.lock().expect("feed playback") = to;
    while !feed.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        let asked = feed.ask.lock().expect("feed ask").take();
        if let Some(ask) = asked {
            playing = None;
            *recording.lock().expect("recording") = match ask {
                Ask::Record(seconds) => Some((Vec::new(), recorded(rate, seconds))),
                Ask::Stop => None,
            };
            playback(match ask {
                Ask::Record(_) => Playback::Recording,
                Ask::Stop => Playback::Idle,
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
            match speaker(&host, output().as_deref(), rate, queue.clone()) {
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
        // the monitor follows the output: another speaker picked moves it
        let monitor = feed.monitor.load(Ordering::Relaxed).then(output);
        if monitoring.as_ref().map(|(_, on)| on) != monitor.as_ref() {
            monitoring = None;
        }
        if let Some(on) = monitor
            && monitoring.is_none()
        {
            heard.lock().expect("heard").clear();
            match speaker(&host, on.as_deref(), rate, heard.clone()) {
                Ok(stream) => monitoring = Some((stream, on)),
                Err(status) => {
                    set(&feed.status, status);
                    feed.monitor.store(false, Ordering::Relaxed);
                }
            }
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

/// [`crate::play_chime`]: the chime on its own speaker stream, waited
/// out. Bounded: a speaker that takes no samples for 2 s past the chime's
/// length is an error, not a wait forever.
pub fn play_chime(output: Option<&str>) -> Result<(), String> {
    let host = cpal::default_host();
    let rate = find(&host, output, false)
        .ok_or("no such speaker")?
        .default_output_config()
        .map_err(|error| error.to_string())?
        .sample_rate();
    let samples = chime(rate);
    let length = Duration::from_secs_f32(samples.len() as f32 / rate.max(1) as f32);
    let queue = Queue::new(Mutex::new(VecDeque::from(samples)));
    let stream = speaker(&host, output, rate, queue.clone()).map_err(|status| match status {
        Status::Failed(why) => why,
        Status::Denied => "the system refused the speaker".into(),
        Status::Busy => "another app is using the speaker".into(),
        _ => "the speaker is gone".into(),
    })?;
    let deadline = std::time::Instant::now() + length + Duration::from_secs(2);
    while !queue.lock().expect("chime").is_empty() {
        if std::time::Instant::now() > deadline {
            return Err("the speaker did not play".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // ponytail: the last callback's buffer may still be sounding, as for
    // a playback; a short tail is cut rather than waited on
    std::thread::sleep(Duration::from_millis(100));
    drop(stream);
    Ok(())
}

/// The test chime, mono at `rate`: 0.3 s of 440 Hz then 0.3 s of 660 Hz
/// at -12 dBFS, each note faded in and out over 10 ms so it does not click.
pub(crate) fn chime(rate: u32) -> Vec<f32> {
    let rate = rate.max(1) as f32;
    let note = (rate * 0.3) as usize;
    let fade = (rate * 0.01) as usize;
    let amplitude = 10f32.powf(-12. / 20.);
    [440f32, 660.]
        .into_iter()
        .flat_map(|pitch| {
            (0..note).map(move |at| {
                let edge = at.min(note - 1 - at).min(fade) as f32 / fade.max(1) as f32;
                let phase = std::f32::consts::TAU * pitch * at as f32 / rate;
                amplitude * edge * phase.sin()
            })
        })
        .collect()
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
    tracing::error!(target: "ducktape::av", %error, "{what} did not start");
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

    /// A recording holds one to ten seconds, whatever its piece asks.
    #[test]
    fn a_recording_is_one_to_ten_seconds() {
        assert_eq!(recorded(48_000, 0), 48_000);
        assert_eq!(recorded(48_000, 4), 4 * 48_000);
        assert_eq!(recorded(48_000, u8::MAX), 10 * 48_000);
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

    /// Against the system's own audio devices: ALSA's `null` plugin, which
    /// cpal lists and this crate does not, and plugins that write a file or
    /// run a program, open nothing, and a microphone piece that names one
    /// is told there is no such device, its file never written.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_name_this_crate_did_not_list_reaches_no_device() {
        let host = cpal::default_host();
        let written = std::env::temp_dir().join(format!("ducktape-av-{}", std::process::id()));
        let path = written.display();
        let names = [
            "alsa:null".to_owned(),
            format!("alsa:file:'{path}',raw"),
            format!("alsa:file:'|touch {path}',raw"),
        ];
        for name in &names {
            assert!(find(&host, Some(name), true).is_none(), "{name}");
            assert!(find(&host, Some(name), false).is_none(), "{name}");
            let feed = Arc::new(MicFeed::default());
            *feed.output.lock().unwrap() = Some(name.clone());
            run_microphone(Some(name.clone()), &feed);
            assert_eq!(feed.status.lock().unwrap().0, Status::NoDevice, "{name}");
        }
        assert!(!written.exists(), "a plugin wrote {path}");
    }

    /// A device whose id is past the bound is not listed: no piece could
    /// name it back.
    #[test]
    fn a_device_id_past_the_bound_is_not_listed() {
        let device = |id: String| Device {
            id,
            name: "Camera".into(),
            default: false,
        };
        let longest = "d".repeat(MAX_ID_BYTES);
        let past = "d".repeat(MAX_ID_BYTES + 1);
        let kept: Vec<String> = [
            device("video0".into()),
            device(past.clone()),
            device(longest.clone()),
        ]
        .into_iter()
        .filter(nameable)
        .map(|device| device.id)
        .collect();
        assert_eq!(kept, ["video0".to_owned(), longest]);
    }

    /// The chime is 0.6 s, two notes, never louder than -12 dBFS, and
    /// starts and ends in silence.
    #[test]
    fn the_chime_is_two_short_quiet_notes() {
        let samples = chime(48_000);
        assert_eq!(samples.len(), 28_800);
        let peak = samples.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!(peak <= 0.252 && peak > 0.24, "{peak}");
        assert_eq!(samples[0], 0.);
        assert!(samples[samples.len() - 1].abs() < 1e-3);
        // zero crossings: 440 Hz then 660 Hz, each for 0.3 s
        let crossings = |half: &[f32]| half.windows(2).filter(|w| w[0] < 0. && w[1] >= 0.).count();
        let (low, high) = samples.split_at(14_400);
        assert!((130..=133).contains(&crossings(low)), "{}", crossings(low));
        assert!(
            (196..=199).contains(&crossings(high)),
            "{}",
            crossings(high)
        );
    }

    /// THE ROOT FIX: a piece made again while its old self's thread is
    /// still closing the device (Try again, the pane moved, another place
    /// in its view) opens only once that one has closed it; a holder whose
    /// piece is still there holds nobody up.
    #[test]
    fn a_device_opens_only_once_the_one_let_go_has_closed() {
        static HOLDERS: Holders = Holders::new();
        let old = Arc::new(MicFeed::default());
        let closing = HOLDERS.hold(old.clone());
        // its piece let go; its thread has not closed the device yet
        old.stop.store(true, Ordering::Relaxed);
        let (opened, heard) = std::sync::mpsc::channel();
        let next = std::thread::spawn(move || {
            let held = HOLDERS.hold(Arc::new(MicFeed::default()));
            opened.send(Instant::now()).unwrap();
            held
        });
        assert!(
            heard.recv_timeout(Duration::from_millis(200)).is_err(),
            "opened while the old one still held the device"
        );
        let closed = Instant::now();
        drop(closing);
        let at = heard.recv_timeout(Duration::from_secs(1)).expect("opens");
        assert!(at >= closed);
        let _live = next.join().unwrap();
        let waited = Instant::now();
        let _other = HOLDERS.hold(Arc::new(MicFeed::default()));
        assert!(
            waited.elapsed() < Duration::from_millis(100),
            "a live holder held another up"
        );
    }

    /// A camera that delivers `frames` frames, then nothing.
    struct Fake {
        frames: usize,
        run: Run,
    }

    /// What a test reads off a fake camera's run: whether it was released,
    /// when it delivered its first and last frames, and how many waits
    /// began after its piece let go.
    #[derive(Clone)]
    struct Run {
        feed: Arc<CameraFeed>,
        released: Arc<AtomicBool>,
        delivered: Arc<Mutex<Option<(Instant, Instant)>>>,
        after_stop: Arc<AtomicU32>,
    }

    impl Source for Fake {
        fn next(&mut self, wait: Duration, wanted: bool, _: bool) -> Result<Got, Status> {
            if self.run.feed.let_go() {
                self.run.after_stop.fetch_add(1, Ordering::SeqCst);
            }
            if self.frames == 0 {
                std::thread::sleep(wait);
                return Ok(Got::Nothing);
            }
            self.frames -= 1;
            std::thread::sleep(Duration::from_millis(33));
            let now = Instant::now();
            let mut delivered = self.run.delivered.lock().unwrap();
            *delivered = Some((delivered.map_or(now, |(first, _)| first), now));
            Ok(match wanted {
                true => Got::Picture(Picture {
                    width: 1,
                    height: 1,
                    bgra: vec![0; 4],
                }),
                false => Got::Frame,
            })
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.run.released.store(true, Ordering::SeqCst);
        }
    }

    /// Runs `stream` over a fake camera on a thread of its own.
    fn streaming(frames: usize, wanted: bool) -> Run {
        let run = Run {
            feed: Arc::new(CameraFeed::default()),
            released: Arc::default(),
            delivered: Arc::default(),
            after_stop: Arc::default(),
        };
        run.feed.wanted.store(wanted, Ordering::Relaxed);
        let camera = Fake {
            frames,
            run: run.clone(),
        };
        let feed = run.feed.clone();
        std::thread::spawn(move || stream(camera, &feed));
        run
    }

    fn status(feed: &CameraFeed) -> Status {
        feed.status.lock().unwrap().0.clone()
    }

    /// Waits for `what` to hold, and returns when it was seen: the event is
    /// waited on, however loaded the machine (a failsafe of 30 s ends a
    /// test that would wait forever).
    fn until(what: &str, holds: impl Fn() -> bool) -> Instant {
        let failsafe = Instant::now() + Duration::from_secs(30);
        loop {
            if holds() {
                return Instant::now();
            }
            assert!(Instant::now() < failsafe, "never {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// THE HEADLINE: a live camera that stops delivering is told Gone no
    /// sooner than [`QUIET`] after its last frame, and only once released.
    #[test]
    fn a_camera_that_stops_delivering_is_gone_and_released() {
        let run = streaming(3, true);
        until("live", || status(&run.feed) == Status::Live);
        let gone = until("gone", || status(&run.feed) == Status::Gone(String::new()));
        assert!(
            run.released.load(Ordering::SeqCst),
            "told Gone before release"
        );
        assert!(run.feed.picture.lock().unwrap().is_some());
        let (_, last) = run.delivered.lock().unwrap().expect("frames came");
        assert!(
            gone - last > QUIET,
            "{:?} after the last frame",
            gone - last
        );
    }

    /// Stop releases the camera within the wait under way; a camera taken
    /// only (nothing draws it) gives no pictures and is not mistaken for
    /// gone.
    #[test]
    fn stop_releases_at_once_and_a_hidden_camera_stays_live() {
        let run = streaming(usize::MAX, false);
        until("live", || status(&run.feed) == Status::Live);
        until("frames past QUIET", || {
            run.delivered
                .lock()
                .unwrap()
                .is_some_and(|(first, last)| last - first > QUIET + SLICE * 2)
        });
        assert_eq!(status(&run.feed), Status::Live);
        assert!(run.feed.picture.lock().unwrap().is_none());
        run.feed.stop();
        until("released", || run.released.load(Ordering::SeqCst));
        assert!(
            run.after_stop.load(Ordering::SeqCst) <= 1,
            "waited again after the stop"
        );
        assert_eq!(status(&run.feed), Status::Live);
    }
}
