//! The app's drawings: small shapes in characters, tumbling. Each is a
//! cloud of points on its surface, sampled once; a frame turns them,
//! projects them onto the grid of cells, keeps the nearest in each, and
//! shades it by how its surface faces the light (donut.c's way). A frame
//! is some ten thousand points turned: far under a millisecond.

use std::f32::consts::TAU;
use std::sync::OnceLock;

/// Dark to light, in twelve steps: a drawn cell is an index into it.
pub(super) const RAMP: [char; 12] = [' ', '.', '·', ':', ';', '-', '=', '+', '*', '#', '%', '@'];
pub(super) const COLS: usize = 46;
pub(super) const ROWS: usize = 30;
/// The glyph box: the mono face's advance at `SIZE`, and a line exactly
/// `SIZE` tall. A cell is taller than wide; the projection squares it up.
pub(super) const SIZE: f32 = 11.;
/// The canvas sets 11px with `letter-spacing: 0.5px`: a 7.1px cell. gpui
/// has no letter spacing, so the glyphs are drawn a touch larger instead
/// (`GLYPH`, whose advance is the cell).
pub(super) const ADVANCE: f32 = 7.1;
pub(super) const GLYPH: f32 = ADVANCE / 0.6;
/// Frames a second.
pub(super) const FPS: u64 = 30;
/// Toward the light: from the upper left, in front. The eye looks down +z.
const LIGHT: [f32; 3] = [-0.55, -0.6, -0.58];
/// The points' spacing on a surface: under a cell at any turn, so a
/// surface never shows holes.
const STEP: f32 = 0.024;
/// Half the grid's width, in the figures' units.
const SPAN: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Figure {
    /// A roll of duct tape, its loose end trailing: the app itself.
    Roll,
    /// A ring: a key.
    Ring,
    /// A written card: the recovery phrase on paper.
    Sheets,
    /// A sphere and its small moon: you, and the network you joined.
    Pair,
}

/// A point on a figure's surface, its outward normal, how light its
/// surface is (`0..=1`), and whether it is the moon (which orbits).
#[derive(Clone, Copy)]
struct Dot {
    at: [f32; 3],
    normal: [f32; 3],
    tone: f32,
    moon: bool,
}

impl Figure {
    fn dots(self) -> &'static [Dot] {
        static DOTS: [OnceLock<Vec<Dot>>; 4] = [const { OnceLock::new() }; 4];
        DOTS[self as usize].get_or_init(|| match self {
            Figure::Roll => roll(),
            Figure::Ring => torus(0.78, 0.28),
            Figure::Sheets => card(),
            Figure::Pair => {
                let mut dots = sphere([0., 0., 0.], 0.7, false);
                dots.extend(sphere([0., 0., 0.], 0.16, true));
                dots
            }
        })
    }

    /// The frame at `t` seconds: each cell's ramp step, row by row, 0
    /// where nothing is. It turns about three axes at speeds that never
    /// line up, so it never plays the same turn twice.
    pub(super) fn frame(self, t: f32) -> Vec<u8> {
        let turn = rotation(0.61 * t + 0.4, 0.93 * t + 0.7, 0.29 * t + 0.25);
        // the moon's orbit: round the sphere, its plane tipped
        let orbit = TAU * t / 5.;
        let moon = rotation(0.5, 0., 0.).apply([0.95 * orbit.cos(), 0., 0.95 * orbit.sin()]);
        let light = unit(LIGHT);
        let half = unit([light[0], light[1], light[2] - 1.]);
        let ux = SPAN / (COLS as f32 / 2.);
        let uy = ux * SIZE / ADVANCE;
        let mut depth = [f32::INFINITY; COLS * ROWS];
        let mut cells = [0u8; COLS * ROWS];
        for dot in self.dots() {
            let at = match dot.moon {
                true => add(dot.at, moon),
                false => turn.apply(dot.at),
            };
            let col = at[0] / ux + COLS as f32 / 2.;
            let row = at[1] / uy + ROWS as f32 / 2.;
            if col < 0. || row < 0. || col >= COLS as f32 || row >= ROWS as f32 {
                continue;
            }
            let cell = row as usize * COLS + col as usize;
            if at[2] >= depth[cell] {
                continue;
            }
            depth[cell] = at[2];
            let mut normal = match dot.moon {
                true => dot.normal,
                false => turn.apply(dot.normal),
            };
            // a thin sheet shows either face: light the one toward the eye
            if normal[2] > 0. {
                normal = normal.map(|v| -v);
            }
            let diffuse = dot3(normal, light).max(0.);
            let shine = dot3(normal, half).max(0.).powi(24);
            let rim = (1. + normal[2]).clamp(0., 1.).powi(3);
            let lum = ((0.18 + 0.68 * diffuse + 0.35 * shine + 0.22 * rim) * dot.tone)
                .clamp(0., 1.)
                .powf(1.15);
            cells[cell] = ((lum * (RAMP.len() - 1) as f32).round() as u8).clamp(1, 11);
        }
        cells.to_vec()
    }
}

