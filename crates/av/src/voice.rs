//! The one voice I/O of the app in a call: the microphone and the speaker
//! as a pair at 48 kHz mono, so what the speaker plays (the remote
//! voices, [`crate::mixer`]) is known to the echo canceller that cleans
//! what the microphone hears (sonora: WebRTC's AEC3, noise suppression and
//! AGC2, pure Rust, the one path on every OS; the phones' own voice modes
//! are set as hints beside it, `platform::call`). cpal's streams
//! underneath, on a thread of their own ([`run`]), as a microphone piece's
//! are.
//!
//! Holders: a [`Capture`] (a publishing microphone piece: the microphone is
//! open while any lives, so a muted piece that drops its capture releases
//! the device and the system's mark goes off) and a [`Voice`] (a remote
//! audio piece: the speaker plays while any lives). The I/O itself lives
//! from the first holder to the last ([`hold`]).
//!
//! With `DUCKTAPE_AV_FAKE=1` no device opens: the microphone is a 440 Hz
//! tone and the speaker's frames go to the tap file ([`crate::fake`]).
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, StreamTrait as _};

use crate::backend::{self, MicFeed, Status};
use crate::codec::{FRAME, SAMPLE_RATE};
use crate::mixer::{self, Mixer, Source};
use crate::platform;

/// Sonora's block: 10 ms.
const BLOCK: usize = 480;
/// Frames a capture holds for its reader before the oldest goes: the
/// encoder is behind by more than this only when something is wrong.
const CAPTURE_BACKLOG: usize = 8;
/// How often the I/O thread looks at who holds it.
const TICK: Duration = Duration::from_millis(10);
/// The echo path's delay the canceller is told, microphone in to speaker
/// out, when `DUCKTAPE_AEC_DELAY_MS` names none: two 20 ms device buffers
/// and the mixer's frame. A real device drifts from it; the knob stays.
const DELAY_MS: i32 = 60;
/// How long after a device would not open (or was taken) before it is
/// asked again.
const RETRY: Duration = Duration::from_secs(2);

/// When a device that failed is opened again: after [`RETRY`], except one
/// the system refused (Denied), which waits until nothing wants it, so
/// the phones' permission prompt is not raised over and over.
#[derive(Default)]
struct Retry {
    until: Option<Instant>,
    denied: bool,
}

impl Retry {
    fn may(&self, now: Instant) -> bool {
        !self.denied && self.until.is_none_or(|until| now >= until)
    }

    fn failed(&mut self, now: Instant, status: &Status) {
        match status {
            Status::Denied => self.denied = true,
            _ => self.until = Some(now + RETRY),
        }
    }

