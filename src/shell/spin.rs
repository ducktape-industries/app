//! A figure (`figure.rs`: a small 3D shape in characters), held: it tumbles
//! on its own at `figure::FPS`; a drag turns it by hand and a release lets
//! it fly on, slowing, until it eases back into its own tumble. Each tick
//! draws a frame live, painted as one layer of glyphs. With motion off (the
//! Settings switch), or the system asking for less motion
//! (`App::reduce_motion`, which the kit reads from the OS), it only turns
//! by hand.

use std::time::Instant;

use gpui_kit::*;

use super::figure::{self, Figure, Rotation};

/// A drag's turn, radians a pixel.
const DRAG: f32 = 0.012;
/// A fling slows to a third in this long, seconds.
const COAST: f32 = 0.9;
/// After the pointer lets go, its own tumble comes back over these
/// seconds (from, to).
const RESUME: (f32, f32) = (0.8, 2.2);
/// The fastest a fling spins, radians a second.
const FLING_MAX: f32 = 8.;

pub(super) struct Spin {
    pub(super) figure: Figure,
    /// The app's motion switch.
    switch: bool,
    /// It tumbles on its own: the switch is on and the system does not ask
    /// for less motion. Read at `new` and on every tick.
    moving: bool,
    pub(super) ink: Hsla,
    /// How it is held now.
    turn: Rotation,
    /// The spin a release gave it, radians a second about x and y.
    fling: [f32; 2],
    /// Where the pointer held it last, and when.
    drag: Option<(Point<Pixels>, Instant)>,
    /// When the pointer last let go: its own tumble waits a moment.
    touched: Instant,
    /// Frames drawn since it began: the moon's clock.
    ticks: u64,
    /// The ramp, shaped on the first render.
    glyphs: Option<Glyphs>,
    /// Waiting for the next frame.
    ticking: bool,
    /// When it last drew, for `figure.interval`; kept only with perf on.
    last_drawn: Option<Instant>,
}

