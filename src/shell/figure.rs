//! The app's drawings: small shapes in characters, tumbling. Each is a
//! cloud of points on its surface, sampled once; a frame turns them,
//! projects them onto the grid of cells, keeps the nearest in each, and
//! shades it by how its surface faces the light (donut.c's way). A frame
//! is some forty thousand points turned: well under a millisecond.

use std::f32::consts::TAU;
use std::sync::OnceLock;

pub(super) const RAMP: [char; 12] = [' ', '.', '·', ':', ';', '-', '=', '+', '*', '#', '%', '@'];
pub(super) const COLS: usize = 69;
pub(super) const ROWS: usize = 45;
pub(super) const SIZE: f32 = 11. / 1.5;
pub(super) const ADVANCE: f32 = 7.1 / 1.5;
pub(super) const GLYPH: f32 = ADVANCE / 0.6;
pub(super) const FPS: u64 = 30;
const UPPER_LEFT_FRONT_LIGHT: [f32; 3] = [-0.55, -0.6, -0.58];
const SURFACE_SAMPLE_SPACING: f32 = 0.016;
const GRID_HALF_WIDTH: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Figure {
    Roll,
    Ring,
    Card,
    Pair,
}

#[derive(Clone, Copy)]
struct Dot {
    at: [f32; 3],
    normal: [f32; 3],
    tone: f32,
    gloss: f32,
    moon: bool,
}

pub(super) const TUMBLE: [f32; 3] = [0.61, 0.93, 0.29];

impl Figure {
    fn dots(self) -> &'static [Dot] {
        static DOTS: [OnceLock<Vec<Dot>>; 4] = [const { OnceLock::new() }; 4];
        DOTS[self as usize].get_or_init(|| match self {
            Figure::Roll => roll(),
            Figure::Ring => torus(0.78, 0.28),
            Figure::Card => card(),
            Figure::Pair => {
                let mut dots = sphere([0., 0., 0.], 0.7, false);
                dots.extend(sphere([0., 0., 0.], 0.16, true));
                dots
            }
        })
    }

    pub(super) fn frame(self, turn: Rotation, t: f32) -> Vec<u8> {
        let orbit = TAU * t / 5.;
        let moon = rotation(0.5, 0., 0.).apply([0.95 * orbit.cos(), 0., 0.95 * orbit.sin()]);
        let light = unit(UPPER_LEFT_FRONT_LIGHT);
        let half = unit([light[0], light[1], light[2] - 1.]);
        let ux = GRID_HALF_WIDTH / (COLS as f32 / 2.);
        let uy = ux * SIZE / ADVANCE;
        let dots = self.dots();
        let mut depth = [f32::INFINITY; COLS * ROWS];
        let mut nearest = [u32::MAX; COLS * ROWS];
        for (i, dot) in dots.iter().enumerate() {
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
            nearest[cell] = i as u32;
        }
        let mut cells = [0u8; COLS * ROWS];
        for (cell, &i) in nearest.iter().enumerate() {
            let Some(dot) = dots.get(i as usize) else {
                continue;
            };
            let mut normal = match dot.moon {
                true => dot.normal,
                false => turn.apply(dot.normal),
            };
            if normal[2] > 0. {
                normal = normal.map(|v| -v);
            }
            let diffuse = dot3(normal, light).max(0.);
            let shine = dot3(normal, half).max(0.).powi(24);
            let rim = (1. + normal[2]).clamp(0., 1.).powi(3);
            let lum = ((0.18 + 0.68 * diffuse + dot.gloss * shine + 0.22 * rim) * dot.tone)
                .clamp(0., 1.)
                .powf(1.15);
            cells[cell] = ((lum * (RAMP.len() - 1) as f32).round() as u8).clamp(1, 11);
        }
        cells.to_vec()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rotation([[f32; 3]; 3]);

pub(super) fn start() -> Rotation {
    rotation(0.4, 0.7, 0.25)
}

pub(super) fn rotation(ax: f32, ay: f32, az: f32) -> Rotation {
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

    pub(super) fn then(self, step: Rotation) -> Rotation {
        let [x, y, _] = mul(step.0, self.0);
        let x = unit(x);
        let y = unit(add(y, x.map(|v| -v * dot3(x, y))));
        let z = [
            x[1] * y[2] - x[2] * y[1],
            x[2] * y[0] - x[0] * y[2],
            x[0] * y[1] - x[1] * y[0],
        ];
        Rotation([x, y, z])
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

fn count(length: f32) -> usize {
    (length / SURFACE_SAMPLE_SPACING).ceil().max(1.) as usize
}

fn across(from: f32, to: f32, n: usize) -> impl Iterator<Item = f32> {
    (0..n).map(move |i| from + (to - from) * (i as f32 + 0.5) / n as f32)
}

fn dot(at: [f32; 3], normal: [f32; 3], tone: f32) -> Dot {
    Dot {
        at,
        normal,
        tone,
        gloss: 0.35,
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
                gloss: 0.35,
                moon,
            });
        }
    }
    dots
}

