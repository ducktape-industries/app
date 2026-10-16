//! THE SEAM: every call into a capture or playback library, and nothing of
//! gpui. Cameras through our own capture (V4L2 on Linux in `v4l2.rs`,
//! AVFoundation on macOS and iOS in `apple.rs`, Camera2 on Android in
//! `android.rs`) driven by one stream loop here,
//! microphones and speakers through cpal, each piece on a thread of its
//! own that leaves what it has in a feed ([`CameraFeed`], [`MicFeed`]) for
//! the gpui side to read. Another backend replaces this file and those
//! two: the feeds, [`Device`], [`Status`] and the four functions are the
//! whole surface (each OS lists its own microphones and speakers:
//! `platform::audio_devices`).
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, StreamTrait as _};

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
    /// the V4L2 node (`video0`) on Linux, the unique id on macOS, the
    /// Camera2 id on Android. Only a listing fills it.
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
    /// The monitor as its piece asks it ([`MicFeed::ask_monitor`]).
    monitor: AtomicBool,
    /// The monitor turned off under its piece ([`MicFeed::silence`]).
    silenced: AtomicBool,
    /// How the microphone ended under its piece, not yet told
    /// ([`MicFeed::end`]).
    ended: Mutex<Option<Status>>,
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

    /// The piece's monitor, on or off, asked as often as it likes: on
    /// plays what the microphone hears straight back, unless it was
    /// turned off under the piece ([`MicFeed::silence`]), which holds
    /// until the piece asks it off.
    pub fn ask_monitor(&self, on: bool) {
        if !on {
            self.silenced.store(false, Ordering::Relaxed);
        }
        self.monitor.store(on, Ordering::Relaxed);
    }

    /// Whether the monitor plays: asked on, and not turned off since.
    pub fn monitoring(&self) -> bool {
        self.monitor.load(Ordering::Relaxed) && !self.silenced.load(Ordering::Relaxed)
    }

    /// Turns the monitor off under its piece: its sound fell to the
    /// loudspeaker, where it would howl, or its speaker failed.
    pub(crate) fn silence(&self) {
        self.silenced.store(true, Ordering::Relaxed);
    }

    /// Ends the microphone under its piece (a call took it): its thread
    /// closes every stream, then tells `why`, as a camera's does, so a
    /// piece told so has its device let go already.
    pub(crate) fn end(&self, why: Status) {
        *self.ended.lock().expect("feed ended") = Some(why);
    }
}

/// Where a microphone's route moved under it, as the system tells it
/// ([`moved`]).
pub(crate) struct Moved {
    /// The microphone heard now, as listed.
    pub(crate) heard: Option<Device>,
    /// Whether its sound fell to the device's own loudspeaker from
    /// elsewhere (the headphones gone, the loudspeaker picked).
    pub(crate) to_loudspeaker: bool,
}

