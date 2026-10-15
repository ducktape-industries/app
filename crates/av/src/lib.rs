//! The camera and the microphone in a gpui window: a [`Camera`] draws a
//! camera's pictures (the whole picture, centred, mirrored if asked), a
//! [`Meter`] draws a microphone's level and records and plays back, and
//! [`cameras`] and [`audio_devices`] list what can be opened, and
//! [`play_chime`] tests a speaker. Each piece emits
//! its device's [`State`] as a gpui event. Nothing here knows of views or
//! the wire.
//!
//! Shape:
//! - every device call is in [`backend`] (cpal; the camera's stream loop)
//!   over our own camera capture, `v4l2` on Linux and `mac` on macOS: the
//!   seam another backend replaces. Each piece runs its device on a
//!   thread of its own that leaves pictures, the level and its status in
//!   a feed; the latest picture wins. A camera that stops delivering is
//!   told gone within [`backend::QUIET`] and released.
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

pub use backend::{Device, Playback, Status};
pub use camera::{Camera, Fit};
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
pub fn cameras() -> Result<Vec<Device>, String> {
    backend::cameras()
}

/// The microphones and the speakers the system lists now.
pub fn audio_devices() -> Result<(Vec<Device>, Vec<Device>), String> {
    backend::audio_devices()
}

/// Plays the test chime on `output` (the system's speaker when `None`)
/// and returns once it has sounded, about 0.7 s: the speaker test, apart
/// from any microphone. Blocks the calling thread.
pub fn play_chime(output: Option<&str>) -> Result<(), String> {
    backend::play_chime(output)
}

/// How often a piece reads its feed.
const POLL: std::time::Duration = std::time::Duration::from_millis(33);
