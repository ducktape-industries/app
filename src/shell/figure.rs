//! The launcher's one drawing: a small shape in characters, turning slowly.
//! An orthographic raymarch of a signed-distance field, shaded into a ramp
//! of glyphs, one frame per step of a short loop. Built once per shape and
//! kept; a frame is picked by the clock at draw time.

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

/// Dark to light, in twelve steps: enough that a lit curve reads as a
/// gradient rather than as bands of one glyph.
const RAMP: [char; 12] = [' ', '.', '·', ':', ';', '-', '=', '+', '*', '#', '%', '@'];
pub(super) const COLS: usize = 46;
pub(super) const ROWS: usize = 30;
/// The glyph box the frames are drawn for: the mono face's advance at
/// `SIZE`, and a line exactly `SIZE` tall. Rays step y by the same physical
/// distance as x, so a sphere stays round.
pub(super) const SIZE: f32 = 11.;
pub(super) const ADVANCE: f32 = 6.6;
/// Enough frames that a step is under a character's width of movement.
const FRAMES: usize = 96;
/// One loop, start to start.
pub(super) const LOOP_MS: u64 = 6000;
/// How often the drawing is redrawn: one loop frame each time.
pub(super) const FPS: f32 = FRAMES as f32 * 1000. / LOOP_MS as f32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Figure {
    /// A globe turning under a still light: a node.
    Node,
    /// A turning ring: a key.
    Ring,
    /// A written card, turning: the recovery phrase on paper.
    Sheets,
    /// A sphere and its small moon: you, and the network you joined.
    Pair,
}

impl Figure {
    /// The frame to show `elapsed_ms` into the loop. Each frame is drawn
    /// the first time it is asked for, then kept.
    pub(super) fn frame(self, elapsed_ms: u64) -> &'static [String] {
        static CACHE: [[OnceLock<Vec<String>>; FRAMES]; 4] =
            [const { [const { OnceLock::new() }; FRAMES] }; 4];
        let nth = (elapsed_ms % LOOP_MS) as usize * FRAMES / LOOP_MS as usize;
        CACHE[self as usize][nth].get_or_init(|| self.draw(nth as f32 / FRAMES as f32))
    }

    fn draw(self, t: f32) -> Vec<String> {
        let turn = TAU * t;
        match self {
            // meridians carved into the sphere make its turning visible
            Figure::Node => raymarch(
                |p| sphere(p, [0., 0., 0.], 1.),
                move |p| {
                    let q = rot(p, 0.35, turn, 0.);
                    let longitude = q[2].atan2(q[0]);
                    let latitude = q[1].clamp(-1., 1.).asin();
                    // how near a meridian (every 30°) or a parallel (every 36°)
                    let near = |angle: f32, every: f32| {
                        let off = (angle / every).fract().abs();
                        off.min(1. - off) * every
                    };
                    let line = near(longitude + PI, PI / 6.).min(near(latitude + PI, PI / 5.));
                    1. - 0.5 * (-(line * line) / 0.004).exp()
                },
            ),
            Figure::Ring => raymarch(
                |p| {
                    torus(
                        rot(p, 1.05 + 0.25 * turn.sin(), turn / 2., 0.35),
                        0.78,
                        0.28,
                    )
                },
                |_| 1.,
            ),
            Figure::Sheets => raymarch(
                move |p| rounded_box(rot(p, 0.2, 0.7 * turn.sin(), 0.12), [0.62, 0.84, 0.025]),
                // lines of writing across the card
                |p| match (p[1] * 5.5).rem_euclid(1.) < 0.4 {
                    true => 0.55,
                    false => 1.,
                },
            ),
            Figure::Pair => {
                let (c, s) = (turn.cos(), turn.sin());
                raymarch(
                    move |p| {
                        sphere(p, [-0.15, 0.1, 0.], 0.82).min(sphere(
                            p,
                            [-0.15 + c, 0.1 - 0.3 * c, s],
                            0.2,
                        ))
                    },
                    |_| 1.,
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

/// Light from the upper left, in front; the eye looks down +z.
const LIGHT: [f32; 3] = [-0.55, -0.6, -0.58];

/// One frame: a ray per cell straight into the scene, stepped by the
/// distance field. A hit is shaded by ambient, diffuse and a small
/// highlight, times the surface's own `tone` there.
fn raymarch(sdf: impl Fn([f32; 3]) -> f32, tone: impl Fn([f32; 3]) -> f32) -> Vec<String> {
    let span = 1.25;
    let ux = span / (COLS as f32 / 2.);
    let uy = ux * SIZE / ADVANCE;
    let n = length(LIGHT[0], LIGHT[1], LIGHT[2]);
    let light = LIGHT.map(|l| l / n);
    // halfway between the light and the eye (0, 0, -1)
    let half = {
        let h = [light[0], light[1], light[2] - 1.];
        let n = length(h[0], h[1], h[2]);
        h.map(|v| v / n)
    };
    (0..ROWS)
        .map(|j| {
            let line: String = (0..COLS)
                .map(|i| {
                    let x = (i as f32 - COLS as f32 / 2. + 0.5) * ux;
                    let y = (j as f32 - ROWS as f32 / 2. + 0.5) * uy;
                    let mut z = -3.;
                    let mut hit = false;
                    for _ in 0..80 {
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
                    let normal = [
                        sdf([x + e, y, z]) - sdf([x - e, y, z]),
                        sdf([x, y + e, z]) - sdf([x, y - e, z]),
                        sdf([x, y, z + e]) - sdf([x, y, z - e]),
                    ];
                    let norm = length(normal[0], normal[1], normal[2]).max(f32::EPSILON);
                    let normal = normal.map(|v| v / norm);
                    let dot = |v: [f32; 3]| normal[0] * v[0] + normal[1] * v[1] + normal[2] * v[2];
                    let diffuse = dot(light).max(0.);
                    let shine = dot(half).max(0.).powf(24.);
                    let lit = (0.12 + 0.7 * diffuse + 0.35 * shine) * tone([x, y, z]);
                    let steps = (RAMP.len() - 1) as f32;
                    let level = lit.clamp(0., 1.) * steps;
                    // a hit is never blank: the shape keeps its outline
                    RAMP[(level.round() as usize).clamp(1, RAMP.len() - 1)]
                })
                .collect();
            line.trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test dump_frames -- --ignored --nocapture` prints frames to
    /// look at.
    #[test]
    #[ignore]
    fn dump_frames() {
        for figure in [Figure::Node, Figure::Ring, Figure::Sheets, Figure::Pair] {
            for nth in 0..4 {
                println!("== {figure:?} {nth}");
                for line in figure.frame(nth * LOOP_MS / 16) {
                    println!("{line}");
                }
            }
        }
    }

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
        for figure in [Figure::Node, Figure::Ring, Figure::Sheets, Figure::Pair] {
            assert_ne!(
                figure.frame(0),
                figure.frame(LOOP_MS / 4),
                "{figure:?} is still"
            );
        }
    }
}
