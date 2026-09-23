//! The launcher's one drawing: a small shape in characters, turning slowly.
//! An orthographic raymarch of a signed-distance field, shaded into a ramp
//! of glyphs, one frame per step of a short loop. Built once per shape and
//! kept; a frame is picked by the clock at draw time.

use std::f32::consts::TAU;
use std::sync::OnceLock;

const RAMP: [char; 9] = [' ', '.', '·', ':', '-', '=', '+', '*', '#'];
pub(super) const COLS: usize = 46;
pub(super) const ROWS: usize = 30;
/// The glyph box the frames are drawn for: the mono face's advance at
/// `SIZE`, and a line exactly `SIZE` tall. Rays step y by the same physical
/// distance as x, so a sphere stays round.
pub(super) const SIZE: f32 = 11.;
pub(super) const ADVANCE: f32 = 6.6;
const FRAMES: usize = 16;
/// One loop, start to start.
pub(super) const LOOP_MS: u64 = 4800;
const FRONT: [f32; 3] = [-0.55, -0.6, -0.58];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Figure {
    /// A lit sphere, its light circling: a node.
    Node,
    /// A turning ring: a key.
    Ring,
    /// Three sheets, rocking: the recovery phrase on paper.
    Sheets,
    /// A sphere and its small moon: you, and the network you joined.
    Pair,
}

impl Figure {
    /// The frame to show `elapsed_ms` into the loop.
    pub(super) fn frame(self, elapsed_ms: u64) -> &'static [String] {
        static CACHE: [OnceLock<Vec<Vec<String>>>; 4] = [const { OnceLock::new() }; 4];
        let frames = CACHE[self as usize].get_or_init(|| {
            (0..FRAMES)
                .map(|nth| self.draw(nth as f32 / FRAMES as f32))
                .collect()
        });
        let step = LOOP_MS / FRAMES as u64;
        &frames[(elapsed_ms / step) as usize % FRAMES]
    }

    fn draw(self, t: f32) -> Vec<String> {
        match self {
            Figure::Node => {
                let light = [(TAU * t).cos() * 0.8, -0.55, -0.6 + (TAU * t).sin() * 0.5];
                raymarch(|p| sphere(p, [0., 0., 0.], 1.), light)
            }
            Figure::Ring => raymarch(
                |p| torus(rot(p, 1.05, TAU * t / 2., 0.35), 0.78, 0.28),
                FRONT,
            ),
            Figure::Sheets => raymarch(
                |p| {
                    (0..3)
                        .map(|k| {
                            let k = k as f32;
                            let q = [p[0] + 0.12 * k - 0.12, p[1] + 0.16 * k - 0.16, p[2]];
                            rounded_box(
                                rot(q, 0.95, 0.25 * (TAU * t).sin(), 0.45),
                                [0.72, 0.035, 0.95],
                            )
                        })
                        .fold(f32::MAX, f32::min)
                },
                FRONT,
            ),
            Figure::Pair => {
                let (c, s) = ((TAU * t).cos(), (TAU * t).sin());
                raymarch(
                    |p| {
                        sphere(p, [-0.15, 0.1, 0.], 0.82).min(sphere(
                            p,
                            [-0.15 + c, 0.1 - 0.3 * c, s],
                            0.2,
                        ))
                    },
                    FRONT,
                )
            }
        }
    }
}

fn length(x: f32, y: f32, z: f32) -> f32 {
    (x * x + y * y + z * z).sqrt()
}

fn rot(p: [f32; 3], ax: f32, ay: f32, az: f32) -> [f32; 3] {
    let [mut x, mut y, mut z] = p;
    let (c, s) = (ax.cos(), ax.sin());
    (y, z) = (y * c - z * s, y * s + z * c);
    let (c, s) = (ay.cos(), ay.sin());
    (x, z) = (x * c + z * s, -x * s + z * c);
    let (c, s) = (az.cos(), az.sin());
    (x, y) = (x * c - y * s, x * s + y * c);
    [x, y, z]
}

fn sphere(p: [f32; 3], c: [f32; 3], r: f32) -> f32 {
    length(p[0] - c[0], p[1] - c[1], p[2] - c[2]) - r
}

fn torus(p: [f32; 3], big: f32, small: f32) -> f32 {
    let q = (p[0] * p[0] + p[2] * p[2]).sqrt() - big;
    (q * q + p[1] * p[1]).sqrt() - small
}

fn rounded_box(p: [f32; 3], b: [f32; 3]) -> f32 {
    let q = [p[0].abs() - b[0], p[1].abs() - b[1], p[2].abs() - b[2]];
    length(q[0].max(0.), q[1].max(0.), q[2].max(0.)) + q[0].max(q[1]).max(q[2]).min(0.) - 0.04
}

fn raymarch(sdf: impl Fn([f32; 3]) -> f32, light: [f32; 3]) -> Vec<String> {
    // the light stays in front of the shape, never behind it
    let light = match light[2] < 0. {
        true => light,
        false => [light[0], light[1], -0.05],
    };
    let span = 1.25;
    let ux = span / (COLS as f32 / 2.);
    let uy = ux * SIZE / ADVANCE;
    let n = length(light[0], light[1], light[2]);
    let light = light.map(|l| l / n);
    (0..ROWS)
        .map(|j| {
            let line: String = (0..COLS)
                .map(|i| {
                    let x = (i as f32 - COLS as f32 / 2. + 0.5) * ux;
                    let y = (j as f32 - ROWS as f32 / 2. + 0.5) * uy;
                    let mut z = -3.;
                    let mut hit = false;
                    for _ in 0..70 {
                        let d = sdf([x, y, z]);
                        if d < 0.002 {
                            hit = true;
                            break;
                        }
                        z += d;
                        if z > 3. {
                            break;
                        }
                    }
                    if !hit {
                        return ' ';
                    }
                    let e = 0.002;
                    let nx = sdf([x + e, y, z]) - sdf([x - e, y, z]);
                    let ny = sdf([x, y + e, z]) - sdf([x, y - e, z]);
                    let nz = sdf([x, y, z + e]) - sdf([x, y, z - e]);
                    let norm = length(nx, ny, nz).max(f32::EPSILON);
                    let lit = ((nx * light[0] + ny * light[1] + nz * light[2]) / norm).max(0.);
                    let v = 0.1 + 0.8 * lit.powf(1.2);
                    RAMP[((v * (RAMP.len() - 1) as f32 + 0.5) as usize).min(RAMP.len() - 1)]
                })
                .collect();
            line.trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_figure_draws_a_shape_that_fits_its_box() {
        for figure in [Figure::Node, Figure::Ring, Figure::Sheets, Figure::Pair] {
            for elapsed in [0, LOOP_MS / 3, LOOP_MS * 5 / 2] {
                let frame = figure.frame(elapsed);
                assert_eq!(frame.len(), ROWS);
                assert!(frame.iter().all(|line| line.chars().count() <= COLS));
                let inked = frame
                    .iter()
                    .flat_map(|line| line.chars())
                    .filter(|c| *c != ' ')
                    .count();
                assert!(
                    inked > COLS * ROWS / 10,
                    "{figure:?} at {elapsed}ms is nearly empty"
                );
            }
        }
        assert_ne!(
            Figure::Ring.frame(0),
            Figure::Ring.frame(LOOP_MS / 4),
            "the ring turns"
        );
    }
}
