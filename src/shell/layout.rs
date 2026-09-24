//! Window-local view ownership and placement, independent of native view
//! entities. The desk seats each program's view in a window of its own,
//! free to move, resize and overlap: a frame on the desk and a place in the
//! stacking order.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MAX_PANES: usize = 8;
pub(crate) const MIN_WIDTH: f32 = 320.;
pub(crate) const MIN_HEIGHT: f32 = 220.;
/// What of a window must stay on the desk: enough of its title bar to grab.
const KEEP: f32 = 96.;
/// A new window opens this far down and right of the one before it.
const CASCADE: f32 = 28.;
/// The inset of a window that fills the desk.
pub(crate) const INSET: f32 = 12.;
/// How far past its border a window can still be taken hold of: its
/// edges' grips reach out this much into the gap around it.
pub(crate) const GRAB: f32 = 5.;

/// A window's place on the desk, in pixels from the desk's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl Frame {
    /// The whole desk, inset.
    pub(crate) fn fill(desk: (f32, f32)) -> Self {
        Self {
            x: INSET,
            y: INSET,
            w: (desk.0 - 2. * INSET).max(MIN_WIDTH),
            h: (desk.1 - 2. * INSET).max(MIN_HEIGHT),
        }
    }

    /// At least the smallest window, no larger than the desk, and never so
    /// far off it that its title bar can't be grabbed back.
    pub(crate) fn clamped(self, desk: (f32, f32)) -> Self {
        let w = self.w.max(MIN_WIDTH).min(desk.0.max(MIN_WIDTH));
        let h = self.h.max(MIN_HEIGHT).min(desk.1.max(MIN_HEIGHT));
        Self {
            x: self.x.clamp(KEEP - w, (desk.0 - KEEP).max(0.)),
            y: self.y.clamp(0., (desk.1 - KEEP / 2.).max(0.)),
            w,
            h,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pane {
    pub(crate) module: &'static str,
    pub(crate) instance: u64,
    /// None until the desk it lands on is measured.
    pub(crate) frame: Option<Frame>,
    /// Stacking order: the highest is drawn on top.
    pub(crate) z: u64,
    /// The frame to go back to when a filled window is filled again.
    pub(crate) restore: Option<Frame>,
}

impl Pane {
    pub(crate) fn new(module: &'static str) -> Self {
        static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
        Self {
            module,
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            frame: None,
            z: 0,
            restore: None,
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct Layout {
    pub(crate) panes: Vec<Pane>,
    pub(crate) focused: usize,
    top: u64,
}

impl Layout {
    fn raise(&mut self, index: usize) {
        self.top += 1;
        if let Some(pane) = self.panes.get_mut(index) {
            pane.z = self.top;
        }
    }

    /// Rail selection reuses an existing module before replacing a focused view.
    pub(crate) fn select(&mut self, module: &'static str) -> bool {
        if let Some(index) = self.panes.iter().position(|pane| pane.module == module) {
            return self.focus(index);
        }
        let mut pane = Pane::new(module);
        if let Some(current) = self.panes.get_mut(self.focused) {
            pane.frame = current.frame;
            pane.z = current.z;
            *current = pane;
        } else {
            self.panes.push(pane);
            self.focused = 0;
            self.raise(0);
        }
        true
    }

    /// Another window beside the focused one (cascaded when it lands).
    pub(crate) fn split(&mut self, module: &'static str) -> bool {
        if self.panes.len() == MAX_PANES {
            return false;
        }
        let index = if self.panes.is_empty() {
            0
        } else {
            self.focused + 1
        };
        self.panes.insert(index, Pane::new(module));
        self.focused = index;
        self.raise(index);
        true
    }

    /// Focus brings a window to the top. False if nothing changed.
    pub(crate) fn focus(&mut self, index: usize) -> bool {
        if index >= self.panes.len() {
            return false;
        }
        let on_top = self.panes[index].z == self.top;
        if self.focused == index && on_top {
            return false;
        }
        self.focused = index;
        self.raise(index);
        true
    }

    pub(crate) fn close(&mut self, index: usize) -> Option<Pane> {
        if index >= self.panes.len() {
            return None;
        }
        let pane = self.panes.remove(index);
        // the next focus is the window now on top
        self.focused = (0..self.panes.len())
            .max_by_key(|&index| self.panes[index].z)
            .unwrap_or(0);
        Some(pane)
    }

    pub(crate) fn popout(&mut self, index: usize) -> Option<Pane> {
        self.close(index)
    }

    /// Returns the displaced view so its entity can be released by the caller.
    pub(crate) fn popin(&mut self, mut pane: Pane) -> Option<Pane> {
        pane.frame = None;
        pane.restore = None;
        if self.panes.len() == MAX_PANES {
            pane.frame = self.panes[self.focused].frame;
            let displaced = std::mem::replace(&mut self.panes[self.focused], pane);
            self.raise(self.focused);
            Some(displaced)
        } else {
            self.panes.push(pane);
            self.focused = self.panes.len() - 1;
            self.raise(self.focused);
            None
        }
    }

    /// Gives every window a frame on a desk of `desk` size: the first one
    /// fills it, a later one cascades from the window it opened beside;
    /// every frame is kept where it can be grabbed.
    pub(crate) fn place(&mut self, desk: (f32, f32)) {
        for index in 0..self.panes.len() {
            if self.panes[index].frame.is_some() {
                continue;
            }
            let beneath = self
                .panes
                .iter()
                .filter(|pane| pane.frame.is_some())
                .max_by_key(|pane| pane.z)
                .and_then(|pane| pane.frame);
            let frame = match beneath {
                None => Frame::fill(desk),
                Some(under) => {
                    let w = (desk.0 * 0.6).max(MIN_WIDTH);
                    let h = (desk.1 * 0.7).max(MIN_HEIGHT);
                    let mut x = under.x + CASCADE;
                    let mut y = under.y + CASCADE;
                    // past the desk's far corner, start again at its top-left
                    if x + w > desk.0 || y + h > desk.1 {
                        x = INSET;
                        y = INSET;
                    }
                    Frame { x, y, w, h }
                }
            };
            self.panes[index].frame = Some(frame);
        }
        for pane in &mut self.panes {
            pane.frame = pane.frame.map(|frame| frame.clamped(desk));
        }
    }

    pub(crate) fn set_frame(&mut self, index: usize, frame: Frame, desk: (f32, f32)) -> bool {
        let valid = [frame.x, frame.y, frame.w, frame.h]
            .iter()
            .all(|v| v.is_finite());
        match self.panes.get_mut(index) {
            Some(pane) if valid => {
                pane.frame = Some(frame.clamped(desk));
                pane.restore = None;
                true
            }
            _ => false,
        }
    }

    /// Fills the desk with a window, or puts a filled one back.
    pub(crate) fn toggle_fill(&mut self, index: usize, desk: (f32, f32)) {
        let Some(pane) = self.panes.get_mut(index) else {
            return;
        };
        match pane.restore.take() {
            Some(restore) => pane.frame = Some(restore.clamped(desk)),
            None => {
                pane.restore = pane.frame;
                pane.frame = Some(Frame::fill(desk));
            }
        }
    }

    /// The window on top at `at` on the desk, its grips around it included.
    pub(crate) fn under(&self, at: (f32, f32)) -> Option<usize> {
        self.stacking().into_iter().rev().find(|&index| {
            self.panes[index].frame.is_some_and(|frame| {
                (frame.x - GRAB..frame.x + frame.w + GRAB).contains(&at.0)
                    && (frame.y - GRAB..frame.y + frame.h + GRAB).contains(&at.1)
            })
        })
    }

    /// Indices bottom to top: the order windows are drawn in.
    pub(crate) fn stacking(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.panes.len()).collect();
        order.sort_by_key(|&index| self.panes[index].z);
        order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESK: (f32, f32) = (1400., 860.);

    #[test]
    fn rail_selection_focuses_existing_or_replaces_only_focused() {
        let mut layout = Layout::default();
        assert!(layout.select("chat"));
        let chat = layout.panes[0].instance;
        assert!(layout.split("files"));
        assert!(layout.select("chat"));
        assert_eq!(layout.focused, 0);
        assert_eq!(layout.panes[0].instance, chat);
        assert!(!layout.select("chat"));
        layout.place(DESK);
        let frame = layout.panes[0].frame;
        assert!(layout.select("calendar"));
        assert_eq!(layout.panes.len(), 2);
        assert_eq!(layout.panes[0].module, "calendar");
        assert_eq!(
            layout.panes[0].frame, frame,
            "a replaced view keeps its window"
        );
        assert_ne!(layout.panes[0].instance, chat);
        assert_eq!(layout.panes[1].module, "files");
    }

    #[test]
    fn split_opens_on_top_with_unique_instances_and_limits_count() {
        let mut layout = Layout::default();
        assert!(layout.split("chat"));
        assert!(layout.split("files"));
        assert!(layout.focus(0));
        assert!(layout.split("chat"));
        assert_eq!(layout.focused, 1);
        assert_eq!(layout.panes[1].module, "chat");
        assert_eq!(
            *layout.stacking().last().unwrap(),
            1,
            "the new window is on top"
        );
        assert_ne!(layout.panes[0].instance, layout.panes[1].instance);
        while layout.split("calendar") {}
        assert_eq!(layout.panes.len(), MAX_PANES);
        assert!(!layout.focus(MAX_PANES));
    }

    #[test]
    fn focus_raises_and_closing_focuses_the_window_left_on_top() {
        let mut layout = Layout::default();
        for module in ["chat", "files", "calendar"] {
            layout.split(module);
        }
        assert!(layout.focus(0));
        assert_eq!(layout.stacking(), vec![1, 2, 0]);
        for (index, pane) in layout.panes.iter_mut().enumerate() {
            pane.frame = Some(Frame::fill((800., 600.)));
            pane.frame.as_mut().unwrap().x += index as f32 * 100.;
        }
        assert_eq!(layout.under((50., 50.)), Some(0), "the top of three");
        assert_eq!(layout.under((250., 50.)), Some(0));
        assert_eq!(
            layout.under((850., 50.)),
            Some(2),
            "only the last reaches here"
        );
        assert_eq!(layout.under((990., 50.)), Some(2), "just past its border");
        assert_eq!(layout.under((5., 5.)), None, "the desk's inset");
        assert!(!layout.focus(0), "already focused and on top");
        assert!(layout.close(0).is_some());
        // "calendar" (now index 1) was raised after "files"
        assert_eq!(layout.focused, 1);
        assert!(layout.close(1).is_some());
        assert!(layout.close(0).is_some());
        assert!(layout.panes.is_empty());
        assert!(layout.close(0).is_none());
        assert!(!layout.focus(0));
    }

    #[test]
    fn popout_and_popin_transfer_instance_and_replace_at_capacity() {
        let mut source = Layout::default();
        source.split("chat");
        let original = source.panes[0].instance;
        let pane = source.popout(0).unwrap();
        assert!(source.panes.is_empty());
        let mut destination = Layout::default();
        assert!(destination.popin(pane).is_none());
        assert_eq!(destination.panes[0].instance, original);
        while destination.split("files") {}
        destination.focus(1);
        let displaced = destination.panes[1].instance;
        let pane = Pane::new("chat");
        let incoming = pane.instance;
        assert_eq!(destination.popin(pane).unwrap().instance, displaced);
        assert_eq!(destination.panes.len(), MAX_PANES);
        assert_eq!(destination.panes[1].instance, incoming);
        assert!(destination.popout(MAX_PANES).is_none());
    }

    #[test]
    fn the_first_window_fills_the_desk_and_later_ones_cascade_inside_it() {
        let mut layout = Layout::default();
        layout.split("chat");
        layout.place(DESK);
        let first = layout.panes[0].frame.unwrap();
        assert_eq!(first, Frame::fill(DESK));
        layout.split("files");
        layout.place(DESK);
        let second = layout.panes[1].frame.unwrap();
        assert_eq!((second.x, second.y), (first.x + CASCADE, first.y + CASCADE));
        assert!(second.x + second.w <= DESK.0 && second.y + second.h <= DESK.1);
    }

    #[test]
    fn frames_stay_grabbable_and_reject_invalid_input() {
        let mut layout = Layout::default();
        layout.split("chat");
        layout.place(DESK);
        let far = Frame {
            x: 5000.,
            y: -300.,
            w: 10.,
            h: 10.,
        };
        assert!(layout.set_frame(0, far, DESK));
        let kept = layout.panes[0].frame.unwrap();
        assert_eq!((kept.w, kept.h), (MIN_WIDTH, MIN_HEIGHT));
        assert!(kept.x <= DESK.0 - KEEP && kept.y >= 0.);
        let nan = Frame {
            x: f32::NAN,
            ..kept
        };
        assert!(!layout.set_frame(0, nan, DESK));
        assert!(!layout.set_frame(3, kept, DESK));
        assert_eq!(layout.panes[0].frame, Some(kept));
        // a shrinking desk pulls windows back onto it
        layout.place((600., 400.));
        let shrunk = layout.panes[0].frame.unwrap();
        assert!(shrunk.x <= 600. - KEEP);
    }

    #[test]
    fn filling_a_window_and_filling_it_again_puts_it_back() {
        let mut layout = Layout::default();
        layout.split("chat");
        layout.place(DESK);
        let small = Frame {
            x: 100.,
            y: 80.,
            w: 500.,
            h: 400.,
        };
        layout.set_frame(0, small, DESK);
        layout.toggle_fill(0, DESK);
        assert_eq!(layout.panes[0].frame, Some(Frame::fill(DESK)));
        layout.toggle_fill(0, DESK);
        assert_eq!(layout.panes[0].frame, Some(small));
    }
}
