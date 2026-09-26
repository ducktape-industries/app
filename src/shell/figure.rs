//! The launcher's one drawing: a small shape in characters that answers
//! the pointer. An orthographic raymarch of a signed-distance field, shaded
//! into a ramp of glyphs, drawn live from a [`Pose`]: the turn and tilt the
//! pointer gave it, and a light that leans toward the cursor.

use std::f32::consts::{PI, TAU};

/// Dark to light, in twelve steps: enough that a lit curve reads as a
/// gradient rather than as bands of one glyph.
pub(super) const RAMP: [char; 12] = [' ', '.', '·', ':', ';', '-', '=', '+', '*', '#', '%', '@'];
pub(super) const COLS: usize = 46;
pub(super) const ROWS: usize = 30;
/// The glyph box the frames are drawn for: the mono face's advance at
/// `SIZE`, and a line exactly `SIZE` tall. Rays step y by the same physical
/// distance as x, so a sphere stays round.
pub(super) const SIZE: f32 = 11.;
/// The canvas sets 11px with `letter-spacing: 0.5px`: a 7.1px cell. gpui
/// has no letter spacing, so the glyphs are drawn a touch larger instead
/// (`GLYPH`, whose advance is the cell).
pub(super) const ADVANCE: f32 = 7.1;
pub(super) const GLYPH: f32 = ADVANCE / 0.6;
/// Light from the upper left, in front; the eye looks down +z.
pub(super) const LIGHT: [f32; 3] = [-0.55, -0.6, -0.58];

/// How the figure is held: turned by `yaw` about the vertical, tipped by
/// `pitch` toward the eye, lit from `light` (toward the light, any length).
#[derive(Clone, Copy, Debug)]
pub(super) struct Pose {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) light: [f32; 3],
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            yaw: 0.,
            pitch: 0.,
            light: LIGHT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Figure {
    /// A globe: a node.
    Node,
    /// A ring, wobbling: a key.
    Ring,
    /// A written card: the recovery phrase on paper.
    Sheets,
    /// A sphere and its small moon: you, and the network you joined.
    Pair,
}

