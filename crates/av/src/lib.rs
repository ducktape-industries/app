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
//!   share). A platform module is the whole OS seam: `cameras()` (each
//!   with the system's own handle to it), `open(handle)` (a
//!   [`backend::Source`]), `audio_kind()` (what a listed microphone or
//!   speaker is), `camera_allowed()` and `microphone_allowed()`, and what
//!   a person is told of a refused device (`allow`, `settings_page`). iOS
//!   lists its audio session's routes (each saying what it is) instead of
//!   cpal's one device, routes the session to the one picked, and holds
//!   the session active while a microphone or the chime plays
//!   (`audio_devices`, `route`, `activate`). Another OS (Android) is
//!   another module and one more line below; nothing that calls this
//!   crate moves.
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
//! Without feature `capture` the crate is empty (`Cargo.toml` says why).
#![cfg(feature = "capture")]

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
compile_error!(
    "ducktape-av captures cameras on Linux (V4L2), macOS and iOS (AVFoundation) only: build the app without `av` here"
);

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
mod backend;
mod camera;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod mac;
mod meter;
#[cfg(target_os = "linux")]
mod v4l2;

#[cfg(target_os = "ios")]
use ios as platform;
#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(target_os = "linux")]
use v4l2 as platform;

#[cfg(feature = "test-support")]
pub use backend::Ask;
pub use backend::{
    AudioDevice, AudioKind, CameraDevice, Device, Facing, Playback, Release, Status,
};
pub use camera::Camera;
pub use meter::Meter;

/// What a piece emits whenever any of it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub status: Status,
    /// The opened device's name; empty while none is.
    pub device: String,
    pub playback: Playback,
    /// A microphone's: the person is speaking ([`meter`]'s hysteresis).
    pub speaking: bool,
}

/// The cameras the system lists now; the first is the default.
pub fn cameras() -> Result<Vec<CameraDevice>, String> {
    backend::cameras()
}

/// The microphones and the speakers the system lists now.
pub fn audio_devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    backend::audio_devices()
}

/// Plays the test chime on `output` (the system's speaker when `None`)
/// and returns once it has sounded, about 0.7 s: the speaker test, apart
/// from any microphone. Blocks the calling thread.
pub fn play_chime(output: Option<&str>) -> Result<(), String> {
    backend::play_chime(output)
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
/// iOS's page of the app; [`allow`] names it).
pub fn settings_page(kind: Kind) -> Option<&'static str> {
    platform::settings_page(kind)
}

/// How often a piece reads its feed.
const POLL: std::time::Duration = std::time::Duration::from_millis(33);
