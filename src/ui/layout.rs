//! Which view is where: each native window's panes, their frames on the
//! desk, their stacking and focus. Each window's `Desk` (a
//! `shell::entities` slice) holds one and moves it by its methods; the
//! shell draws it and reports the desk's size. The desk seats each
//! program's view in a window of its own, free to move, resize and
//! overlap: a frame on the desk and a place in the stacking order.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MIN_WIDTH: f32 = 320.;
pub(crate) const MIN_HEIGHT: f32 = 220.;
pub(crate) const KEEP: f32 = 96.;
const CASCADE: f32 = 28.;
const NEW_WINDOW_DESK_SHARE: (f32, f32) = (0.6, 0.7);
pub(crate) const INSET: f32 = 12.;
pub(crate) use crate::render::GRAB;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Frame {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl Frame {
    pub(crate) fn clamped(self, desk: (f32, f32), min_w: f32) -> Self {
        let w = self.w.max(min_w).min(desk.0.max(MIN_WIDTH));
        let h = self.h.max(MIN_HEIGHT).min(desk.1.max(MIN_HEIGHT));
        let x = match w > self.w {
            true => self.x.min(desk.0 - w).max(0.),
            false => self.x,
        };
        Self {
            x: x.clamp(0., (desk.0 - KEEP).max(0.)),
            y: self.y.clamp(0., (desk.1 - KEEP / 2.).max(0.)),
            w,
            h,
        }
    }
}

pub(crate) const EMPTY: &str = "";
pub(crate) const HELP: &str = "ducktape:help";
pub(crate) const MEMORY: &str = "ducktape:memory";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pane {
    /// The program shown, or [`EMPTY`] / [`HELP`] / [`MEMORY`]: `is_view`
    /// tells them apart.
    pub(crate) program: &'static str,
    pub(crate) instance: u64,
    pub(crate) frame: Option<Frame>,
    pub(crate) z: u64,
    pub(crate) title: Option<String>,
    pub(crate) by_hand: bool,
    pub(crate) showing: Option<String>,
}

impl Pane {
    pub(crate) fn new(program: &'static str) -> Self {
        static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
        Self {
            program,
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            frame: None,
            z: 0,
            title: None,
            by_hand: false,
            showing: None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.program == EMPTY
    }

    /// A program's view is in it: not empty, not the app's help or memory.
    pub(crate) fn is_view(&self) -> bool {
        !matches!(self.program, EMPTY | HELP | MEMORY)
    }

    pub(crate) fn min_width(&self) -> f32 {
        let view = self
            .is_view()
            .then(|| crate::runtime::min_width(self.program));
        view.flatten()
            .map_or(MIN_WIDTH, |min_width| MIN_WIDTH.max(min_width + 2.))
    }

    pub(crate) fn width(&self) -> Option<f32> {
        let view = self.is_view().then(|| crate::runtime::width(self.program));
        view.flatten().map(|width| width + 2.)
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) panes: Vec<Pane>,
    pub(crate) focused: usize,
    top: u64,
    pub(crate) desk: Option<(f32, f32)>,
    pub(crate) initialized: bool,
    pub(crate) held: Option<Held>,
    pub(crate) filled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Held {
    pub(crate) instance: u64,
    frame: Frame,
    filled: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PaneMessage {
    Select(&'static str),
    Open(&'static str),
    Split(&'static str),
    Close(usize),
    Focus(usize),
    PopOut {
        index: usize,
        at: Option<gpui_kit::Bounds<gpui_kit::Pixels>>,
    },
    PopIn,
    Cycle {
        forward: bool,
    },
    Fill,
    Hold(usize),
    Release {
        keep: bool,
    },
    Frame(usize, Frame),
    Snap(Side),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Layout {
    pub(crate) fn desk(&self) -> (f32, f32) {
        self.desk.unwrap_or_default()
    }

    pub(crate) fn fills(&self, index: usize) -> bool {
        self.filled && index == self.focused
    }

    pub(crate) fn shown(&self) -> Option<&'static str> {
        self.panes
            .get(self.focused)
            .filter(|pane| pane.is_view())
            .map(|pane| pane.program)
    }

    pub(crate) fn clear(&mut self) {
        *self = Self {
            desk: self.desk,
            ..Self::default()
        };
    }

    pub(crate) fn restored(
        panes: Vec<Pane>,
        focused: usize,
        filled: bool,
        desk: Option<(f32, f32)>,
    ) -> Self {
        let top = panes.iter().map(|pane| pane.z).max().unwrap_or(0);
        Self {
            focused: focused.min(panes.len().saturating_sub(1)),
            filled: filled && !panes.is_empty(),
            panes,
            top,
            desk,
            initialized: true,
            held: None,
        }
    }

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
        }
    }

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

    pub(crate) fn select(&mut self, program: &'static str) -> bool {
        if self
            .panes
            .get(self.focused)
            .is_some_and(|pane| pane.program == program)
        {
            return self.focus(self.focused);
        }
        if let Some(index) = self.panes.iter().position(|pane| pane.program == program) {
            return self.focus(index);
        }
        self.load(program);
        true
    }

    pub(crate) fn open(&mut self, program: &'static str) -> bool {
        if self.panes.get(self.focused).is_some_and(Pane::is_empty) {
            self.load(program);
            return true;
        }
        if let Some(index) = self.panes.iter().position(|pane| pane.program == program) {
            return self.focus(index);
        }
        self.split(program);
        true
    }

    pub(crate) fn load(&mut self, program: &'static str) {
        let mut pane = Pane::new(program);
        if let Some(current) = self.panes.get_mut(self.focused) {
            pane.frame = current.frame;
            pane.by_hand = current.by_hand;
            pane.z = current.z;
            *current = pane;
        } else {
            self.panes.push(pane);
            self.focused = 0;
            self.raise(0);
        }
    }

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

    pub(crate) fn split(&mut self, program: &'static str) {
        let index = if self.panes.is_empty() {
            0
        } else {
            self.focused + 1
        };
        self.panes.insert(index, Pane::new(program));
        self.focused = index;
        self.raise(index);
    }

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
        self.focused = (0..self.panes.len())
            .max_by_key(|&index| self.panes[index].z)
            .unwrap_or(0);
        if self.panes.is_empty() {
            self.filled = false;
        }
        Some(pane)
    }

    pub(crate) fn popin(&mut self, mut pane: Pane) {
        pane.frame = None;
        pane.by_hand = false;
        self.panes.push(pane);
        self.focused = self.panes.len() - 1;
        self.raise(self.focused);
    }

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
            let pane = &self.panes[index];
            let w = (desk.0 * NEW_WINDOW_DESK_SHARE.0)
                .round()
                .max(pane.width().unwrap_or(0.))
                .max(pane.min_width())
                .min(desk.0.max(MIN_WIDTH));
            let h = (desk.1 * NEW_WINDOW_DESK_SHARE.1).round().max(MIN_HEIGHT);
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
            let Some(mut frame) = pane.frame else {
                continue;
            };
            if !pane.by_hand
                && let Some(width) = pane.width()
            {
                let w = width.min(desk.0.max(MIN_WIDTH));
                if w > frame.w {
                    let x = frame.x - ((w - frame.w) / 2.).round();
                    frame.x = x.min(desk.0 - w).max(0.);
                    frame.w = w;
                }
            }
            pane.frame = Some(frame.clamped(desk, pane.min_width()));
        }
    }

    pub(crate) fn set_frame(&mut self, index: usize, frame: Frame, desk: (f32, f32)) -> bool {
        let valid = [frame.x, frame.y, frame.w, frame.h]
            .iter()
            .all(|v| v.is_finite());
        match self.panes.get_mut(index) {
            Some(pane) if valid => {
                let frame = Some(frame.clamped(desk, pane.min_width()));
                pane.by_hand |= pane.frame != frame;
                pane.frame = frame;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn snap(&mut self, side: Side) {
        let (desk, index) = (self.desk(), self.focused);
        let (w, h) = (desk.0 - 2. * INSET, desk.1 - 2. * INSET);
        if w <= 0. || h <= 0. {
            return;
        }
        let (half_w, half_h) = ((w / 2.).round(), (h / 2.).round());
        let frame = match side {
            Side::Left => Frame {
                x: INSET,
                y: INSET,
                w: half_w,
                h,
            },
            Side::Right => Frame {
                x: INSET + half_w,
                y: INSET,
                w: w - half_w,
                h,
            },
            Side::Top => Frame {
                x: INSET,
                y: INSET,
                w,
                h: half_h,
            },
            Side::Bottom => Frame {
                x: INSET,
                y: INSET + half_h,
                w,
                h: h - half_h,
            },
        };
        if self.set_frame(index, frame, desk) {
            self.filled = false;
        }
    }

    pub(crate) fn toggle_fill(&mut self) {
        self.filled = !self.filled && !self.panes.is_empty();
    }

    pub(crate) fn hold(&mut self, index: usize) {
        let filled = self.filled;
        let held = self.panes.get(index).and_then(|pane| {
            pane.frame.map(|frame| Held {
                instance: pane.instance,
                frame,
                filled,
            })
        });
        self.held = match self.held {
            Some(_) => None,
            None => held,
        };
        if self.held.is_some() {
            self.filled = false;
        }
    }

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
            self.filled = held.filled;
        }
    }

    pub(crate) fn set_title(&mut self, instance: u64, title: Option<String>) -> bool {
        let title = title.filter(|title| !title.is_empty());
        match self.panes.iter_mut().find(|pane| pane.instance == instance) {
            Some(pane) if pane.title != title => {
                pane.title = title;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn set_showing(&mut self, instance: u64, showing: Option<String>) -> bool {
        let showing = showing.filter(|showing| !showing.is_empty());
        match self.panes.iter_mut().find(|pane| pane.instance == instance) {
            Some(pane) if pane.showing != showing => {
                pane.showing = showing;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn under(&self, at: (f32, f32)) -> Option<usize> {
        let (w, h) = self.desk();
        if !(0. ..w).contains(&at.0) || !(0. ..h).contains(&at.1) {
            return None;
        }
        if self.filled {
            return (self.focused < self.panes.len()).then_some(self.focused);
        }
        self.stacking().into_iter().rev().find(|&index| {
            self.panes[index].frame.is_some_and(|frame| {
                (frame.x - GRAB..frame.x + frame.w + GRAB).contains(&at.0)
                    && (frame.y - GRAB..frame.y + frame.h + GRAB).contains(&at.1)
            })
        })
    }

    pub(crate) fn stacking(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.panes.len()).collect();
        order.sort_by_key(|&index| self.panes[index].z);
        order
    }
}

#[cfg(test)]
mod tests;