/// A turn about x, then y, then z, as a matrix.
#[derive(Clone, Copy)]
struct Rotation([[f32; 3]; 3]);

fn rotation(ax: f32, ay: f32, az: f32) -> Rotation {
    let (cx, sx) = (ax.cos(), ax.sin());
    let (cy, sy) = (ay.cos(), ay.sin());
    let (cz, sz) = (az.cos(), az.sin());
    let x = [[1., 0., 0.], [0., cx, -sx], [0., sx, cx]];
    let y = [[cy, 0., sy], [0., 1., 0.], [-sy, 0., cy]];
    let z = [[cz, -sz, 0.], [sz, cz, 0.], [0., 0., 1.]];
    Rotation(mul(z, mul(y, x)))
}

impl Rotation {
    fn apply(self, v: [f32; 3]) -> [f32; 3] {
        self.0.map(|row| dot3(row, v))
    }
}

fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let n = dot3(v, v).sqrt().max(f32::EPSILON);
    v.map(|x| x / n)
}

/// How many samples cover `length` at `STEP` apart.
fn count(length: f32) -> usize {
    (length / STEP).ceil().max(1.) as usize
}

/// `n` evenly spaced values across `from..=to`.
fn across(from: f32, to: f32, n: usize) -> impl Iterator<Item = f32> {
    (0..n).map(move |i| from + (to - from) * (i as f32 + 0.5) / n as f32)
}

fn dot(at: [f32; 3], normal: [f32; 3], tone: f32) -> Dot {
    Dot {
        at,
        normal,
        tone,
        moon: false,
    }
}

fn torus(big: f32, small: f32) -> Vec<Dot> {
    let mut dots = Vec::new();
    for u in across(0., TAU, count(TAU * (big + small))) {
        for v in across(0., TAU, count(TAU * small)) {
            let normal = [v.cos() * u.cos(), v.sin(), v.cos() * u.sin()];
            let r = big + small * v.cos();
            dots.push(dot([r * u.cos(), small * v.sin(), r * u.sin()], normal, 1.));
        }
    }
    dots
}

fn sphere(centre: [f32; 3], r: f32, moon: bool) -> Vec<Dot> {
    let mut dots = Vec::new();
    for lat in across(-TAU / 4., TAU / 4., count(TAU / 2. * r)) {
        let ring = r * lat.cos();
        for lon in across(0., TAU, count(TAU * ring)) {
            let normal = [lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin()];
            dots.push(Dot {
                at: add(centre, normal.map(|v| v * r)),
                normal,
                tone: 1.,
                moon,
            });
        }
    }
    dots
}

