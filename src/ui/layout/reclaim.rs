//! Space back from a closed window: which neighbours grow over its frame.

use super::{Frame, Layout};

/// Edges within half a pixel count as one.
const SLACK: f32 = 0.5;

impl Layout {
    /// The windows beside a closed one grow back over its frame: the one
    /// that shared a whole edge with it, or else the windows lined up
    /// along one side of it that together span that side.
    pub(super) fn reclaim(&mut self, gone: Frame) {
        // a frame along the axis it grows on, then across it
        let along = |frame: Frame, row: bool| match row {
            true => (frame.x, frame.x + frame.w, frame.y, frame.y + frame.h),
            false => (frame.y, frame.y + frame.h, frame.x, frame.x + frame.w),
        };
        let near = |a: f32, b: f32| (a - b).abs() < SLACK;
        let frames: &Vec<Option<Frame>> = &self.panes.iter().map(|pane| pane.frame).collect();
        // same span, touching or overlapping (a window clamped to its
        // floor can overlap its neighbour): the one on top
        let whole = [true, false]
            .into_iter()
            .flat_map(|row| {
                let (g0, g1, s0, s1) = along(gone, row);
                (0..frames.len())
                    .filter(move |&index| {
                        frames[index].is_some_and(|frame| {
                            let (a0, a1, b0, b1) = along(frame, row);
                            b0 == s0 && b1 == s1 && a0 <= g1 && g0 <= a1
                        })
                    })
                    .map(move |index| (index, row))
            })
            .max_by_key(|&(index, _)| self.panes[index].z)
            .map(|(index, row)| (vec![index], row));
        // flush against one side, each within its span, together all of it
        let lined = || {
            [(true, false), (true, true), (false, false), (false, true)]
                .into_iter()
                .find_map(|(row, after)| {
                    let (g0, g1, s0, s1) = along(gone, row);
                    let lined: Vec<usize> = (0..frames.len())
                        .filter(|&index| {
                            frames[index].is_some_and(|frame| {
                                let (a0, a1, b0, b1) = along(frame, row);
                                let flush = if after { near(a0, g1) } else { near(a1, g0) };
                                flush && b0 > s0 - SLACK && b1 < s1 + SLACK
                            })
                        })
                        .collect();
                    let span: f32 = lined
                        .iter()
                        .map(|&index| {
                            let (_, _, b0, b1) = along(frames[index].unwrap(), row);
                            b1 - b0
                        })
                        .sum();
                    (!lined.is_empty() && near(span, s1 - s0)).then_some((lined, row))
                })
        };
        let Some((grown, row)) = whole.or_else(lined) else {
            return;
        };
        let (g0, g1, _, _) = along(gone, row);
        for index in grown {
            let pane = &mut self.panes[index];
            let frame = pane.frame.as_mut().unwrap();
            let (a0, a1, _, _) = along(*frame, row);
            let (from, to) = (a0.min(g0), a1.max(g1));
            match row {
                true => (frame.x, frame.w) = (from, to - from),
                false => (frame.y, frame.h) = (from, to - from),
            }
            pane.restore = None;
        }
    }
}
