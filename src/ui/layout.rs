//! Which view is where: each native window's panes, their frames on the
//! desk, their stacking and focus. The model's, moved by the reducer; the
//! shell draws it and reports the desk's size. The desk seats each
//! program's view in a window of its own, free to move, resize and
//! overlap: a frame on the desk and a place in the stacking order.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MAX_PANES: usize = 8;
pub(crate) const MIN_WIDTH: f32 = 320.;
pub(crate) const MIN_HEIGHT: f32 = 220.;
/// What of a window must stay on the desk: enough of its title bar to grab.
pub(crate) const KEEP: f32 = 96.;
/// A new window opens this far down and right of the one before it.
const CASCADE: f32 = 28.;
/// A new window's share of the desk, wide and high.
const NEW_SHARE: (f32, f32) = (0.6, 0.7);
/// The inset of a window that fills the desk.
pub(crate) const INSET: f32 = 12.;
pub(crate) use crate::render::GRAB;

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

    /// At least `min_w` wide (its pane's [`Pane::min_width`]) and the
    /// smallest window high, no larger than the desk, and never so far off
    /// it that its title bar can't be grabbed back. On a desk narrower than
    /// `min_w` it is the desk's width, and the view scrolls sideways in it.
    /// A frame widened to its floor moves left as far as it takes to stay
    /// on the desk.
    pub(crate) fn clamped(self, desk: (f32, f32), min_w: f32) -> Self {
        let w = self.w.max(min_w).min(desk.0.max(MIN_WIDTH));
        let h = self.h.max(MIN_HEIGHT).min(desk.1.max(MIN_HEIGHT));
        let x = match w > self.w {
            true => self.x.min(desk.0 - w).max(0.),
            false => self.x,
        };
        Self {
            x: x.clamp(KEEP - w, (desk.0 - KEEP).max(0.)),
            y: self.y.clamp(0., (desk.1 - KEEP / 2.).max(0.)),
            w,
            h,
        }
    }
}

/// The module of a window with nothing in it yet: it lists what it can open.
pub(crate) const EMPTY: &str = "";
/// The app's own help, drawn by the shell: no program behind it.
pub(crate) const HELP: &str = "ducktape:help";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pane {
    /// The program shown, or [`EMPTY`] / [`HELP`]: `is_view` tells them apart.
    pub(crate) module: &'static str,
    /// Unique per pane, for life: the shell keys the mounted view by it, so
    /// a pane moved between native windows keeps its view.
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

    pub(crate) fn is_empty(&self) -> bool {
        self.module == EMPTY
    }

    /// A program's view is in it: not empty, not the app's help.
    pub(crate) fn is_view(&self) -> bool {
        !matches!(self.module, EMPTY | HELP)
    }

    /// The narrowest its frame goes: once its view is drawn, the view's
    /// own minimum and the frame's 1px border on each side; never under
    /// the smallest window.
    pub(crate) fn min_width(&self) -> f32 {
        let view = self
            .is_view()
            .then(|| crate::runtime::min_width(self.module));
        view.flatten()
            .map_or(MIN_WIDTH, |min_width| MIN_WIDTH.max(min_width + 2.))
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) panes: Vec<Pane>,
    pub(crate) focused: usize,
    top: u64,
    /// The desk's size as the shell last measured it; `None` until drawn.
    pub(crate) desk: Option<(f32, f32)>,
    /// Something was shown here: the desk no longer opens the active
    /// program on its own.
    pub(crate) initialized: bool,
    /// A window the keyboard holds (⌘⇧M): the arrows move and size it.
    pub(crate) held: Option<Held>,
}

/// A window the keyboard holds, and how it sat when the hold began: Escape
/// puts it back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Held {
    pub(crate) instance: u64,
    frame: Frame,
    restore: Option<Frame>,
}