/// A roll of tape: a thick short tube, its wound layers showing as bands
/// on its faces, a card core inside, and the loose end off its side.
fn roll() -> Vec<Dot> {
    let (outer, inner, half) = (0.72, 0.42, 0.24);
    let mut dots = Vec::new();
    for u in across(0., TAU, count(TAU * outer)) {
        let (c, s) = (u.cos(), u.sin());
        for y in across(-half, half, count(2. * half)) {
            dots.push(dot([outer * c, y, outer * s], [c, 0., s], 1.));
            // the core, facing in: card, darker than the tape
            dots.push(dot([inner * c, y, inner * s], [-c, 0., -s], 0.55));
        }
        for r in across(inner, outer, count(outer - inner) * 2) {
            let band = match (r * 6.).fract() < 0.5 {
                true => 0.8,
                false => 1.,
            };
            dots.push(dot([r * c, half, r * s], [0., 1., 0.], band));
            dots.push(dot([r * c, -half, r * s], [0., -1., 0.], band));
        }
    }
    // the loose end: a strip leaving the outer face along its tangent
    for x in across(0., 0.5, count(0.5)) {
        for y in across(-half, half, count(2. * half)) {
            dots.push(dot([x, y, outer], [0., 0., 1.], 1.));
        }
    }
    dots
}

/// A card with lines of writing across both faces.
fn card() -> Vec<Dot> {
    let (w, h) = (0.62, 0.84);
    let mut dots = Vec::new();
    for x in across(-w, w, count(2. * w)) {
        for y in across(-h, h, count(2. * h)) {
            let tone = match (y * 5.5).rem_euclid(1.) < 0.4 {
                true => 0.55,
                false => 1.,
            };
            dots.push(dot([x, y, 0.], [0., 0., -1.], tone));
        }
    }
    dots
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Figure; 4] = [Figure::Roll, Figure::Ring, Figure::Sheets, Figure::Pair];

    fn inked(cells: &[u8]) -> usize {
        cells.iter().filter(|&&step| step > 0).count()
    }

    /// `cargo test show_frames -- --ignored --nocapture` prints frames to
    /// look at, and what one costs.
    #[test]
    #[ignore]
    fn show_frames() {
        for figure in ALL {
            let start = std::time::Instant::now();
            let cells = figure.frame(2.);
            println!("== {figure:?} {:?}", start.elapsed());
            for row in cells.chunks(COLS) {
                let line: String = row.iter().map(|&step| RAMP[step as usize]).collect();
                println!("{}", line.trim_end());
            }
        }
    }

    /// Every frame is steps on the ramp, never empty and never spilling
    /// off the grid's edge, and the figure keeps moving.
    #[test]
    fn every_figure_tumbles_inside_its_box() {
        for figure in ALL {
            let mut last = figure.frame(0.);
            for n in 1..90 {
                let cells = figure.frame(n as f32 / 3.);
                assert_eq!(cells.len(), COLS * ROWS);
                assert!(cells.iter().all(|&step| (step as usize) < RAMP.len()));
                assert!(
                    inked(&cells) > COLS * ROWS / 40,
                    "{figure:?} vanished at {n}"
                );
                let edge = (0..ROWS)
                    .any(|row| cells[row * COLS] > 0 || cells[row * COLS + COLS - 1] > 0)
                    || cells[..COLS]
                        .iter()
                        .chain(&cells[(ROWS - 1) * COLS..])
                        .any(|&s| s > 0);
                assert!(!edge, "{figure:?} runs off the grid at {n}");
                assert_ne!(cells, last, "{figure:?} stood still at {n}");
                last = cells;
            }
        }
    }

    /// A closed surface shows no holes: inside its outline every cell is
    /// inked.
    #[test]
    fn the_ring_is_solid_where_it_is_seen() {
        let cells = Figure::Ring.frame(1.);
        for row in cells.chunks(COLS) {
            let inked: Vec<usize> = (0..COLS).filter(|&col| row[col] > 0).collect();
            if let (Some(&first), Some(&last)) = (inked.first(), inked.last()) {
                // a ring's row crosses its hole at most once: two runs
                let runs = (first..=last)
                    .filter(|&col| row[col] > 0 && (col == first || row[col - 1] == 0))
                    .count();
                assert!(runs <= 2, "a hole in {row:?}");
            }
        }
    }

    /// A frame costs well under the frame it fills, even unoptimised.
    #[test]
    fn a_frame_is_cheap() {
        for figure in ALL {
            figure.frame(0.);
            let start = std::time::Instant::now();
            for n in 0..10 {
                figure.frame(n as f32);
            }
            let each = start.elapsed() / 10;
            assert!(
                each < std::time::Duration::from_millis(15),
                "{figure:?} took {each:?}"
            );
        }
    }
}
