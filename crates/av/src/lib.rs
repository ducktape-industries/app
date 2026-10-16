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
//!   OS, picked below as `platform`: `v4l2` on Linux, `mac` on macOS. A
//!   platform module is the whole OS seam: `cameras()`, `open(id)` (a
//!   [`backend::Source`]), `audio_kind()` (what a listed microphone or
//!   speaker is), `microphone_allowed()`, and what a person is told
//!   of a refused device (`allow`, `settings_page`). Another OS (iOS, its
//!   AVFoundation as macOS's where the API is the same; Android) is another
//!   module and one more line below; nothing that calls this crate moves.
//!   Each piece runs its device on a thread of its own that leaves
//!   pictures, the level and its status in a feed; the latest picture
//!   wins. A camera that stops delivering is told gone within
//!   [`backend::QUIET`] and released.
//! - the gpui side reads its feed on a 30 Hz timer while it lives and
//!   redraws only on news: a new picture, a level that moved, a new state.
//! - a piece's device is released when the piece is dropped: its thread
//!   sees the feed's stop and closes the device (the camera's light off).
//!
//! Without feature `capture` the crate is empty (`Cargo.toml` says why).
#![cfg(feature = "capture")]

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!(
    "ducktape-av captures cameras on Linux (V4L2) and macOS (AVFoundation) only: build the app without `av` here"
);

mod backend;
mod camera;
#[cfg(target_os = "macos")]
mod mac;
mod meter;
#[cfg(target_os = "linux")]
mod v4l2;

#[cfg(target_os = "macos")]
use mac as platform;
#[cfg(target_os = "linux")]
use v4l2 as platform;

pub use backend::{AudioDevice, AudioKind, CameraDevice, Device, Facing, Playback, Status};
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

/// The system's own privacy settings page that allows `kind` for this
/// app, as a link the system opens, where there is one (macOS's, which
/// [`allow`] names).
pub fn settings_page(kind: Kind) -> Option<&'static str> {
    platform::settings_page(kind)
}

/// How often a piece reads its feed.
const POLL: std::time::Duration = std::time::Duration::from_millis(33);
