//! The devices a proof runs on where there are none (`DUCKTAPE_AV_FAKE=1`:
//! a headless box with no camera and no sound card): a camera of moving
//! bars with the frame's number burnt in as a strip of blocks a decoder's
//! output can be read back from ([`stamp`], [`read_stamp`]: big blocks,
//! so the codec keeps them), and a microphone of a 440 Hz tone. What the
//! speaker would play goes to `<DUCKTAPE_AV_FAKE_TAP>/audio.f32` (raw
//! little-endian f32 at 48 kHz) when that names a directory, so a proof
//! finds the tone on the far side; the host writes the remote pictures'
//! stamps beside it.
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{OnceLock, Weak};
use std::time::{Duration, Instant};

use crate::backend::{Got, Picture, Source, Status};
use crate::codec::{FRAME, SAMPLE_RATE};
use crate::voice::{self, Io};

/// Whether the fake devices stand in for the system's.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("DUCKTAPE_AV_FAKE").is_some_and(|v| v == "1"))
}

/// Where the fake's taps go, if asked.
pub fn tap_dir() -> Option<PathBuf> {
    std::env::var_os("DUCKTAPE_AV_FAKE_TAP").map(PathBuf::from)
}

/// The fake camera's picture size and rate.
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 360;
const FPS: u64 = 15;
/// The stamp: one block per bit, top left; the first two blocks are a
/// white then a black mark, then the number's 24 bits, high first.
const BLOCK: u32 = 16;
const BITS: u32 = 24;
pub const TONE_HZ: f32 = 440.;

/// The frame's number `n` burnt into `picture` (BGRA) as the strip.
pub fn stamp(picture: &mut Picture, n: u32) {
    for block in 0..BITS + 2 {
        let on = match block {
            0 => true,
            1 => false,
            bit => n >> (BITS - 1 - (bit - 2)) & 1 == 1,
        };
        let value = if on { 255 } else { 0 };
        for y in 0..BLOCK.min(picture.height) {
            for x in block * BLOCK..((block + 1) * BLOCK).min(picture.width) {
                let at = ((y * picture.width + x) * 4) as usize;
                picture.bgra[at..at + 3].fill(value);
            }
        }
    }
}

/// The number [`stamp`] burnt into `picture`, if its marks are there.
pub fn read_stamp(picture: &Picture) -> Option<u32> {
    if picture.width < (BITS + 2) * BLOCK || picture.height < BLOCK {
        return None;
    }
    let lit = |block: u32| {
        let (x, y) = (block * BLOCK + BLOCK / 2, BLOCK / 2);
        let at = ((y * picture.width + x) * 4) as usize;
        picture.bgra[at + 1] >= 128
    };
    if !lit(0) || lit(1) {
        return None;
    }
    Some((0..BITS).fold(0, |n, bit| n << 1 | u32::from(lit(bit + 2))))
}

/// The fake camera: bars that move, the frame counted in the stamp.
pub(crate) struct Bars {
    started: Instant,
    frames: u64,
}

impl Bars {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            frames: 0,
        }
    }
}

impl Source for Bars {
    fn next(&mut self, wait: Duration, wanted: bool, _mirror: bool) -> Result<Got, Status> {
        let due = self.started + Duration::from_millis(1000 * self.frames / FPS);
        let now = Instant::now();
        if due > now {
            let left = due - now;
            if left > wait {
                std::thread::sleep(wait);
                return Ok(Got::Nothing);
            }
            std::thread::sleep(left);
        }
        let n = self.frames as u32;
        self.frames += 1;
        if !wanted {
            return Ok(Got::Frame);
        }
        let mut bgra = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                // eight bars, scrolling one pixel a frame
                let bar = ((x + n) / (WIDTH / 8)) % 8;
                let lit = |bit: u32| if bar & bit != 0 { 220u32 } else { 30 };
                // darker towards the bottom
                let shade = 255 - y * 60 / HEIGHT;
                let shaded = |value: u32| (value * shade / 255) as u8;
                bgra.extend_from_slice(&[shaded(lit(1)), shaded(lit(2)), shaded(lit(4)), 255]);
            }
        }
        let mut picture = Picture {
            width: WIDTH,
            height: HEIGHT,
            bgra,
        };
        stamp(&mut picture, n);
        Ok(Got::Picture(picture))
    }
}

/// One 20 ms frame of the tone, `n` frames in, at -12 dBFS.
pub fn tone(n: u64) -> [f32; FRAME] {
    let amplitude = 10f32.powf(-12. / 20.);
    let mut frame = [0f32; FRAME];
    for (at, sample) in frame.iter_mut().enumerate() {
        let t = (n as usize * FRAME + at) as f32 / SAMPLE_RATE as f32;
        *sample = amplitude * (std::f32::consts::TAU * TONE_HZ * t).sin();
    }
    frame
}

/// The fake voice I/O: the tone to every capture, the mixer's frames to
/// the tap, every 20 ms, until the last holder lets go.
pub(crate) fn run_voice(io: Weak<Io>) {
    let mut tap = tap_dir().and_then(|dir| std::fs::File::create(dir.join("audio.f32")).ok());
    let started = Instant::now();
    let mut n = 0u64;
    let mut out = [0f32; FRAME];
    loop {
        let Some(io) = io.upgrade() else { break };
        if io.stopped() {
            break;
        }
        if io.capturing() {
            io.set_status(Status::Live, "Fake microphone");
            io.heard(tone(n));
        }
        if io.mixer().any() {
            io.mixer().pull(&mut out);
            if let Some(tap) = &mut tap {
                let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
                let _ = tap.write_all(&bytes);
            }
        }
        drop(io);
        n += 1;
        let due = voice::frame_due(started, n);
        if let Some(left) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A number burnt in is read back; a picture without the marks reads
    /// nothing.
    #[test]
    fn a_stamp_reads_back() {
        let mut picture = Picture {
            width: WIDTH,
            height: HEIGHT,
            bgra: vec![90; (WIDTH * HEIGHT * 4) as usize],
        };
        assert_eq!(read_stamp(&picture), None);
        for n in [0, 1, 12_345_678, (1 << BITS) - 1] {
            stamp(&mut picture, n);
            assert_eq!(read_stamp(&picture), Some(n));
        }
    }

    /// The fake camera's frames are stamped in order and come at its rate.
    #[test]
    fn the_bars_count_their_frames() {
        let mut bars = Bars::new();
        let mut seen = Vec::new();
        let started = Instant::now();
        while seen.len() < 3 {
            match bars.next(Duration::from_millis(100), true, false).unwrap() {
                Got::Picture(picture) => seen.push(read_stamp(&picture).unwrap()),
                _ => {}
            }
        }
        assert_eq!(seen, vec![0, 1, 2]);
        assert!(started.elapsed() >= Duration::from_millis(1000 * 2 / FPS));
    }

    /// The tone is 440 Hz: a -12 dBFS sine is -15 dBFS RMS (level 0.75),
    /// and it crosses zero 17 or 18 times in 20 ms.
    #[test]
    fn the_tone_is_a4() {
        let frame = tone(3);
        let level = crate::mixer::level(&frame);
        assert!((level - 0.75).abs() < 0.01, "{level}");
        let crossings = frame
            .windows(2)
            .filter(|pair| (pair[0] >= 0.) != (pair[1] >= 0.))
            .count();
        assert!((17..=18).contains(&crossings), "{crossings}");
    }
}
