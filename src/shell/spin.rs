//! A figure, playing: its baked loop a frame at a time, painted as one
//! layer of glyphs. With motion off (the Settings switch) it holds its
//! first frame.

use std::time::Instant;

use gpui_kit::*;

use super::figure::{self, Figure};

pub(super) struct Spin {
    pub(super) figure: Figure,
    pub(super) moving: bool,
    pub(super) ink: Hsla,
    born: Instant,
    /// The frame on screen.
    frame: usize,
    /// The ramp, shaped on the first render.
    glyphs: Option<Glyphs>,
    /// Waiting for the next frame.
    ticking: bool,
}

impl Spin {
    pub(super) fn new(figure: Figure, moving: bool, ink: Hsla) -> Self {
        Self {
            figure,
            moving,
            ink,
            born: Instant::now(),
            frame: 0,
            glyphs: None,
            ticking: false,
        }
    }

    /// The frame the loop has reached.
    fn due(&self) -> usize {
        match self.moving {
            true => self.born.elapsed().as_millis() as usize * figure::FPS as usize / 1000,
            false => 0,
        }
    }

    /// Waits for the next frame and asks for it to be drawn. Its render
    /// waits for the one after, so a hidden window, never drawn, stops.
    fn run(&mut self, cx: &mut Context<Self>) {
        if self.ticking || !self.moving {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |spin, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1000 / figure::FPS))
                .await;
            let _ = spin.update(cx, |spin, cx| {
                spin.ticking = false;
                spin.frame = spin.due();
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
fn stamps(cells: impl Iterator<Item = u8>) -> Vec<(Point<Pixels>, usize)> {
    cells
        .enumerate()
        .filter(|(_, step)| *step > 0)
        .map(|(cell, step)| {
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
        let stamps = stamps(self.figure.frame(self.frame));
        let ink = self.ink;
        div()
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
    }
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
            spin.frame = spin.due();
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
    use super::{figure, stamps};
    use gpui_kit::{point, px};

    #[test]
    fn stamps_put_each_inked_cell_at_its_column_and_row() {
        let mut cells = vec![0u8; figure::COLS * figure::ROWS];
        cells[0] = 1;
        cells[figure::COLS + 2] = 11;
        cells[figure::COLS * figure::ROWS - 1] = 5;
        assert_eq!(
            stamps(cells.into_iter()),
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
