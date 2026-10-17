//! Every remote voice summed for the one speaker: a [`Source`] per
//! person (their packets through [`crate::jitter`] and the decoder), and
//! [`Mixer::pull`] the frame the speaker plays, each voice's level left
//! for its piece ("speaking"). The voice I/O ([`crate::voice`]) pulls it
//! from its playback callback, once per 20 ms.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::codec::{AudioDecoder, FRAME};
use crate::jitter::{Jitter, Pop};

/// The remote voices to mix.
#[derive(Default)]
pub struct Mixer {
    sources: Mutex<Vec<Weak<Shared>>>,
}

struct Shared {
    jitter: Mutex<Jitter>,
    decoder: Mutex<AudioDecoder>,
    /// The voice's level, 0 to 1 (-60 to 0 dBFS), as `f32` bits.
    level: AtomicU32,
}

/// One remote voice, mixed from the first [`Source::push`] until dropped.
pub struct Source(Arc<Shared>);

impl Mixer {
    /// One more voice; `Err` when this build decodes none.
    pub fn source(&self) -> Result<Source, String> {
        let shared = Arc::new(Shared {
            jitter: Mutex::new(Jitter::default()),
            decoder: Mutex::new(AudioDecoder::new()?),
            level: AtomicU32::new(0),
        });
        lock(&self.sources).push(Arc::downgrade(&shared));
        Ok(Source(shared))
    }

    /// Whether any voice is here to play.
    pub fn any(&self) -> bool {
        let mut sources = lock(&self.sources);
        sources.retain(|source| source.strong_count() > 0);
        !sources.is_empty()
    }

    /// One frame of every voice summed into `out`, soft-clipped; a voice
    /// with nothing to play adds silence. How many voices were mixed.
    pub fn pull(&self, out: &mut [f32; FRAME]) -> usize {
        out.fill(0.);
        // ponytail: decoded here, on the speaker's callback (an Opus frame
        // is tens of microseconds); the upgrade is the voice thread
        // decoding into a ring the callback only copies from
        let sources = lock(&self.sources);
        let mut mixed = 0;
        for source in sources.iter().filter_map(Weak::upgrade) {
            let popped = lock(&source.jitter).pop();
            let frame = match popped {
                Pop::Empty => {
                    source.level.store(0f32.to_bits(), Ordering::Relaxed);
                    continue;
                }
                Pop::Packet(packet) => lock(&source.decoder).decode(&packet),
                Pop::Lost { next } => lock(&source.decoder).lost(next.as_deref()),
            };
            let Ok(frame) = frame else {
                continue;
            };
            source
                .level
                .store(level(&frame).to_bits(), Ordering::Relaxed);
            mix(out, &frame);
            mixed += 1;
        }
        mixed
    }
}

impl Source {
    /// `packet` numbered `seq` by its sender.
    pub fn push(&self, seq: u64, packet: Vec<u8>) {
        lock(&self.0.jitter).push(seq, packet);
    }

    /// The voice's level now, 0 to 1 (-60 to 0 dBFS).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.0.level.load(Ordering::Relaxed))
    }
}

/// `frame` added onto `into`, the sum soft-clipped so two loud voices do
/// not crack.
pub(crate) fn mix(into: &mut [f32], frame: &[f32]) {
    for (out, sample) in into.iter_mut().zip(frame) {
        *out = soft_clip(*out + sample);
    }
}

/// Linear under half scale, bending to 1 above it.
fn soft_clip(sample: f32) -> f32 {
    sample.tanh()
}

/// A frame's level on the feed's scale: 0 at -60 dBFS, 1 at 0 dBFS.
pub(crate) fn level(frame: &[f32]) -> f32 {
    let power = frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32;
    let db = 10.0 * power.max(1e-10).log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two full-scale voices summed stay under full scale; a quiet one
    /// adds nearly linearly; silence is silence.
    #[test]
    fn the_sum_is_soft_clipped() {
        let mut out = [0f32; 4];
        mix(&mut out, &[1., -1., 0.1, 0.]);
        mix(&mut out, &[1., -1., 0.1, 0.]);
        assert!(out[0] < 1. && out[0] > 0.9);
        assert!(out[1] > -1. && out[1] < -0.9);
        assert!((out[2] - 0.2).abs() < 0.01);
        assert_eq!(out[3], 0.);
    }

    /// A full-scale frame is level 1, silence 0, -30 dBFS half.
    #[test]
    fn a_level_is_dbfs_on_the_feeds_scale() {
        assert_eq!(level(&[1., -1., 1., -1.]), 1.);
        assert_eq!(level(&[0.; 4]), 0.);
        let half = 10f32.powf(-30. / 20.);
        assert!((level(&[half, -half]) - 0.5).abs() < 0.01);
    }
}
