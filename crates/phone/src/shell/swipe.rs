//! iOS's swipe back from anywhere on the screen, as iOS 26's own stacks
//! take it (`UINavigationController`'s content pop gesture), beside the
//! left edge's. The launcher's pan (`phone/ios/App.swift`) asks [`begins`]
//! the moment UIKit would start it, from inside UIKit's touch handling,
//! where nothing of the app's may be reached; so each draw of the window
//! publishes what that answer needs ([`publish`]): whether a swipe would
//! drag a screen off one under it now, and where the screen takes a
//! sideways drag itself (`ViewTree::sideways`). The left edge's swipe asks
//! nothing: it begins over anything, as UIKit's does. Either one's release
//! is [`pops`]'s to judge.

use gpui_kit::{Bounds, Pixels, Point, px};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// What the last draw showed, as a swipe's start reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// A swipe would drag the screen on top off one under it: there is one
    /// under, nothing is over the screen, nothing moves, no finger is on it.
    pub open: bool,
    /// The boxes that take a sideways drag themselves, in the window's
    /// points.
    pub claimed: Vec<Bounds<Pixels>>,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown {
    open: false,
    claimed: Vec::new(),
});

/// How long a finger may rest before moving for its move to be a swipe:
/// gpui's long press (`GestureTuning::long_press_duration`), which what is
/// under the finger may have taken by then.
const LONG_PRESS: Duration = Duration::from_millis(500);

/// What this draw shows, for the next swipe to read.
pub(super) fn publish(shown: Shown) {
    #[cfg(test)]
    PUBLISHED.with_borrow_mut(|published| *published = shown.clone());
    *SHOWN.lock().unwrap_or_else(PoisonError::into_inner) = shown;
}

// a test's own draws, apart from the other tests' running beside it
#[cfg(test)]
thread_local! {
    static PUBLISHED: std::cell::RefCell<Shown> = std::cell::RefCell::default();
}

/// What the last draw on this thread published.
#[cfg(test)]
pub(super) fn published() -> Shown {
    PUBLISHED.with_borrow(Clone::clone)
}

/// [`begins`] against what the last draw on this thread published.
#[cfg(test)]
pub(super) fn begins_here(start: Point<Pixels>, moving: Point<Pixels>) -> bool {
    begins_on(&published(), start, moving, Duration::ZERO)
}

/// Whether a swipe from anywhere goes back: it came down at `start`, is
/// `moving` (points per second, each way), and rested `held` before it
/// moved; read against the last draw. Never touches the app: UIKit asks
/// from inside its touch handling. It asks at the finger's first move of
/// a point or two, so the way it goes is read from its speed, not from
/// how far it went.
pub fn begins(start: Point<Pixels>, moving: Point<Pixels>, held: Duration) -> bool {
    begins_on(
        &SHOWN.lock().unwrap_or_else(PoisonError::into_inner),
        start,
        moving,
        held,
    )
}

fn begins_on(shown: &Shown, start: Point<Pixels>, moving: Point<Pixels>, held: Duration) -> bool {
    // clearly sideways, to the right: ViewPager's ratio, the horizontal
    // past twice the vertical (`onInterceptTouchEvent`, `xDiff * 0.5f >
    // yDiff`), on the finger's speed; a scroll down is never taken
    let rightward = moving.x > px(0.) && moving.x * 0.5 > moving.y.abs();
    shown.open
        && rightward
        && held < LONG_PRESS
        && !shown.claimed.iter().any(|claimed| claimed.contains(&start))
}

/// Whether a swipe let go `x` points in, moving at `speed` points per
/// second to the right, goes back on a screen `width` wide: past half the
/// width with its speed counted for 0.3 s of travel (react-native-screens'
/// rule, `RNSScreenStack.mm`, which the Expo build ran).
pub(super) fn pops(x: f32, speed: f32, width: f32) -> bool {
    x + 0.3 * speed > width / 2.
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, size};

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    /// Over plain content a finger moving right begins; one moving more
    /// down than twice across, one moving left, one that rested first, one
    /// over what takes a sideways drag, or any while nothing can go back,
    /// does not.
    #[test]
    fn a_swipe_begins_only_clearly_rightward_over_plain_content() {
        let scroller = Bounds::new(at(0., 300.), size(px(400.), px(120.)));
        let shown = Shown {
            open: true,
            claimed: vec![scroller],
        };
        let still = Duration::from_millis(80);
        assert!(begins_on(&shown, at(200., 100.), at(300., 70.), still));
        assert!(
            !begins_on(&shown, at(200., 100.), at(300., 170.), still),
            "a diagonal"
        );
        assert!(
            !begins_on(&shown, at(200., 100.), at(70., 300.), still),
            "a scroll down"
        );
        assert!(
            !begins_on(&shown, at(200., 100.), at(-300., 0.), still),
            "leftward"
        );
        let rested = Duration::from_millis(600);
        assert!(
            !begins_on(&shown, at(200., 100.), at(300., 0.), rested),
            "after a long press"
        );
        assert!(
            !begins_on(&shown, at(200., 350.), at(300., 0.), still),
            "over the scroller"
        );
        let closed = Shown {
            open: false,
            ..shown
        };
        assert!(!begins_on(&closed, at(200., 100.), at(300., 0.), still));
    }

    /// A short swipe let go fast goes back; the same let go slowly, or
    /// moving back left, does not; a long slow one does.
    #[test]
    fn a_fast_short_flick_pops() {
        let width = 402.;
        assert!(pops(60., 900., width), "a flick");
        assert!(!pops(60., 0., width), "let go still, short");
        assert!(!pops(250., -300., width), "let go moving back");
        assert!(pops(250., 0., width), "let go still, past half");
    }
}