/// A microphone's route moved under it: a piece that named its
/// microphone (`named`) and hears another now ends Gone, saying so (the
/// system never swaps it silently); one on the default takes the new
/// one's name. A monitor playing when the sound fell to the loudspeaker
/// stops, until its piece turns it off and on.
pub(crate) fn moved(feed: &MicFeed, named: Option<&str>, moved: Moved) {
    if moved.to_loudspeaker && feed.monitoring() {
        feed.silence();
    }
    let mut status = feed.status.lock().expect("feed status");
    match (named, moved.heard) {
        (Some(named), heard) if heard.as_ref().is_none_or(|heard| heard.id != named) => {
            if matches!(status.0, Status::Starting | Status::Live) {
                status.0 = Status::Gone("The microphone you picked is no longer in use.".into());
            }
        }
        (None, Some(heard)) => status.1 = heard.name,
        _ => {}
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
pub(crate) fn nameable(device: &Device) -> bool {
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

fn audio_fault(error: &cpal::Error, open: bool) -> Status {
    match error.kind() {
        cpal::ErrorKind::DeviceBusy => Status::Busy,
        cpal::ErrorKind::PermissionDenied => Status::Denied,
        cpal::ErrorKind::DeviceNotAvailable if open => {
            Status::Gone("The microphone was disconnected.".into())
        }
        cpal::ErrorKind::StreamInvalidated if open => {
            Status::Gone("The system stopped the microphone.".into())
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
    let Some(input) = platform::find(&host, device.as_deref(), true) else {
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
    // the system's audio session (a phone's) held while the microphone is
    // open, its ends told to the feed
    let _session = match platform::activate(Some((feed, device.as_deref()))) {
        Ok(session) => session,
        Err(status) => return set(&feed.status, status),
    };
    feed.status.lock().expect("feed status").1 = platform::input_name(&input);
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
        device.as_deref(),
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
    serve(stream, &host, rate, &recording, &heard, feed);
    feed.level.store(0f32.to_bits(), Ordering::Relaxed);
}

/// Serves an open microphone, `input` its stream, until its piece lets it
/// go or the device ends: Live from the start, unless something ended it
/// already (an interruption told before), and over the moment its status
/// is anything but Live (Gone, Busy, a failure), as a camera's [`stream`]
/// is: a microphone the system took never comes back on by itself. One
/// ended under its piece ([`MicFeed::end`]) is told so once every stream
/// closed. What its piece asks (a recording played back, the monitor)
/// plays on its speaker meanwhile. Every stream, `input` and the
/// speaker's, is closed when this returns: the device let go.
fn serve<S>(
    input: S,
    host: &cpal::Host,
    rate: u32,
    recording: &Recording,
    heard: &Queue,
    feed: &MicFeed,
) {
    let ended = || feed.ended.lock().expect("feed ended").is_some();
    {
        let mut status = feed.status.lock().expect("feed status");
        if status.0 == Status::Starting && !ended() {
            status.0 = Status::Live;
        }
    }
    let live = || feed.status.lock().expect("feed status").0 == Status::Live && !ended();
    // the speaker's streams: a recording's playback, the monitor (with
    // the output it plays on)
    let mut playing: Option<(cpal::Stream, Queue)> = None;
    let mut monitoring: Option<(cpal::Stream, Option<String>)> = None;
    let output = || feed.output.lock().expect("feed output").clone();
    let playback = |to: Playback| *feed.playback.lock().expect("feed playback") = to;
    // a speaker stream on the output `on` names, found as it starts
    let play = |on: Option<&str>, queue: Queue| {
        let device = platform::find(host, on, false).ok_or(Status::Failed("no speaker".into()))?;
        speaker(&device, rate, queue)
    };
    while !feed.stop.load(Ordering::Relaxed) && live() {
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
            match play(output().as_deref(), queue.clone()) {
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
        let monitor = feed.monitoring().then(output);
        if monitoring.as_ref().map(|(_, on)| on) != monitor.as_ref() {
            monitoring = None;
        }
        if let Some(on) = monitor
            && monitoring.is_none()
        {
            heard.lock().expect("heard").clear();
            match play(on.as_deref(), heard.clone()) {
                Ok(stream) => monitoring = Some((stream, on)),
                Err(status) => {
                    set(&feed.status, status);
                    feed.silence();
                }
            }
        }
    }
    drop((input, playing, monitoring));
    if let Some(why) = feed.ended.lock().expect("feed ended").take() {
        set(&feed.status, why);
    }
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

/// The microphone's stream (`input`, the one its piece `named`, or the
/// default): its level into `feed`, its samples (mono) into the
/// recording being made and, while the monitor runs, `heard`.
fn capture(
    input: &cpal::Device,
    named: Option<&str>,
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
        if feed.monitoring() {
            // ponytail: at most a fifth of a second behind; drift between
            // the two clocks is dropped here, not resampled
            let mut heard = heard.lock().expect("heard");
            heard.extend(&mono);
            let over = heard.len().saturating_sub(rate / 5);
            heard.drain(..over);
        }
    };
    let named = named.map(str::to_owned);
    let on_error = move |error: cpal::Error| match error.kind() {
        // a phone's session routes the stream in place and tells a route
        // change (another speaker picked, a headset in or out) as a
        // changed or invalidated stream, though it plays on: it tells
        // its moves and ends itself (`platform::activate`)
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::StreamInvalidated
            if platform::ROUTED_IN_PLACE => {}
        // the stream followed the system's default to another microphone
        cpal::ErrorKind::DeviceChanged => {
            let to = Moved {
                heard: platform::heard(),
                to_loudspeaker: false,
            };
            moved(&errors, named.as_deref(), to);
        }
        // the microphone gone under its piece: told once its streams closed
        cpal::ErrorKind::DeviceNotAvailable
        | cpal::ErrorKind::BackendError
        | cpal::ErrorKind::StreamInvalidated => errors.end(audio_fault(&error, true)),
        _ => {}
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
    let said = |status| match status {
        Status::Failed(why) => why,
        Status::Denied => "the system refused the speaker".into(),
        Status::Busy => "another app is using the speaker".into(),
        _ => "the speaker is gone".into(),
    };
    let host = cpal::default_host();
    let found = platform::find(&host, output, false).ok_or("no such speaker")?;
    // the system's audio session (a phone's) held while the chime plays,
    // once the speaker is known: an id that names none activates nothing
    let _session = platform::activate(None).map_err(said)?;
    let rate = found
        .default_output_config()
        .map_err(|error| error.to_string())?
        .sample_rate();
    let samples = chime(rate);
    let length = Duration::from_secs_f32(samples.len() as f32 / rate.max(1) as f32);
    let queue = Queue::new(Mutex::new(VecDeque::from(samples)));
    // on the speaker found above: found again, the id could name none
    // once the session left recording (a Bluetooth headset's hands-free
    // id, listed among the inputs)
    let stream = speaker(&found, rate, queue.clone()).map_err(said)?;
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

/// A stream on `speaker` playing `queue` (mono at `rate`) on every
/// channel, silent once it is empty.
fn speaker(speaker: &cpal::Device, rate: u32, queue: Queue) -> Result<cpal::Stream, Status> {
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

    /// A microphone's route moved under it: a piece that named its
    /// microphone and hears another ends Gone, saying so; one on the
    /// default takes the new one's name. A monitor playing when the sound
    /// fell to the loudspeaker stops, and asked on again stays off until
    /// its piece asks it off; one off is not held off.
    #[test]
    fn a_moved_microphone_is_told_and_its_monitor_stops_on_the_loudspeaker() {
        let live = || {
            let feed = MicFeed::default();
            feed.status.lock().unwrap().0 = Status::Live;
            feed
        };
        let to = |id: &str, name: &str, to_loudspeaker| Moved {
            heard: Some(Device {
                id: id.into(),
                name: name.into(),
                default: true,
            }),
            to_loudspeaker,
        };
        let feed = live();
        moved(&feed, Some("headset"), to("headset", "Headset", false));
        assert_eq!(feed.status.lock().unwrap().0, Status::Live);
        moved(&feed, Some("headset"), to("built-in", "Built-in", false));
        assert!(matches!(&feed.status.lock().unwrap().0, Status::Gone(why) if !why.is_empty()));
        let feed = live();
        let none = Moved {
            heard: None,
            to_loudspeaker: false,
        };
        moved(&feed, Some("headset"), none);
        assert!(matches!(feed.status.lock().unwrap().0, Status::Gone(_)));
        let feed = live();
        moved(&feed, None, to("headset", "Headset", false));
        assert_eq!(
            *feed.status.lock().unwrap(),
            (Status::Live, "Headset".into())
        );
        feed.ask_monitor(true);
        moved(&feed, None, to("built-in", "Built-in", true));
        assert!(!feed.monitoring());
        assert_eq!(feed.status.lock().unwrap().1, "Built-in");
        feed.ask_monitor(true);
        assert!(!feed.monitoring(), "asked on again, it stays off");
        feed.ask_monitor(false);
        feed.ask_monitor(true);
        assert!(feed.monitoring());
        let feed = live();
        moved(&feed, None, to("built-in", "Built-in", true));
        feed.ask_monitor(true);
        assert!(feed.monitoring(), "a monitor off is not held off");
    }

    /// A recording holds one to ten seconds, whatever its piece asks.
    #[test]
    fn a_recording_is_one_to_ten_seconds() {
        assert_eq!(recorded(48_000, 0), 48_000);
        assert_eq!(recorded(48_000, 4), 4 * 48_000);
        assert_eq!(recorded(48_000, u8::MAX), 10 * 48_000);
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
            assert!(platform::find(&host, Some(name), true).is_none(), "{name}");
            assert!(platform::find(&host, Some(name), false).is_none(), "{name}");
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

    /// THE CONSENT RULE: a microphone whose status goes anything but Live
    /// (Gone: a call took it) lets its device go though its piece still
    /// holds it, as a camera does: nothing that restarts the device under
    /// it (iOS resumes its audio unit after a call) records again. One
    /// ended before it is served never comes on.
    #[test]
    fn a_microphone_no_longer_live_lets_its_device_go() {
        struct Stream(Arc<AtomicBool>);
        impl Drop for Stream {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let serving = |feed: Arc<MicFeed>, closed: Arc<AtomicBool>| {
            std::thread::spawn(move || {
                let host = cpal::default_host();
                let (recording, heard) = (Recording::default(), Queue::default());
                serve(Stream(closed), &host, 48_000, &recording, &heard, &feed)
            })
        };
        let status = |feed: &MicFeed| feed.status.lock().unwrap().0.clone();
        let feed = Arc::new(MicFeed::default());
        let closed = Arc::new(AtomicBool::new(false));
        let served = serving(feed.clone(), closed.clone());
        until("live", || status(&feed) == Status::Live);
        assert!(!closed.load(Ordering::SeqCst));
        let took = Status::Gone("A call took the microphone.".into());
        set(&feed.status, took.clone());
        until("let go", || closed.load(Ordering::SeqCst));
        served.join().unwrap();
        assert_eq!(status(&feed), took);
        assert!(!feed.let_go(), "its piece still holds it");

        let ended = Arc::new(MicFeed::default());
        set(&ended.status, took.clone());
        let closed = Arc::new(AtomicBool::new(false));
        serving(ended.clone(), closed.clone()).join().unwrap();
        assert!(closed.load(Ordering::SeqCst));
        assert_eq!(status(&ended), took);
    }

    /// A microphone ended under its piece (Android: a call silenced it) is
    /// told so only once its streams closed, as a camera is: no Gone card
    /// over a device still recording.
    #[test]
    fn a_microphone_ended_under_its_piece_is_told_once_let_go() {
        struct Stream(Arc<AtomicBool>);
        impl Drop for Stream {
            fn drop(&mut self) {
                // a device that takes its time to close
                std::thread::sleep(Duration::from_millis(50));
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let status = |feed: &MicFeed| feed.status.lock().unwrap().0.clone();
        let feed = Arc::new(MicFeed::default());
        let closed = Arc::new(AtomicBool::new(false));
        let served = {
            let (feed, closed) = (feed.clone(), closed.clone());
            std::thread::spawn(move || {
                let host = cpal::default_host();
                let (recording, heard) = (Recording::default(), Queue::default());
                serve(Stream(closed), &host, 48_000, &recording, &heard, &feed)
            })
        };
        until("live", || status(&feed) == Status::Live);
        let took = Status::Gone("A call took the microphone.".into());
        feed.end(took.clone());
        until("told", || status(&feed) != Status::Live);
        assert!(closed.load(Ordering::SeqCst), "told before let go");
        assert_eq!(status(&feed), took);
        served.join().unwrap();
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
