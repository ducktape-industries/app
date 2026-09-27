//! The app's drawings: small shapes in characters, turning in a loop. They
//! are baked, not drawn: `dev/figures/bake.py` renders each once into
//! frames of ramp steps (`assets/figures/*.bin`), and the app plays them.

/// Dark to light, in twelve steps: a baked cell is an index into it.
pub(super) const RAMP: [char; 12] = [' ', '.', '·', ':', ';', '-', '=', '+', '*', '#', '%', '@'];
pub(super) const COLS: usize = 46;
pub(super) const ROWS: usize = 30;
/// The glyph box the frames were baked for: the mono face's advance at
/// `SIZE`, and a line exactly `SIZE` tall.
pub(super) const SIZE: f32 = 11.;
/// The canvas sets 11px with `letter-spacing: 0.5px`: a 7.1px cell. gpui
/// has no letter spacing, so the glyphs are drawn a touch larger instead
/// (`GLYPH`, whose advance is the cell).
pub(super) const ADVANCE: f32 = 7.1;
pub(super) const GLYPH: f32 = ADVANCE / 0.6;
/// The rate the loops were baked for.
pub(super) const FPS: u64 = 20;
/// One frame's bytes: two cells a byte, the high nibble first.
const FRAME: usize = COLS * ROWS / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Figure {
    /// A roll of duct tape, its loose end trailing: the app itself.
    Roll,
    /// A ring, wobbling: a key.
    Ring,
    /// A written card: the recovery phrase on paper.
    Sheets,
    /// A sphere and its small moon: you, and the network you joined.
    Pair,
}

impl Figure {
    fn baked(self) -> &'static [u8] {
        match self {
            Figure::Roll => include_bytes!("../../assets/figures/roll.bin"),
            Figure::Ring => include_bytes!("../../assets/figures/ring.bin"),
            Figure::Sheets => include_bytes!("../../assets/figures/sheets.bin"),
            Figure::Pair => include_bytes!("../../assets/figures/pair.bin"),
        }
    }

    /// How many frames its loop holds.
    pub(super) fn frames(self) -> usize {
        self.baked().len() / FRAME
    }

    /// Frame `n` of the loop (wrapping), each cell's ramp step, row by
    /// row: 0 is blank.
    pub(super) fn frame(self, n: usize) -> impl Iterator<Item = u8> {
        let at = n % self.frames() * FRAME;
        self.baked()[at..at + FRAME]
            .iter()
            .flat_map(|byte| [byte >> 4, byte & 0xF])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Figure; 4] = [Figure::Roll, Figure::Ring, Figure::Sheets, Figure::Pair];

    /// Every loop is whole frames of steps on the ramp, each frame a shape
    /// that fills a fair part of its box, and the loop moves.
    #[test]
    fn every_figure_is_a_moving_loop_of_whole_frames() {
        for figure in ALL {
            assert_eq!(figure.baked().len() % FRAME, 0, "{figure:?}");
            assert_eq!(figure.frames(), 120, "{figure:?}");
            for n in 0..figure.frames() {
                let cells: Vec<u8> = figure.frame(n).collect();
                assert_eq!(cells.len(), COLS * ROWS);
                assert!(cells.iter().all(|&step| (step as usize) < RAMP.len()));
                let inked = cells.iter().filter(|&&step| step > 0).count();
                assert!(
                    inked > COLS * ROWS / 10,
                    "{figure:?} frame {n} is nearly empty"
                );
            }
            assert!(
                figure.frame(0).ne(figure.frame(figure.frames() / 4)),
                "{figure:?} stands still"
            );
            assert!(figure.frame(figure.frames()).eq(figure.frame(0)), "wraps");
        }
    }
}