    /// Nothing wants the device: the next want opens it at once.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The app's voice I/O while anything holds it.
pub struct Io {
    /// The microphone's status and name (Live while open; Starting while
    /// no capture asks for it).
    status: Mutex<(Status, String)>,
    /// The speaker's status (Live while open; Starting while no voice
    /// asks for it).
    speaker: Mutex<Status>,
    /// What the phone's audio session tells of the microphone (an
    /// interruption, a route moved): read each tick, told as the status.
    feed: Arc<MicFeed>,
    apm: Mutex<sonora::AudioProcessing>,
    mixer: Mixer,
    captures: Mutex<Vec<mpsc::SyncSender<[f32; FRAME]>>>,
    /// How many [`Capture`]s live: the microphone is open while any does.
    capturing: AtomicUsize,
    /// The microphone's level after processing, 0 to 1, as `f32` bits.
    level: AtomicU32,
    /// The microphone and speaker the first holder named (`None`: the
    /// defaults). ponytail: one pair for the app; another piece's choice
    /// is ignored until the I/O is made again.
    device: Option<String>,
    output: Option<String>,
    delay_ms: i32,
    stop: AtomicBool,
}

static IO: Mutex<Weak<Io>> = Mutex::new(Weak::new());

/// Told `true` when the voice I/O is made and `false` when its last holder
/// lets go: the app's call, as what stands outside it sees it (the phone's
/// call screen, iOS CallKit through the launcher, ends its call with it).
/// Told on the thread that makes or drops the I/O, any thread, under the
/// voice's lock: a follower must not hold the voice itself, and should
/// hand the news on and return. One for the process, set before the first
/// hold ([`on_life`]).
static LIFE: std::sync::OnceLock<fn(bool)> = std::sync::OnceLock::new();

/// Sets what is told the voice I/O's life ([`LIFE`]); the first set stands.
pub fn on_life(told: fn(bool)) {
    if LIFE.set(told).is_err() {
        tracing::warn!(target: "ducktape::av", "the voice I/O's life is already followed");
    }
}

/// The app's voice I/O, made on the first hold (its thread started, the
/// microphone and speaker on `device` and `output`) and gone with the last
/// holder.
pub fn hold(device: Option<String>, output: Option<String>) -> Arc<Io> {
    let mut held = lock(&IO);
    if let Some(io) = held.upgrade() {
        return io;
    }
    let delay_ms = std::env::var("DUCKTAPE_AEC_DELAY_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(DELAY_MS);
    let io = Arc::new(Io {
        status: Mutex::new((Status::Starting, String::new())),
        speaker: Mutex::new(Status::Starting),
        feed: Arc::new(MicFeed::default()),
        apm: Mutex::new(apm()),
        mixer: Mixer::default(),
        captures: Mutex::new(Vec::new()),
        capturing: AtomicUsize::new(0),
        level: AtomicU32::new(0),
        device,
        output,
        delay_ms,
        stop: AtomicBool::new(false),
    });
    *held = Arc::downgrade(&io);
    let weak = held.clone();
    let spawned = std::thread::Builder::new()
        .name("av-voice".into())
        .spawn(move || match crate::fake::on() {
            true => crate::fake::run_voice(weak),
            false => run(weak),
        });
    if let Err(error) = spawned {
        tracing::error!(target: "ducktape::av", %error, "voice thread did not start");
    }
    if let Some(told) = LIFE.get() {
        told(true);
    }
    io
}

fn apm() -> sonora::AudioProcessing {
    use sonora::config::{EchoCanceller, GainController2, NoiseSuppression};
    let config = sonora::Config {
        echo_canceller: Some(EchoCanceller::default()),
        noise_suppression: Some(NoiseSuppression::default()),
        gain_controller2: Some(GainController2::default()),
        ..Default::default()
    };
    sonora::AudioProcessing::builder()
        .config(config)
        .capture_config(sonora::StreamConfig::new(SAMPLE_RATE, 1))
        .render_config(sonora::StreamConfig::new(SAMPLE_RATE, 1))
        .build()
}

impl Drop for Io {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // under the hold's lock, and only when no I/O was made since: a
        // hold that raced this drop made a new one, and the voice goes on
        // (`strong_count`, never `upgrade`: a strong reference taken here
        // could be some I/O's last, and its drop would re-enter under the
        // lock)
        let held = lock(&IO);
        if held.strong_count() == 0
            && let Some(told) = LIFE.get()
        {
            told(false);
        }
    }
}

impl backend::Held for Io {
    fn let_go(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.capturing.load(Ordering::Relaxed) == 0
    }
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Io {
    /// The microphone's status and name.
    pub fn status(&self) -> (Status, String) {
        lock(&self.status).clone()
    }

    /// The microphone's level now, 0 to 1 (-60 to 0 dBFS), after
    /// processing.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// The speaker's status.
    pub fn speaker(&self) -> Status {
        lock(&self.speaker).clone()
    }

    /// What the microphone hears, echo-cancelled, in 20 ms frames: the
    /// microphone opens while any capture lives.
    pub fn capture(self: &Arc<Self>) -> Capture {
        let (give, frames) = mpsc::sync_channel(CAPTURE_BACKLOG);
        lock(&self.captures).push(give);
        self.capturing.fetch_add(1, Ordering::Relaxed);
        Capture {
            frames,
            io: self.clone(),
        }
    }

    /// One more remote voice on the speaker, which plays while any lives;
    /// `Err` when this build decodes none.
    pub fn voice(self: &Arc<Self>) -> Result<Voice, String> {
        Ok(Voice {
            source: self.mixer.source()?,
            _io: self.clone(),
        })
    }

    /// One processed frame of the microphone's, to every capture; a
    /// capture that is behind loses it.
    pub(crate) fn heard(&self, frame: [f32; FRAME]) {
        self.level
            .store(mixer::level(&frame).to_bits(), Ordering::Relaxed);
        lock(&self.captures).retain(|give| {
            !matches!(
                give.try_send(frame),
                Err(mpsc::TrySendError::Disconnected(_))
            )
        });
    }

    /// Whether any capture lives.
    pub(crate) fn capturing(&self) -> bool {
        self.capturing.load(Ordering::Relaxed) > 0
    }

    pub(crate) fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    pub(crate) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub(crate) fn set_status(&self, status: Status, name: impl Into<String>) {
        *lock(&self.status) = (status, name.into());
    }

    fn set_speaker(&self, status: Status) {
        *lock(&self.speaker) = status;
    }
}

/// A publishing microphone piece's hold on the microphone: its frames.
pub struct Capture {
    frames: mpsc::Receiver<[f32; FRAME]>,
    io: Arc<Io>,
}

impl Capture {
    /// The next frame, waited for at most `wait`.
    pub fn recv_timeout(&self, wait: Duration) -> Option<[f32; FRAME]> {
        self.frames.recv_timeout(wait).ok()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.io.capturing.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A remote audio piece's voice on the speaker.
pub struct Voice {
    source: Source,
    _io: Arc<Io>,
}

impl Voice {
    /// `packet` numbered `seq` by its sender.
    pub fn push(&self, seq: u64, packet: Vec<u8>) {
        self.source.push(seq, packet);
    }

    /// The voice's level now, 0 to 1.
    pub fn level(&self) -> f32 {
        self.source.level()
    }
}

/// The I/O's thread: the microphone open while a capture lives, the
/// speaker while a voice does, until the last holder lets go. The phones'
/// call mode (`platform::call`) is on for the thread's life.
fn run(io: Weak<Io>) {
    let host = cpal::default_host();
    let mut input: Option<(cpal::Stream, backend::Holding, platform::Active)> = None;
    let mut output: Option<(cpal::Stream, platform::Active)> = None;
    let (mut retry_in, mut retry_out) = (Retry::default(), Retry::default());
    platform::call(true);
    loop {
        let Some(io) = io.upgrade() else { break };
        if io.stopped() {
            break;
        }
        let now = Instant::now();
        // the phone's session ended the microphone under the I/O: told,
        // the device let go, and asked again after the retry
        if input.is_some()
            && let Some(why) = io.feed.take_ended()
        {
            input = None;
            retry_in.failed(now, &why);
            io.set_status(why, String::new());
        }
        match (io.capturing(), input.is_some()) {
            (true, false) if retry_in.may(now) => match open_input(&host, &io) {
                Ok(opened) => input = Some(opened),
                Err(status) => {
                    retry_in.failed(now, &status);
                    io.set_status(status, String::new());
                }
            },
            (false, true) => {
                input = None;
                io.set_status(Status::Starting, String::new());
            }
            (false, false) => retry_in.reset(),
            _ => {}
        }
        match (io.mixer.any(), output.is_some()) {
            (true, false) if retry_out.may(now) => match open_output(&host, &io) {
                Ok(opened) => {
                    output = Some(opened);
                    io.set_speaker(Status::Live);
                }
                Err(status) => {
                    retry_out.failed(now, &status);
                    io.set_speaker(status);
                }
            },
            (false, true) => {
                output = None;
                io.set_speaker(Status::Starting);
            }
            (false, false) => retry_out.reset(),
            _ => {}
        }
        drop(io);
        std::thread::sleep(TICK);
    }
    drop((input, output));
    platform::call(false);
}

/// The microphone opened: Live, its frames processed and handed to the
/// captures.
fn open_input(
    host: &cpal::Host,
    io: &Arc<Io>,
) -> Result<(cpal::Stream, backend::Holding, platform::Active), Status> {
    // after any microphone piece that let its device go has closed it
    let holding = backend::hold_microphone(io.clone());
    let device = platform::find(host, io.device.as_deref(), true).ok_or(Status::NoDevice)?;
    if !platform::microphone_allowed() {
        return Err(Status::Denied);
    }
    let session = platform::activate(Some((&io.feed, io.device.as_deref())))?;
    let config = device
        .default_input_config()
        .map_err(|error| backend::audio_fault(&error, false))?;
    let name = platform::input_name(&device);
    let channels = usize::from(config.channels()).max(1);
    let step = f64::from(SAMPLE_RATE) / f64::from(config.sample_rate());
    let mut due = 0f64;
    let mut block = Vec::with_capacity(BLOCK);
    let mut processed = [0f32; BLOCK];
    let mut frame = Vec::with_capacity(FRAME);
    let delay_ms = io.delay_ms;
    let weak = Arc::downgrade(io);
    let mut on_mono = move |sample: f32| {
        // ponytail: nearest-sample rate conversion to 48 kHz
        due += step;
        while due >= 1. {
            due -= 1.;
            block.push(sample);
        }
        if block.len() < BLOCK {
            return;
        }
        let Some(io) = weak.upgrade() else { return };
        {
            let mut apm = lock(&io.apm);
            let _ = apm.set_stream_delay_ms(delay_ms);
            if apm
                .process_capture_f32(&[&block[..BLOCK]], &mut [&mut processed[..]])
                .is_err()
            {
                processed.copy_from_slice(&block[..BLOCK]);
            }
        }
        block.clear();
        frame.extend_from_slice(&processed);
        if frame.len() >= FRAME {
            let mut whole = [0f32; FRAME];
            whole.copy_from_slice(&frame[..FRAME]);
            frame.clear();
            io.heard(whole);
        }
    };
    let errors = io.clone();
    let on_error = move |error: cpal::Error| {
        if matches!(
            error.kind(),
            cpal::ErrorKind::DeviceNotAvailable
                | cpal::ErrorKind::BackendError
                | cpal::ErrorKind::StreamInvalidated
        ) {
            errors.set_status(backend::audio_fault(&error, true), String::new());
        }
    };
    macro_rules! typed {
        ($t:ty) => {
            device.build_input_stream::<$t, _, _>(
                backend::short(&config),
                move |data: &[$t], _| {
                    for chunk in data.chunks(channels) {
                        let mono = chunk
                            .iter()
                            .map(|s| <f32 as cpal::FromSample<$t>>::from_sample_(*s))
                            .sum::<f32>()
                            / chunk.len() as f32;
                        on_mono(mono);
                    }
                },
                on_error,
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
            return Err(Status::Failed(format!("the microphone sends {other}")));
        }
    }
    .map_err(|error| backend::audio_fault(&error, false))?;
    stream
        .play()
        .map_err(|error| backend::audio_fault(&error, false))?;
    io.set_status(Status::Live, name);
    Ok((stream, holding, session))
}

/// The speaker opened: the mixer's frames played, each told to the echo
/// canceller as what the microphone will hear; the system's audio session
/// (a phone's) held while it plays, as for the chime.
fn open_output(
    host: &cpal::Host,
    io: &Arc<Io>,
) -> Result<(cpal::Stream, platform::Active), Status> {
    let device = platform::find(host, io.output.as_deref(), false).ok_or(Status::NoDevice)?;
    let session = platform::activate(None)?;
    let config = device
        .default_output_config()
        .map_err(|error| backend::audio_fault(&error, false))?;
    let channels = usize::from(config.channels()).max(1);
    let step = f64::from(SAMPLE_RATE) / f64::from(config.sample_rate());
    let mut due = 0f64;
    let mut frame = [0f32; FRAME];
    let mut at = FRAME;
    let mut sample = 0f32;
    let mut scratch = [0f32; BLOCK];
    let weak = Arc::downgrade(io);
    let mut next = move || {
        due += step;
        while due >= 1. {
            due -= 1.;
            if at >= FRAME {
                at = 0;
                match weak.upgrade() {
                    Some(io) => {
                        io.mixer.pull(&mut frame);
                        let mut apm = lock(&io.apm);
                        for block in frame.chunks(BLOCK) {
                            let _ = apm.process_render_f32(&[block], &mut [&mut scratch[..]]);
                        }
                    }
                    None => frame.fill(0.),
                }
            }
            sample = frame[at];
            at += 1;
        }
        sample
    };
    macro_rules! typed {
        ($t:ty) => {
            device.build_output_stream::<$t, _, _>(
                backend::short(&config),
                move |data: &mut [$t], _| {
                    for chunk in data.chunks_mut(channels) {
                        let value = <$t as cpal::FromSample<f32>>::from_sample_(next());
                        chunk.fill(value);
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
    .map_err(|error| backend::audio_fault(&error, false))?;
    stream
        .play()
        .map_err(|error| backend::audio_fault(&error, false))?;
    Ok((stream, session))
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// When the next 20 ms frame is due, from `started`, for a loop that
/// ticks the fake and the tests: `n` frames in.
pub(crate) fn frame_due(started: Instant, n: u64) -> Instant {
    started + Duration::from_millis(20 * n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device that failed is not asked again before the retry passes,
    /// one the system refused not until nothing wants it; both open at
    /// once after a reset.
    #[test]
    fn a_failed_device_is_asked_again_after_the_retry_a_denied_one_after_a_reset() {
        let now = Instant::now();
        let mut retry = Retry::default();
        assert!(retry.may(now));
        retry.failed(now, &Status::Busy);
        assert!(!retry.may(now + RETRY / 2));
        assert!(retry.may(now + RETRY));
        retry.failed(now, &Status::Denied);
        assert!(!retry.may(now + RETRY * 10));
        retry.reset();
        assert!(retry.may(now));
    }

    /// The I/O's life is told once each way: made with the first holder
    /// (a second hold is the same I/O), gone with the last. The follower
    /// is the process's one, and `TOLD` counts every I/O of this binary:
    /// this is the only test that holds the voice.
    #[test]
    fn the_voice_io_s_life_is_told_at_its_first_hold_and_its_last_let_go() {
        use std::sync::atomic::AtomicU8;
        static TOLD: AtomicU8 = AtomicU8::new(0);
        fn told(alive: bool) {
            TOLD.fetch_add(if alive { 1 } else { 0x10 }, Ordering::Relaxed);
        }
        on_life(told);
        let first = hold(None, None);
        let second = hold(None, None);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(TOLD.load(Ordering::Relaxed), 0x01);
        drop(first);
        assert_eq!(TOLD.load(Ordering::Relaxed), 0x01);
        drop(second);
        // the I/O's thread may hold the last reference for one of its ticks
        let until = Instant::now() + Duration::from_secs(2);
        while TOLD.load(Ordering::Relaxed) != 0x11 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(TOLD.load(Ordering::Relaxed), 0x11);
    }
}
