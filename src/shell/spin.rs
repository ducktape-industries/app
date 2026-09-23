//! The figure, held: it turns on its own, follows a drag and keeps the
//! fling when let go, leans toward the cursor, and its light leans with it.
//! Every motion is a critically damped spring, so nothing snaps; once the
//! pointer rests, the figure eases back to turning on its own.

use std::time::Instant;

use gpui_kit::*;

use super::figure::{self, Figure, Pose};

/// The turn on its own, in radians a second.
const IDLE_SPIN: f32 = 0.55;
/// A drag's turn, in radians a pixel.
const DRAG: f32 = 0.011;
/// How far the cursor leans the figure (radians) and its light, at most.
const LEAN_YAW: f32 = 0.22;
const LEAN_PITCH: f32 = 0.16;
const LEAN_LIGHT: f32 = 0.9;
/// Past this many pixels from the figure's centre, the lean is whole.
const LEAN_REACH: f32 = 320.;
/// How quickly the springs follow (the higher, the tighter).
const FOLLOW: f32 = 9.;
const LIGHT_FOLLOW: f32 = 5.;
/// A brightness has to move this far past the glyph it shows (in ramp
/// steps) before the glyph changes: slow turns don't flicker.
const HOLD: f32 = 0.65;

/// A critically damped spring (Holden's exact step: frame-rate independent).
#[derive(Clone, Copy, Default)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, target: f32, omega: f32, dt: f32) {
        let y = self.x - target;
        let j = self.v + omega * y;
        let e = (-omega * dt).exp();
        self.x = target + (y + j * dt) * e;
        self.v = (self.v - omega * j * dt) * e;
    }

    fn resting(&self, target: f32) -> bool {
        (self.x - target).abs() < 1e-3 && self.v.abs() < 1e-3
    }
}

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

pub(super) struct Spin {
    pub(super) figure: Figure,
    /// Off: no motion of its own (the Settings switch); the pointer still
    /// turns it.
    pub(super) moving: bool,
    pub(super) ink: Hsla,
    born: Instant,
    last: Instant,
    /// When the pointer last touched the figure: the turn on its own waits
    /// a moment after.
    touched: Instant,
    yaw: Spring,
    pitch: Spring,
    lean_x: Spring,
    lean_y: Spring,
    aim_yaw: f32,
    aim_pitch: f32,
    fling: f32,
    /// The cursor from the figure's centre, `-1..=1` each way.
    lean: (f32, f32),
    drag: Option<Point<Pixels>>,
    /// The ramp step each cell shows, held against flicker.
    shown: Vec<f32>,
}

impl Spin {
    pub(super) fn new(figure: Figure, moving: bool, ink: Hsla) -> Self {
        let now = Instant::now();
        Self {
            figure,
            moving,
            ink,
            born: now,
            last: now,
            touched: now - std::time::Duration::from_secs(10),
            yaw: Spring::default(),
            pitch: Spring::default(),
            lean_x: Spring::default(),
            lean_y: Spring::default(),
            aim_yaw: 0.,
            aim_pitch: 0.,
            fling: 0.,
            lean: (0., 0.),
            drag: None,
            shown: Vec::new(),
        }
    }

    /// Moves every spring on to `now`. True while anything still moves.
    fn tick(&mut self, now: Instant) -> bool {
        let dt = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        let rest = now.duration_since(self.touched).as_secs_f32();
        let idle = match self.moving && self.drag.is_none() {
            true => smoothstep(1.2, 2.6, rest),
            false => 0.,
        };
        if self.drag.is_none() {
            self.aim_yaw += (idle * IDLE_SPIN + self.fling) * dt;
            self.fling *= (-dt / 0.6).exp();
            // a tip given by a drag settles back once it's let go
            self.aim_pitch *= (-dt * 1.2 * smoothstep(0.4, 1.6, rest)).exp();
        }
        let leaning = self.drag.is_none() as u8 as f32;
        let (yaw, pitch) = (
            self.aim_yaw + leaning * self.lean.0 * LEAN_YAW,
            self.aim_pitch + leaning * self.lean.1 * LEAN_PITCH,
        );
        self.yaw.step(yaw, FOLLOW, dt);
        self.pitch.step(pitch, FOLLOW, dt);
        self.lean_x.step(self.lean.0, LIGHT_FOLLOW, dt);
        self.lean_y.step(self.lean.1, LIGHT_FOLLOW, dt);
        self.moving
            || self.drag.is_some()
            || self.fling.abs() > 1e-3
            || self.aim_pitch.abs() > 1e-3
            || !self.yaw.resting(yaw)
            || !self.pitch.resting(pitch)
            || !self.lean_x.resting(self.lean.0)
            || !self.lean_y.resting(self.lean.1)
    }

