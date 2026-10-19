//! The camera and the microphone in a gpui window: a [`Camera`] draws a
//! camera's pictures (the whole picture, centred, mirrored if asked), a
//! [`Meter`] draws a microphone's level and records and plays back, and
//! [`cameras`] and [`audio_devices`] list what can be opened,
//! [`play_chime`] tests a speaker, and [`allow`] says how a person lets a
//! refused device open here. Each piece emits its device's [`State`] as a
//! gpui event. Nothing here knows of views or the wire.
//!
//! Shape:
//! - every device call is in [`backend`] (cpal for microphones and
//!   speakers, on every OS; the camera's stream loop) over one module per
//!   OS, picked below as `platform`: `v4l2` on Linux, `mac` on macOS,
//!   `ios` on iOS (the last two over `apple`, the AVFoundation they
//!   share), `android` on Android. A platform module is the whole OS
//!   seam: `cameras()` (each with the system's own handle to it),
//!   `open(handle)` (a [`backend::Source`]), `camera_allowed()` and
//!   `microphone_allowed()`, what a person is told of a refused device
//!   (`allow`, `settings_page`), and the microphones and speakers:
//!   `audio_devices()` (each saying what it is), `find()` (the cpal
//!   device a piece's id opens), `input_name()`, `heard()` (the
//!   microphone heard now), `activate()` (what is held while a microphone
//!   or the chime plays) and `ROUTED_IN_PLACE` (whether the system moves
//!   a stream in place, then telling an open microphone's moves to
//!   `backend::moved` itself).
//!   Linux and macOS share theirs (`devices`: cpal's own devices,
//!   nothing held); iOS lists its audio session's routes instead of
//!   cpal's one device, routes the session to the one picked, holds it
//!   active and observes its route changes. Android lists cpal's AAudio
//!   devices, one row each a person picks, named by what Android says of
//!   them (`recognizable`, in `android_rules`, built for every OS's tests
//!   so they run on Linux), and holds a watcher while a microphone is
//!   open: a recording Android silenced (`silenced`) ends it, the
//!   monitor's sound falling onto the loudspeaker turns the monitor off.
//!   Another OS is another module and one more line below; nothing that
//!   calls this crate moves.
//!   Each piece runs its device on a thread of its own that leaves
//!   pictures, the level and its status in a feed; the latest picture
//!   wins. A camera that stops delivering is told gone within
//!   [`backend::QUIET`] and released.
//! - the gpui side reads its feed on a 30 Hz timer while it lives and
//!   redraws only on news: a new picture, a level that moved, a new state.
//! - a piece's device is released when the piece is dropped, or at once
//!   when its [`Release`] is: its thread sees the feed's stop and closes
//!   the device (the camera's light off).
//!
//! - with feature `call`: [`codec`] turns a microphone's samples and a
//!   camera's [`Picture`]s into packets and back; [`voice`] is the one
//!   full-duplex 48 kHz voice I/O of the app, echo-cancelled, whose
//!   playback is [`mixer`]'s sum of every remote voice (each through
//!   [`jitter`]); [`fake`] stands in for every device when
//!   `DUCKTAPE_AV_FAKE=1` (a proof with no camera and no sound card).
//!   [`Camera::remote`] draws a picture someone else's camera sent.
//!
//! Without feature `capture` the crate is empty (`Cargo.toml` says why).
#![cfg(feature = "capture")]

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "android"
)))]
compile_error!(
    "ducktape-av captures cameras on Linux (V4L2), macOS and iOS (AVFoundation) and Android (Camera2) only: build the app without `av` here"
);

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "android", test))]
mod android_rules;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
mod backend;
mod camera;
#[cfg(feature = "call")]
pub mod codec;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod devices;
#[cfg(feature = "call")]
pub mod fake;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(feature = "call")]
pub mod jitter;
#[cfg(target_os = "macos")]
mod mac;
mod meter;
#[cfg(feature = "call")]
pub mod mixer;
#[cfg(target_os = "linux")]
mod pulse;
#[cfg(target_os = "linux")]
mod v4l2;
#[cfg(feature = "call")]
pub mod voice;

#[cfg(target_os = "android")]
use android as platform;
#[cfg(target_os = "ios")]
use ios as platform;
#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(target_os = "linux")]
use v4l2 as platform;

#[cfg(target_os = "android")]
pub use android::{Asker, asks_through};
#[cfg(feature = "test-support")]
pub use backend::Ask;
pub use backend::{
    AudioDevice, AudioKind, CameraDevice, Device, Facing, Playback, Release, Status,
};
#[cfg(feature = "call")]
pub use backend::{CameraFeed, MicFeed, Picture};
pub use camera::Camera;
pub use meter::Meter;
#[cfg(feature = "call")]
pub use meter::Speaking;

/// What a phone's microphone the system took is told (iOS's interrupted
/// session, Android's silenced recording): neither says which took it.
#[cfg(any(target_os = "ios", target_os = "android"))]
const MICROPHONE_TAKEN: &str = "A call, another app or the background took the microphone.";

/// What a piece emits whenever any of it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub status: Status,
    /// The opened device's name; empty while none is.
    pub device: String,
    pub playback: Playback,
    /// A microphone's: the person is speaking ([`meter`]'s hysteresis).
    pub speaking: bool,
    /// A microphone's: its monitor was turned off under its piece and is
    /// held off until the piece asks it off ([`backend::MicFeed::silence`]).
    pub monitor_stopped: bool,
}

/// The cameras the system lists now; the first is the default.
pub fn cameras() -> Result<Vec<CameraDevice>, String> {
    backend::cameras()
}

/// The microphones and the speakers the system lists now.
pub fn audio_devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    platform::audio_devices()
}

/// Plays the test chime on `output` (the system's speaker when `None`)
/// and returns once it has sounded, about 0.7 s: the speaker test, apart
/// from any microphone. Blocks the calling thread.
pub fn play_chime(output: Option<&str>) -> Result<(), String> {
    backend::play_chime(output)
}

pub fn play_sound(output: Option<&str>, samples: Vec<f32>, rate: u32) -> Result<(), String> {
    backend::play_sound(output, samples, rate)
}

/// A camera or a microphone, as a person is told how to allow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Camera,
    Microphone,
}

/// How a person allows `kind` once this system refused it
/// ([`Status::Denied`]), in one line.
pub fn allow(kind: Kind) -> &'static str {
    platform::allow(kind)
}

/// The system's own settings page that allows `kind` for this app, as a
/// link the system opens, where there is one (macOS's privacy pane,
/// iOS's and Android's page of the app; [`allow`] names it).
pub fn settings_page(kind: Kind) -> Option<&'static str> {
    platform::settings_page(kind)
}

/// How often a piece reads its feed.
const POLL: std::time::Duration = std::time::Duration::from_millis(33);
