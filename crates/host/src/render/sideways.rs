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

/// The marks of one tree.
#[derive(Default)]
pub(super) struct Sideways {
    /// Each scroller sideways that does not also scroll down (that one
    /// keeps its handle in `ViewTree::scrolls`), by its path: its handle,
    /// which keeps its offset from frame to frame and says how far it can
    /// go.
    handles: HashMap<AuthoredPath, ScrollHandle>,
    /// The boxes this draw gave them, on the screen and as far as they
    /// show: filled as the frame is laid out, emptied when it is made
    /// again. A frame gpui keeps as it was (a still view) keeps them.
    drawn: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Sideways {
    /// A new frame is being made: its marks replace the last one's.
    pub(super) fn clear(&self) {
        self.drawn.borrow_mut().clear();
    }

    /// The handles of scrollers still in the frame, by `kept`.
    pub(super) fn retain(&mut self, kept: impl Fn(&AuthoredPath) -> bool) {
        self.handles.retain(|path, _| kept(path));
    }
}

/// Whether `style` scrolls sideways and not down: such a scroller keeps a
/// handle here (the frame's walk and the container agree on it).
pub(super) fn scrolls_only_sideways(style: &gpui_kit::StyleRefinement) -> bool {
    style.overflow.x == Some(Overflow::Scroll) && style.overflow.y != Some(Overflow::Scroll)
}

impl ViewTree {
    /// `element`, the container at the current path, marked when it takes
    /// a sideways drag (see the module): a scroller sideways tracks its
    /// handle, and a mark spanning the node records its box as it is laid
    /// out. Unmarked off a touch-first device.
    pub(super) fn mark_sideways(
        &mut self,
        mut element: Stateful<Div>,
        style: &gpui_kit::StyleRefinement,
        identified: bool,
        wheel: bool,
    ) -> Stateful<Div> {
        let sideways = identified && style.overflow.x == Some(Overflow::Scroll);
        if !(sideways || wheel) || !crate::contract::os().facts().touch_first {
            return element;
        }
        // a scroller's own box and reach, from its handle: the mark inside
        // it moves with its content
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