fn smoothstep((from, to): (f32, f32), x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// The switch on, and the system not asking for less motion. Never written
/// back into `reduce_motion`: gpui-base stops following the OS once the
/// app has set it.
fn moving(switch: bool, cx: &App) -> bool {
    switch && !cx.reduce_motion()
}

impl Spin {
    pub(super) fn new(figure: Figure, switch: bool, ink: Hsla, cx: &App) -> Self {
        Self {
            figure,
            switch,
            moving: moving(switch, cx),
            ink,
            turn: figure::start(),
            fling: [0., 0.],
            drag: None,
            touched: Instant::now() - std::time::Duration::from_secs(10),
            ticks: 0,
            glyphs: None,
            ticking: false,
            last_drawn: None,
        }
    }

    /// Where its own motion is, in seconds.
    fn t(&self) -> f32 {
        match self.moving {
            true => self.ticks as f32 / figure::FPS as f32,
            false => 0.,
        }
    }

    /// Something still moves it: its own tumble, or a fling coasting.
    fn alive(&self) -> bool {
        self.drag.is_none() && (self.moving || self.fling.iter().any(|v| v.abs() > 0.02))
    }

    /// One frame on: its own tumble (faded in after a release) and what
    /// is left of a fling.
    fn tick(&mut self, cx: &App) {
        self.moving = moving(self.switch, cx);
        let dt = 1. / figure::FPS as f32;
        self.ticks += 1;
        if self.drag.is_some() {
            return;
        }
        let own = match self.moving {
            true => smoothstep(RESUME, self.touched.elapsed().as_secs_f32()),
            false => 0.,
        };
        let [x, y, z] = figure::TUMBLE.map(|rate| rate * own * dt);
        let step = figure::rotation(x + self.fling[0] * dt, y + self.fling[1] * dt, z);
        self.turn = self.turn.then(step);
        let keep = (-dt / COAST).exp();
        self.fling = self.fling.map(|v| v * keep);
    }

    /// The pointer took hold of it at `at`.
    fn press(&mut self, at: Point<Pixels>, now: Instant) {
        self.drag = Some((at, now));
        self.fling = [0., 0.];
    }

    /// The pointer, held, moved to `at`: the side it pulls comes with it.
    fn pull(&mut self, at: Point<Pixels>, now: Instant) {
        let Some((from, then)) = self.drag else {
            return;
        };
        let (dx, dy) = (f32::from(at.x - from.x), f32::from(at.y - from.y));
        let (about_x, about_y) = (dy * DRAG, -dx * DRAG);
        self.turn = self.turn.then(figure::rotation(about_x, about_y, 0.));
        // how fast it was turning, for the fling a release gives
        let dt = now.duration_since(then).as_secs_f32().max(0.004);
        let fresh = [about_x / dt, about_y / dt];
        self.fling = [0, 1].map(|i| 0.5 * self.fling[i] + 0.5 * fresh[i]);
        self.drag = Some((at, now));
    }

    /// The pointer let go: it flies on with the drag's last speed, unless
    /// it had stopped moving before the release.
    fn release(&mut self, now: Instant) {
        if let Some((_, then)) = self.drag.take() {
            if now.duration_since(then).as_secs_f32() > 0.1 {
                self.fling = [0., 0.];
            }
            self.fling = self.fling.map(|v| v.clamp(-FLING_MAX, FLING_MAX));
            self.touched = now;
        }
    }

    /// Waits for the next frame and asks for it to be drawn. Its render
    /// waits for the one after, so a hidden window, never drawn, stops;
    /// shown again, it picks up where it was.
    fn run(&mut self, cx: &mut Context<Self>) {
        if self.ticking || !self.alive() {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |spin, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1000 / figure::FPS))
                .await;
            let _ = spin.update(cx, |spin, cx| {
                spin.ticking = false;
                spin.tick(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// The ramp's glyphs, shaped once: nothing is shaped per frame.
#[derive(Clone, Copy)]
struct Glyphs {
    size: Pixels,
    /// Each ramp step's face (a fallback, if the mono face lacks it) and glyph.
    ids: [(FontId, GlyphId); figure::RAMP.len()],
    /// From a cell's top to the baseline, as a line of text puts it.
    baseline: Pixels,
}

impl Glyphs {
    fn shape(window: &Window) -> Self {
        // "===" is one glyph in the mono face; a drawing wants three
        let font = Font {
            features: FontFeatures::disable_ligatures(),
            ..font(super::theme::FAMILY_MONO)
        };
        let size = px(figure::GLYPH);
        let shaped = figure::RAMP.map(|glyph| {
            let text = SharedString::from(glyph.to_string());
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window.text_system().shape_line(text, size, &[run], None)
        });
        let at = &shaped[figure::RAMP.len() - 1];
        Self {
            ids: shaped.each_ref().map(|line| {
                line.runs
                    .first()
                    .and_then(|run| Some((run.font_id, run.glyphs.first()?.id)))
                    .unwrap_or((at.runs[0].font_id, GlyphId(0)))
            }),
            size,
            baseline: (px(figure::SIZE) - at.ascent - at.descent) / 2. + at.ascent,
        }
    }
}

/// Where each inked cell's glyph goes, from the drawing's top left, and
/// its ramp step.
fn stamps(cells: &[u8]) -> Vec<(Point<Pixels>, usize)> {
    cells
        .iter()
        .enumerate()
        .filter(|(_, step)| **step > 0)
        .map(|(cell, &step)| {
            let (col, row) = (cell % figure::COLS, cell / figure::COLS);
            (
                point(
                    px(col as f32 * figure::ADVANCE),
                    px(row as f32 * figure::SIZE),
                ),
                step as usize,
            )
        })
        .collect()
}

impl Render for Spin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.run(cx);
        let glyphs = *self.glyphs.get_or_insert_with(|| Glyphs::shape(window));
        let timed = crate::perf::time(crate::perf::Key::Shell, "figure.frame");
        if let Some(timed) = &timed {
            if let Some(last) = self.last_drawn {
                let between = timed.started().duration_since(last).as_micros() as u64;
                crate::perf::record(crate::perf::Key::Shell, "figure.interval", between);
            }
            self.last_drawn = Some(timed.started());
        }
        let stamps = stamps(&self.figure.frame(self.turn, self.t()));
        drop(timed);
        let ink = self.ink;
        let spin = cx.entity();
        let holding = self.drag.is_some();
        div()
            .relative()
            .w(px(figure::COLS as f32 * figure::ADVANCE))
            .h(px(figure::ROWS as f32 * figure::SIZE))
            .child(
                canvas(
                    |_, _, _| {},
                    // one layer, as a line of text paints: a thousand glyphs
                    // take one draw order, not one bounds-tree insert each
                    move |bounds, _, window, _| {
                        window.paint_layer(bounds, |window| {
                            for (at, step) in stamps {
                                let (face, glyph) = glyphs.ids[step];
                                let origin = bounds.origin + at + point(px(0.), glyphs.baseline);
                                // a glyph the atlas can't take is left out, not fatal
                                let _ = window.paint_glyph(origin, face, glyph, glyphs.size, ink);
                            }
                        })
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .id("figure-grab")
                    .absolute()
                    .inset_0()
                    .cursor(match holding {
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
/// move while held turns it, a release lets it fly. Window-wide so a fast
/// drag can't slip off it.
fn listen(spin: Entity<Spin>, bounds: Bounds<Pixels>, window: &mut Window) {
    let press = spin.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble
            && event.button == MouseButton::Left
            && bounds.contains(&event.position)
        {
            press.update(cx, |spin, cx| {
                spin.press(event.position, Instant::now());
                // the cursor closes its hand
                cx.notify();
            });
        }
    });
    let pull = spin.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            pull.update(cx, |spin, cx| {
                if spin.drag.is_some() {
                    spin.pull(event.position, Instant::now());
                    cx.notify();
                }
            });
        }
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            spin.update(cx, |spin, cx| {
                if spin.drag.is_some() {
                    spin.release(Instant::now());
                    spin.run(cx);
                    cx.notify();
                }
            });
        }
    });
}

/// The figure, kept across frames under `id` without tying its redraws to
/// the view it sits in: it redraws alone, `figure::FPS` times a second
/// while it moves.
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
            let spin = kept.unwrap_or_else(|| cx.new(|cx| Spin::new(figure, moving, ink, cx)));
            (spin.clone(), spin)
        })
    });
    spin.update(cx, |spin, cx| {
        let now = self::moving(moving, cx);
        if (spin.figure, spin.switch, spin.moving, spin.ink) != (figure, moving, now, ink) {
            spin.figure = figure;
            spin.switch = moving;
            spin.moving = now;
            spin.ink = ink;
            cx.notify();
        }
    });
    // cached: the launcher redrawing (a caret, a hover) reuses the last
    // frame instead of stamping it again
    AnyView::from(spin)
        .cached(
            StyleRefinement::default()
                .w(px(figure::COLS as f32 * figure::ADVANCE))
                .h(px(figure::ROWS as f32 * figure::SIZE)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Figure, Spin, figure, stamps};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Hsla, point, px, size};

    /// A figure with the motion switch `moving` in a window, the system
    /// asking for less motion when `reduce` (as the kit sets it from the
    /// OS), drawn `frames` times a tick apart: how many ticks it counted.
    fn play(moving: bool, reduce: bool, frames: u64, cx: &mut gpui_kit::TestAppContext) -> u64 {
        cx.update(gpui_kit::init);
        cx.update(|cx| cx.set_reduce_motion(reduce));
        let window = cx.open_window(size(px(400.), px(400.)), move |_, cx| {
            Spin::new(Figure::Roll, moving, Hsla::default(), cx)
        });
        let spin = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        frame(&mut native, frames);
        native.update(|_, cx| spin.read(cx).ticks)
    }

    /// `frames` frames, a tick apart.
    fn frame(native: &mut gpui_kit::VisualTestContext, frames: u64) {
        for _ in 0..frames {
            native
                .executor()
                .advance_clock(std::time::Duration::from_millis(1000 / figure::FPS));
            native.run_until_parked();
            native.update(|window, cx| window.render_frame(cx));
        }
    }

    fn at(x: f32) -> gpui_kit::Point<gpui_kit::Pixels> {
        point(px(x), px(0.))
    }

    /// A drag turns it, and a release while still moving flings it on,
    /// slowing until it rests.
    #[gpui_kit::test]
    fn a_drag_turns_it_and_a_release_flings_it(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let mut spin = Spin::new(Figure::Roll, false, Hsla::default(), cx);
            let (before, now) = (spin.turn, std::time::Instant::now());
            let ms = |n| now + std::time::Duration::from_millis(n);
            spin.press(at(0.), ms(0));
            spin.pull(at(40.), ms(40));
            assert_ne!(spin.turn, before, "the drag turned it");
            let held = spin.turn;
            spin.release(ms(50));
            assert!(
                spin.fling[1] < -1.,
                "a rightward drag flings it on: {:?}",
                spin.fling
            );
            assert!(spin.alive());
            spin.tick(cx);
            assert_ne!(spin.turn, held, "it flies on after the release");
            for _ in 0..10 * figure::FPS {
                spin.tick(cx);
            }
            assert!(!spin.alive(), "with motion off, a fling comes to rest");
        });
    }

    /// Held still before the release, it stays where it was put.
    #[gpui_kit::test]
    fn a_release_after_holding_still_does_not_fling(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            let mut spin = Spin::new(Figure::Roll, false, Hsla::default(), cx);
            let now = std::time::Instant::now();
            let ms = |n| now + std::time::Duration::from_millis(n);
            spin.press(at(0.), ms(0));
            spin.pull(at(30.), ms(20));
            spin.release(ms(500));
            assert_eq!(spin.fling, [0., 0.]);
            assert!(!spin.alive());
        });
    }

    /// It plays on for good: every tick is a frame, long past any loop.
    #[gpui_kit::test]
    fn a_moving_figure_never_stops(cx: &mut gpui_kit::TestAppContext) {
        assert_eq!(play(true, false, 600, cx), 600);
    }

    /// With motion off it holds still and asks for no frames.
    #[gpui_kit::test]
    fn a_still_figure_asks_for_no_frames(cx: &mut gpui_kit::TestAppContext) {
        assert_eq!(play(false, false, 30, cx), 0);
    }

    /// The system asking for less motion holds it still whatever the
    /// switch says (AX-122), from its first frame or from the tick after
    /// the system asks.
    #[gpui_kit::test]
    fn a_figure_holds_still_when_the_system_asks_for_less_motion(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        assert_eq!(play(true, true, 30, cx), 0);
        cx.update(|cx| cx.set_reduce_motion(false));
        let window = cx.open_window(size(px(400.), px(400.)), |_, cx| {
            Spin::new(Figure::Roll, true, Hsla::default(), cx)
        });
        let spin = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        frame(&mut native, 10);
        native.update(|_, cx| cx.set_reduce_motion(true));
        let asked = native.update(|_, cx| spin.read(cx).ticks);
        frame(&mut native, 30);
        let ticks = native.update(|_, cx| spin.read(cx).ticks);
        assert!(asked >= 10 && ticks <= asked + 1, "{asked} then {ticks}");
    }

    #[test]
    fn stamps_put_each_inked_cell_at_its_column_and_row() {
        let mut cells = vec![0u8; figure::COLS * figure::ROWS];
        cells[0] = 1;
        cells[figure::COLS + 2] = 11;
        cells[figure::COLS * figure::ROWS - 1] = 5;
        assert_eq!(
            stamps(&cells),
            [
                (point(px(0.), px(0.)), 1),
                (point(px(2. * figure::ADVANCE), px(figure::SIZE)), 11),
                (
                    point(
                        px((figure::COLS - 1) as f32 * figure::ADVANCE),
                        px((figure::ROWS - 1) as f32 * figure::SIZE)
                    ),
                    5
                ),
            ]
        );
    }
}
