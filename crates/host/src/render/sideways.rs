//! What takes a sideways drag itself, as the last draw laid it out, for a
//! phone's swipe back from anywhere on the screen (the shell asks
//! [`ViewTree::sideways`]): a swipe that starts on one of these is its, not
//! Back. Two kinds of node, read from the frame and from gpui's layout,
//! never from which view drew them:
//!
//! - a scroller sideways (`overflow.x: scroll`, with an id) with room to
//!   scroll there, as gpui laid it out: one whose content fits takes no
//!   sideways drag, as UIKit's scroll view begins no pan it cannot scroll;
//! - a node that hears the wheel (`on_scroll_wheel`): on a phone that is a
//!   finger's pan, whichever way it goes, and the view moves what it likes
//!   with it (a chart it pans).
//!
//! The wire carries no other handler of a dragging finger (a touch drag or
//! a long press of the view's own). Only a touch-first device marks them:
//! nothing else swipes back.
use super::*;
use gpui_kit::Overflow;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub(super) struct Sideways {
    handles: HashMap<AuthoredPath, ScrollHandle>,
    drawn: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Sideways {
    pub(super) fn clear(&self) {
        self.drawn.borrow_mut().clear();
    }

    pub(super) fn retain(&mut self, kept: impl Fn(&AuthoredPath) -> bool) {
        self.handles.retain(|path, _| kept(path));
    }
}

pub(super) fn scrolls_only_sideways(style: &gpui_kit::StyleRefinement) -> bool {
    style.overflow.x == Some(Overflow::Scroll) && style.overflow.y != Some(Overflow::Scroll)
}

impl ViewTree {
    pub(super) fn mark_sideways(
        &mut self,
        mut element: Stateful<Div>,
        style: &gpui_kit::StyleRefinement,
        identified: bool,
        wheel: bool,
    ) -> Stateful<Div> {
        let sideways = identified && style.overflow.x == Some(Overflow::Scroll);
        let upright = identified && style.overflow.y == Some(Overflow::Scroll);
        let scrolls = sideways || upright;
        let locked = scrolls && crate::contract::os().facts().touch_first;
        if locked {
            element = element.restrict_scroll_to_axis();
        }
        if !(sideways || wheel) || !crate::contract::os().facts().touch_first {
            return element;
        }
        let handle = match identified && style.overflow.y == Some(Overflow::Scroll) {
            true => self.scrolls.get(&self.authored_path).cloned(),
            false if sideways => {
                let handle = self
                    .sideways
                    .handles
                    .entry(self.authored_path.clone())
                    .or_default()
                    .clone();
                element = element.track_scroll(&handle);
                Some(handle)
            }
            false => None,
        };
        let drawn = self.sideways.drawn.clone();
        let mark = canvas(
            move |bounds, window, _| {
                let (bounds, takes) = match &handle {
                    Some(handle) => (handle.bounds(), wheel || handle.max_offset().x > px(0.)),
                    None => (bounds, true),
                };
                let shown = bounds.intersect(&window.content_mask().bounds);
                if takes && shown.size.width > px(0.) && shown.size.height > px(0.) {
                    drawn.borrow_mut().push(shown);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        element.child(super::layout::over_padding(&style.padding, mark))
    }

    /// The boxes in the last draw that take a sideways drag themselves, in
    /// window coordinates (see the module): a swipe back does not begin on
    /// one.
    pub fn sideways(&self) -> Vec<Bounds<Pixels>> {
        self.sideways.drawn.borrow().clone()
    }
}