/// What is done to one window's panes: the bar, a title bar's buttons,
/// the desk's keys, a drag.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PaneMessage {
    /// Shift-click on the bar: the window `module` is already in comes to
    /// the front, else it takes the focused window's place.
    Select(&'static str),
    /// A bar click, or a pick in an empty window: see [`Layout::open`].
    Open(&'static str),
    /// Another pane showing `module`.
    Split(&'static str),
    Close(usize),
    Focus(usize),
    /// The pane into a window of its own, opened `at` (where it sat).
    PopOut {
        index: usize,
        at: Option<gpui_kit::Bounds<gpui_kit::Pixels>>,
    },
    /// This pop-out's pane back onto the console's desk.
    PopIn,
    /// ⌘` / ⌘⇧` / ctrl-tab.
    Cycle {
        forward: bool,
    },
    /// A title bar's double press, or ⌘⇧↩.
    Fill(usize),
    /// ⌘⇧M: the keyboard holds the window (again, and it lets go).
    Hold(usize),
    /// The keyboard lets go of the window it holds: `keep` where it is, or
    /// else back where it was.
    Release {
        keep: bool,
    },
    /// A drag moved or sized a window.
    Frame(usize, Frame),
}

impl Layout {
    /// The desk's size, or nothing yet measured.
    pub(crate) fn desk(&self) -> (f32, f32) {
        self.desk.unwrap_or_default()
    }

    /// The focused pane's program, unless it has none.
    pub(crate) fn shown(&self) -> Option<&'static str> {
        self.panes
            .get(self.focused)
            .filter(|pane| pane.is_view())
            .map(|pane| pane.module)
    }

    /// Every pane gone, the desk's measure kept.
    pub(crate) fn clear(&mut self) {
        *self = Self {
            desk: self.desk,
            ..Self::default()
        };
    }

    /// The desk as the shell measured it. On a desk that changed size
    /// (the window resized, filled the screen or left it) every window
    /// keeps its share of it: its edges keep their place between the
    /// desk's insets, so windows edge to edge stay flush and a filled
    /// window stays filled.
    pub(crate) fn measure(&mut self, desk: (f32, f32)) {
        let Some(old) = self.desk.replace(desk).filter(|old| *old != desk) else {
            return;
        };
        let scale = |at: f32, old: f32, new: f32| {
            let (old, new) = (old - 2. * INSET, new - 2. * INSET);
            match old > 0. && new > 0. {
                true => INSET + (at - INSET) * new / old,
                false => at,
            }
        };
        let fit = |frame: Frame| {
            let (x0, x1) = (
                scale(frame.x, old.0, desk.0),
                scale(frame.x + frame.w, old.0, desk.0),
            );
            let (y0, y1) = (
                scale(frame.y, old.1, desk.1),
                scale(frame.y + frame.h, old.1, desk.1),
            );
            Frame {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            }
        };
        for pane in &mut self.panes {
            pane.frame = pane.frame.map(fit);
            pane.restore = pane.restore.map(fit);
        }
    }

    /// Frames for the windows without one, once the desk is measured. A
    /// hold ends with the window in front that it was on.
    pub(crate) fn settle(&mut self) {
        if let Some(desk) = self.desk {
            self.place(desk);
        }
        let front = self.panes.get(self.focused).map(|pane| pane.instance);
        self.held = self.held.filter(|held| front == Some(held.instance));
    }

    fn raise(&mut self, index: usize) {
        self.top += 1;
        if let Some(pane) = self.panes.get_mut(index) {
            pane.z = self.top;
        }
    }

    /// Selection reuses an existing module (the focused window first)
    /// before replacing a focused view.
    pub(crate) fn select(&mut self, module: &'static str) -> bool {
        if self
            .panes
            .get(self.focused)
            .is_some_and(|pane| pane.module == module)
        {
            return self.focus(self.focused);
        }
        if let Some(index) = self.panes.iter().position(|pane| pane.module == module) {
            return self.focus(index);
        }
        self.load(module);
        true
    }

    /// A menu bar click: into the focused window if it is empty, else to
    /// the window it is already open in, else in a window of its own; on a
    /// full desk, in the focused window's place (as [`Self::popin`]).
    pub(crate) fn open(&mut self, module: &'static str) -> bool {
        if self.panes.get(self.focused).is_some_and(Pane::is_empty) {
            self.load(module);
            return true;
        }
        if let Some(index) = self.panes.iter().position(|pane| pane.module == module) {
            return self.focus(index);
        }
        if !self.split(module) {
            self.load(module);
        }
        true
    }

    /// `module` in the focused window, in place of what it showed.
    pub(crate) fn load(&mut self, module: &'static str) {
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
    }

    /// The next window up (`forward`: the one at the bottom comes to the
    /// top; back: the top one goes to the bottom), so repeating it visits
    /// every window in turn.
    pub(crate) fn cycle(&mut self, forward: bool) -> bool {
        let mut order = self.stacking();
        if order.len() < 2 {
            return false;
        }
        match forward {
            true => order.rotate_left(1),
            false => order.rotate_right(1),
        }
        for &index in &order {
            self.raise(index);
        }
        self.focused = order[order.len() - 1];
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

    /// `pane` onto this desk, instance and all, so its view survives the
    /// move; placed when it lands. A full desk puts it in the focused
    /// window's place instead and hands the displaced pane back — its view
    /// goes when the shell next mounts, being in no layout.
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
    /// opens centred, a later one cascades from the topmost window that has
    /// a frame; every frame is kept where it can be grabbed.
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
            // whole pixels: frames laid edge to edge add back up
            let w = (desk.0 * NEW_SHARE.0).round().max(MIN_WIDTH);
            let h = (desk.1 * NEW_SHARE.1).round().max(MIN_HEIGHT);
            let frame = match beneath {
                None => Frame {
                    x: ((desk.0 - w) / 2.).round(),
                    y: ((desk.1 - h) / 2.).round(),
                    w,
                    h,
                },
                Some(under) => {
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
            let min_w = pane.min_width();
            pane.frame = pane.frame.map(|frame| frame.clamped(desk, min_w));
        }
    }

    pub(crate) fn set_frame(&mut self, index: usize, frame: Frame, desk: (f32, f32)) -> bool {
        let valid = [frame.x, frame.y, frame.w, frame.h]
            .iter()
            .all(|v| v.is_finite());
        match self.panes.get_mut(index) {
            Some(pane) if valid => {
                pane.frame = Some(frame.clamped(desk, pane.min_width()));
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
            Some(restore) => pane.frame = Some(restore.clamped(desk, pane.min_width())),
            None => {
                pane.restore = pane.frame;
                pane.frame = Some(Frame::fill(desk));
            }
        }
    }

    /// The keyboard takes hold of window `index`, once it has a frame; holding
    /// the window it already holds lets it go.
    pub(crate) fn hold(&mut self, index: usize) {
        let held = self.panes.get(index).and_then(|pane| {
            pane.frame.map(|frame| Held {
                instance: pane.instance,
                frame,
                restore: pane.restore,
            })
        });
        self.held = match self.held {
            Some(_) => None,
            None => held,
        };
    }

    /// The keyboard lets go: `keep` the window where it is, or else put it
    /// back as it was, filled or not.
    pub(crate) fn release(&mut self, keep: bool, desk: (f32, f32)) {
        let Some(held) = self.held.take().filter(|_| !keep) else {
            return;
        };
        if let Some(pane) = self
            .panes
            .iter_mut()
            .find(|pane| pane.instance == held.instance)
        {
            pane.frame = Some(held.frame.clamped(desk, pane.min_width()));
            pane.restore = held.restore;
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
mod tests;