    fn pose(&self) -> Pose {
        let [x, y, z] = figure::LIGHT;
        Pose {
            yaw: self.yaw.x,
            pitch: self.pitch.x,
            light: [
                x + LEAN_LIGHT * self.lean_x.x,
                y + LEAN_LIGHT * self.lean_y.x,
                z,
            ],
        }
    }

    /// The lines to draw: each cell's glyph, changed only once its
    /// brightness has clearly left the one it shows.
    fn lines(&mut self, shade: &[f32]) -> Vec<String> {
        let steps = (figure::RAMP.len() - 1) as f32;
        if self.shown.len() != shade.len() {
            self.shown = vec![f32::NAN; shade.len()];
        }
        for (shown, &lum) in self.shown.iter_mut().zip(shade) {
            if lum < 0. {
                *shown = f32::NAN;
                continue;
            }
            let level = lum * steps;
            if shown.is_nan() || (level - *shown).abs() > HOLD {
                *shown = level.round();
            }
        }
        self.shown
            .chunks(figure::COLS)
            .map(|row| {
                let line: String = row
                    .iter()
                    .map(|&step| match step.is_nan() {
                        true => ' ',
                        false => figure::RAMP[(step as usize).clamp(1, figure::RAMP.len() - 1)],
                    })
                    .collect();
                line.trim_end().to_owned()
            })
            .collect()
    }
}

impl Render for Spin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        if self.tick(now) {
            window.request_animation_frame();
        }
        let t = match self.moving {
            true => now.duration_since(self.born).as_secs_f32(),
            false => 0.,
        };
        let shade = self.figure.shade(t, self.pose());
        let lines = self.lines(&shade);
        let spin = cx.entity();
        let grabbing = self.drag.is_some();
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(figure::COLS as f32 * figure::ADVANCE))
            .font_family(super::theme::FAMILY_MONO)
            // "===" is one glyph in the mono face; a drawing wants three
            .font_features(FontFeatures::disable_ligatures())
            .text_size(px(figure::GLYPH))
            .line_height(px(figure::SIZE))
            .text_color(self.ink)
            .children(
                lines
                    .into_iter()
                    .map(|line| div().h(px(figure::SIZE)).whitespace_nowrap().child(line)),
            )
            .child(
                div()
                    .id("figure-grab")
                    .absolute()
                    .inset_0()
                    .cursor(match grabbing {
                        true => CursorStyle::ClosedHand,
                        false => CursorStyle::OpenHand,
                    })
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| listen(spin, bounds, window),
                        )
                        .size_full(),
                    ),
            )
    }
}

/// The pointer, window-wide: a press on the figure takes hold of it, a
/// move turns it (held) or leans it (not), a release lets it fly.
fn listen(spin: Entity<Spin>, bounds: Bounds<Pixels>, window: &mut Window) {
    let press = spin.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && bounds.contains(&event.position)
        {
            press.update(cx, |spin, cx| {
                spin.drag = Some(event.position);
                spin.fling = 0.;
                spin.touched = Instant::now();
                cx.notify();
            });
        }
    });
    let hover = spin.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        hover.update(cx, |spin, cx| {
            let centre = bounds.center();
            spin.lean = (
                (f32::from(event.position.x - centre.x) / LEAN_REACH).clamp(-1., 1.),
                (f32::from(event.position.y - centre.y) / LEAN_REACH).clamp(-1., 1.),
            );
            if let Some(from) = spin.drag {
                let (dx, dy) = (
                    f32::from(event.position.x - from.x),
                    f32::from(event.position.y - from.y),
                );
                // the dragged side comes toward the pointer
                spin.aim_yaw -= dx * DRAG;
                spin.aim_pitch = (spin.aim_pitch + dy * DRAG).clamp(-1., 1.);
                spin.drag = Some(event.position);
            }
            if spin.drag.is_some() || bounds.contains(&event.position) {
                spin.touched = Instant::now();
            }
            cx.notify();
        });
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            spin.update(cx, |spin, cx| {
                if spin.drag.take().is_some() {
                    spin.fling = spin.yaw.v.clamp(-4., 4.);
                    spin.touched = Instant::now();
                    cx.notify();
                }
            });
        }
    });
}

/// The figure, kept across frames under `id` without tying its redraws to
/// the view it sits in: it redraws alone, at the display's rate.
pub(super) fn drawing(
    id: impl Into<ElementId>,
    figure: Figure,
    moving: bool,
    ink: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let spin = window.with_global_id(id.into(), |global, window| {
        window.with_element_state(global, |kept: Option<Entity<Spin>>, _| {
            let spin = kept.unwrap_or_else(|| cx.new(|_| Spin::new(figure, moving, ink)));
            (spin.clone(), spin)
        })
    });
    spin.update(cx, |spin, cx| {
        if (spin.figure, spin.moving, spin.ink) != (figure, moving, ink) {
            spin.figure = figure;
            spin.moving = moving;
            spin.ink = ink;
            cx.notify();
        }
    });
    AnyView::from(spin).into_any_element()
}