fn roll() -> Vec<Dot> {
    let (outer, inner, core, half, bevel) = (0.72, 0.42, 0.36, 0.24, 0.05);
    let tape = |at, normal, tone| Dot {
        gloss: 0.6,
        ..dot(at, normal, tone)
    };
    let card = |at, normal| Dot {
        gloss: 0.05,
        ..dot(at, normal, 0.5)
    };
    let mut dots = Vec::new();
    for u in across(0., TAU, count(TAU * outer)) {
        let (c, s) = (u.cos(), u.sin());
        for y in across(-half + bevel, half - bevel, count(2. * (half - bevel))) {
            dots.push(tape([outer * c, y, outer * s], [c, 0., s], 1.));
        }
        for a in across(0., TAU / 4., 6) {
            for side in [1., -1.] {
                let normal = [a.cos() * c, side * a.sin(), a.cos() * s];
                let r = outer - bevel + bevel * a.cos();
                let y = side * (half - bevel + bevel * a.sin());
                dots.push(tape([r * c, y, r * s], normal, 1.));
            }
        }
        for r in across(inner, outer - bevel, count(outer - bevel - inner) * 2) {
            let band = match (r * 6.).fract() < 0.5 {
                true => 0.82,
                false => 1.,
            };
            dots.push(tape([r * c, half, r * s], [0., 1., 0.], band));
            dots.push(tape([r * c, -half, r * s], [0., -1., 0.], band));
        }
        for r in across(core, inner, count(inner - core) * 2) {
            dots.push(card([r * c, half, r * s], [0., 1., 0.]));
            dots.push(card([r * c, -half, r * s], [0., -1., 0.]));
        }
        for y in across(-half, half, count(2. * half)) {
            dots.push(card([core * c, y, core * s], [-c, 0., -s]));
        }
    }
    let curl = 0.35;
    for x in across(0., 0.55, count(0.6)) {
        let normal = unit([-2. * curl * x, 0., 1.]);
        for y in across(-half + bevel, half - bevel, count(2. * half)) {
            dots.push(tape([x, y, outer + curl * x * x], normal, 1.));
        }
    }
    dots
}

fn card() -> Vec<Dot> {
    let (w, h, thick) = (0.62, 0.84, 0.03);
    let mut dots = Vec::new();
    for x in across(-w, w, count(2. * w)) {
        for y in across(-h, h, count(2. * h)) {
            let tone = match (y * 5.5).rem_euclid(1.) < 0.4 {
                true => 0.55,
                false => 1.,
            };
            dots.push(dot([x, y, -thick], [0., 0., -1.], tone));
            dots.push(dot([x, y, thick], [0., 0., 1.], tone));
        }
        for side in [1., -1.] {
            for z in across(-thick, thick, 3) {
                dots.push(dot([x, side * h, z], [0., side, 0.], 0.8));
            }
        }
    }
    for y in across(-h, h, count(2. * h)) {
        for side in [1., -1.] {
            for z in across(-thick, thick, 3) {
                dots.push(dot([side * w, y, z], [side, 0., 0.], 0.8));
            }
        }
    }
    dots
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Figure; 4] = [Figure::Roll, Figure::Ring, Figure::Card, Figure::Pair];

    fn inked(cells: &[u8]) -> usize {
        cells.iter().filter(|&&step| step > 0).count()
    }

    #[test]
    #[ignore]
    fn show_frames() {
        for figure in ALL {
            let clock = std::time::Instant::now();
            let cells = figure.frame(start(), 2.);
            println!("== {figure:?} {:?}", clock.elapsed());
            for row in cells.chunks(COLS) {
                let line: String = row.iter().map(|&step| RAMP[step as usize]).collect();
                println!("{}", line.trim_end());
            }
        }
    }

    #[test]
    fn every_figure_tumbles_inside_its_box() {
        for figure in ALL {
            let turn = |t: f32| start().then(rotation(TUMBLE[0] * t, TUMBLE[1] * t, TUMBLE[2] * t));
            let mut last = figure.frame(turn(0.), 0.);
            for n in 1..90 {
                let cells = figure.frame(turn(n as f32 / 3.), n as f32 / 3.);
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

    #[test]
    fn the_ring_is_solid_where_it_is_seen() {
        let cells = Figure::Ring.frame(start(), 1.);
        for row in cells.chunks(COLS) {
            let inked: Vec<usize> = (0..COLS).filter(|&col| row[col] > 0).collect();
            if let (Some(&first), Some(&last)) = (inked.first(), inked.last()) {
                let runs = (first..=last)
                    .filter(|&col| row[col] > 0 && (col == first || row[col - 1] == 0))
                    .count();
                assert!(runs <= 2, "a hole in {row:?}");
            }
        }
    }

    #[test]
    fn many_small_turns_stay_a_rotation() {
        let mut turn = start();
        for _ in 0..100_000 {
            turn = turn.then(rotation(0.021, 0.031, 0.009));
        }
        let rows = turn.0;
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1. } else { 0. };
                assert!((dot3(rows[i], rows[j]) - want).abs() < 1e-4, "{rows:?}");
            }
        }
    }

    #[test]
    fn a_frame_is_cheap() {
        for figure in ALL {
            figure.frame(start(), 0.);
            let clock = std::time::Instant::now();
            for n in 0..10 {
                figure.frame(start(), n as f32);
            }
            let each = clock.elapsed() / 10;
            assert!(
                each < std::time::Duration::from_millis(15),
                "{figure:?} took {each:?}"
            );
        }
    }
}