impl Figure {
    /// Brightness per cell, row by row, `0..=1`; negative where the ray
    /// missed. `t` (seconds) drives the figure's own motion (the ring's
    /// wobble, the moon's orbit); the pose, the rest.
    pub(super) fn shade(self, t: f32, pose: Pose) -> Vec<f32> {
        let hold = move |p: [f32; 3]| rot(p, pose.pitch, pose.yaw, 0.);
        let phase = TAU * t / 6.;
        match self {
            // meridians carved into the sphere make its turning visible
            Figure::Node => raymarch(
                |p| sphere(p, [0., 0., 0.], 1.),
                move |p| {
                    let q = rot(hold(p), 0.35, 0., 0.);
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
                pose.light,
            ),
            Figure::Ring => raymarch(
                move |p| {
                    torus(
                        rot(hold(p), 1.05 + 0.25 * phase.sin(), 0., 0.35),
                        0.78,
                        0.28,
                    )
                },
                |_| 1.,
                pose.light,
            ),
            Figure::Sheets => raymarch(
                move |p| rounded_box(rot(hold(p), 0.2, 0., 0.12), [0.62, 0.84, 0.025]),
                // lines of writing across the card
                move |p| match (rot(hold(p), 0.2, 0., 0.12)[1] * 5.5).rem_euclid(1.) < 0.4 {
                    true => 0.55,
                    false => 1.,
                },
                pose.light,
            ),
            Figure::Pair => {
                let (c, s) = (phase.cos(), phase.sin());
                raymarch(
                    move |p| {
                        let q = hold(p);
                        sphere(q, [-0.15, 0.1, 0.], 0.82).min(sphere(
                            q,
                            [-0.15 + c, 0.1 - 0.3 * c, s],
                            0.2,
                        ))
                    },
                    |_| 1.,
                    pose.light,
                )
            }
        }
    }

    /// The figure at rest, as lines of glyphs: what a still drawing shows.
    #[cfg(test)]
    pub(super) fn still(self, t: f32, pose: Pose) -> Vec<String> {
        self.shade(t, pose)
            .chunks(COLS)
            .map(|row| {
                let line: String = row.iter().map(|&lum| glyph(lum)).collect();
                line.trim_end().to_owned()
            })
            .collect()
    }
}

/// A cell's glyph for a brightness: a hit is never blank, so the shape
/// keeps its outline.
#[cfg(test)]
fn glyph(lum: f32) -> char {
    match lum < 0. {
        true => ' ',
        false => RAMP[level(lum).clamp(1, RAMP.len() - 1)],
    }
}

#[cfg(test)]
fn level(lum: f32) -> usize {
    (lum.clamp(0., 1.) * (RAMP.len() - 1) as f32).round() as usize
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

/// Everything a figure draws fits inside this radius: a ray that passes
/// wider misses without marching.
const REACH: f32 = 1.4;

/// One frame: rays straight into the scene, stepped by the distance
/// field. A hit is shaded by ambient, diffuse, a small highlight
/// and a rim along the silhouette, times the surface's own `tone`, then
/// put through a gentle contrast curve.
fn raymarch(
    sdf: impl Fn([f32; 3]) -> f32,
    tone: impl Fn([f32; 3]) -> f32,
    light: [f32; 3],
) -> Vec<f32> {
    let span = 1.25;
    let ux = span / (COLS as f32 / 2.);
    let uy = ux * SIZE / ADVANCE;
    let n = length(light[0], light[1], light[2]).max(f32::EPSILON);
    let light = light.map(|l| l / n);
    // halfway between the light and the eye (0, 0, -1)
    let half = {
        let h = [light[0], light[1], light[2] - 1.];
        let n = length(h[0], h[1], h[2]).max(f32::EPSILON);
        h.map(|v| v / n)
    };
    // one ray: the lit brightness where it lands, or None past the figure
    let ray = |x: f32, y: f32| -> Option<f32> {
        let wide = REACH * REACH - x * x - y * y;
        if wide <= 0. {
            return None;
        }
        let far = wide.sqrt();
        let mut z = -far;
        let mut hit = false;
        for _ in 0..64 {
            let d = sdf([x, y, z]);
            if d < 0.002 {
                hit = true;
                break;
            }
            z += d;
            if z > far {
                break;
            }
        }
        if !hit {
            return None;
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
        // facing away from the eye (0, 0, -1): the silhouette
        let rim = (1. + normal[2]).clamp(0., 1.).powi(3);
        Some((0.1 + 0.72 * diffuse + 0.35 * shine + 0.22 * rim) * tone([x, y, z]))
    };
    // four rays a cell (2 × 2): an edge cell half covered reads half lit
    let mut out = Vec::with_capacity(COLS * ROWS);
    for j in 0..ROWS {
        for i in 0..COLS {
            let x = (i as f32 - COLS as f32 / 2. + 0.5) * ux;
            let y = (j as f32 - ROWS as f32 / 2. + 0.5) * uy;
            let (mut sum, mut hits) = (0., 0);
            for (sx, sy) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] {
                if let Some(lit) = ray(x + sx * ux, y + sy * uy) {
                    sum += lit;
                    hits += 1;
                }
            }
            out.push(match hits {
                0 => -1.,
                _ => (sum / 4.).clamp(0., 1.).powf(1.15),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Figure; 4] = [Figure::Node, Figure::Ring, Figure::Sheets, Figure::Pair];

    /// `cargo test dump_frames -- --ignored --nocapture` prints frames to
    /// look at, and what one costs.
    #[test]
    #[ignore]
    fn dump_frames() {
        for figure in ALL {
            let pose = Pose {
                yaw: 0.6,
                ..Pose::default()
            };
            let start = std::time::Instant::now();
            let lines = figure.still(1., pose);
            println!("== {figure:?} {:?}", start.elapsed());
            for line in lines {
                println!("{line}");
            }
        }
    }

    #[test]
    fn every_figure_draws_a_shape_that_fits_its_box_and_answers_the_pose() {
        for figure in ALL {
            for t in [0., 2., 7.5] {
                let frame = figure.still(t, Pose::default());
                assert_eq!(frame.len(), ROWS);
                assert!(frame.iter().all(|line| line.chars().count() <= COLS));
                let inked = frame
                    .iter()
                    .flat_map(|line| line.chars())
                    .filter(|c| *c != ' ')
                    .count();
                assert!(
                    inked > COLS * ROWS / 10,
                    "{figure:?} at {t}s is nearly empty"
                );
            }
            let turned = Pose {
                yaw: 0.9,
                pitch: 0.4,
                ..Pose::default()
            };
            assert_ne!(
                figure.still(0., Pose::default()),
                figure.still(0., turned),
                "{figure:?} ignores its pose"
            );
            let lit_right = Pose {
                light: [0.8, -0.3, -0.6],
                ..Pose::default()
            };
            assert_ne!(
                figure.still(0., Pose::default()),
                figure.still(0., lit_right),
                "{figure:?} ignores its light"
            );
        }
    }
}
