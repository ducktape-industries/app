//! iOS's swipe back from anywhere on the screen, as iOS 26's own stacks
//! take it (`UINavigationController`'s content pop gesture), beside the
//! left edge's; and on either OS the tabs' swipe at a tab root, a pager's
//! sideways swipe to the tab beside. The launcher's pan (`phone/ios/App.swift`;
//! on Android `MainActivity`'s) asks [`begins`] (Android: [`pages`]) the
//! moment it would start, from inside the OS's touch handling, where
//! nothing of the app's may be reached; so each draw of the window
//! publishes what that answer needs ([`publish`]): whether a swipe would
//! drag a screen off one under it now, whether it would page the tabs, and
//! where the screen takes a sideways drag itself (`ViewTree::sideways`).
//! The left edge's swipe asks nothing: it begins over anything, as UIKit's
//! does. Either one's release is [`crosses`]'s to judge.

use gpui_kit::{Bounds, Pixels, Point, px};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// What the last draw showed, as a swipe's start reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// A swipe would drag the screen on top off one under it: there is one
    /// under, nothing is over the screen, nothing moves, no finger is on it.
    pub open: bool,
    /// A sideways swipe would page the tabs: a tab root is on screen,
    /// nothing is over it, nothing moves, no finger is on it.
    pub tabs: bool,
    /// The boxes that take a sideways drag themselves, in the window's
    /// points.
    pub claimed: Vec<Bounds<Pixels>>,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown {
    open: false,
    tabs: false,
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
    begins_on(&published(), start, moving, Duration::ZERO, true)
}

/// Whether a swipe from anywhere begins: it came down at `start`, is
/// `moving` (points per second, each way), and rested `held` before it
/// moved; read against the last draw. A swipe back begins moving right,
/// the tabs' either way. Never touches the app: UIKit asks from inside its
/// touch handling. It asks at the finger's first move of a point or two,
/// so the way it goes is read from its speed, not from how far it went.
pub fn begins(start: Point<Pixels>, moving: Point<Pixels>, held: Duration) -> bool {
    begins_on(&shown(), start, moving, held, true)
}

/// [`begins`] on a phone whose Back is the system's own (Android): only
/// the tabs' swipe begins.
pub fn pages(start: Point<Pixels>, moving: Point<Pixels>, held: Duration) -> bool {
    begins_on(&shown(), start, moving, held, false)
}

fn shown() -> std::sync::MutexGuard<'static, Shown> {
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

fn begins_on(
    shown: &Shown,
    start: Point<Pixels>,
    moving: Point<Pixels>,
    held: Duration,
    back: bool,
) -> bool {
    // clearly sideways: ViewPager's ratio, the horizontal past twice the
    // vertical (`onInterceptTouchEvent`, `xDiff * 0.5f > yDiff`), on the
    // finger's speed; a scroll down is never taken
    let sideways = moving.x.abs() * 0.5 > moving.y.abs();
    let goes = shown.tabs || (back && shown.open && moving.x > px(0.));
    sideways
        && goes
        && held < LONG_PRESS
        && !shown.claimed.iter().any(|claimed| claimed.contains(&start))
}

/// Whether a swipe let go `x` points along its way, moving at `speed`
/// points per second along it, goes on (back, or to the tab beside) on a
/// screen `width` wide: past half the width with its speed counted for
/// 0.3 s of travel (react-native-screens' rule, `RNSScreenStack.mm`, which
/// the Expo build ran).
pub(super) fn crosses(x: f32, speed: f32, width: f32) -> bool {
    x + 0.3 * speed > width / 2.
}

/// Where a screen `x` points under the finger is drawn when nothing is
/// beside it to come in (the tab bar's ends): it resists, the less the
/// further it goes, UIScrollView's rubber band (`(1 - 1 / (|x| c / w +
/// 1)) w`, c = 0.55), on a screen `width` wide.
pub(super) fn resisted(x: f32, width: f32) -> f32 {
    let width = width.max(1.);
    (1. - 1. / (x.abs() * 0.55 / width + 1.)) * width * x.signum()
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
            tabs: false,
            claimed: vec![scroller],
        };
        let still = Duration::from_millis(80);
        let begins_on =
            |shown: &Shown, start, moving, held| begins_on(shown, start, moving, held, true);
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

    /// At a tab root a swipe begins clearly sideways either way, never down
    /// or after a long press; on Android (no swipe back there) only the
    /// tabs' begins.
    #[test]
    fn the_tabs_swipe_begins_either_way_and_alone_on_android() {
        let tabs = Shown {
            tabs: true,
            ..Shown::default()
        };
        let still = Duration::from_millis(80);
        let start = at(200., 300.);
        assert!(
            begins_on(&tabs, start, at(-300., 70.), still, true),
            "leftward"
        );
        assert!(
            begins_on(&tabs, start, at(300., 70.), still, false),
            "rightward"
        );
        assert!(
            !begins_on(&tabs, start, at(-70., 300.), still, true),
            "a scroll down"
        );
        let rested = Duration::from_millis(600);
        assert!(
            !begins_on(&tabs, start, at(-300., 0.), rested, true),
            "after a long press"
        );
        let open = Shown {
            open: true,
            ..Shown::default()
        };
        assert!(begins_on(&open, start, at(300., 0.), still, true));
        assert!(
            !begins_on(&open, start, at(300., 0.), still, false),
            "a swipe back began on Android"
        );
    }

    /// A short swipe let go fast goes on; the same let go slowly, or
    /// moving back, does not; a long slow one does.
    #[test]
    fn a_fast_short_flick_crosses() {
        let width = 402.;
        assert!(crosses(60., 900., width), "a flick");
        assert!(!crosses(60., 0., width), "let go still, short");
        assert!(!crosses(250., -300., width), "let go moving back");
        assert!(crosses(250., 0., width), "let go still, past half");
    }

    /// The rubber band follows the finger's way, less than the finger
    /// moves, and less the further it goes.
    #[test]
    fn the_rubber_band_resists_more_the_further_it_goes() {
        let width = 400.;
        let step = |x: f32| resisted(x + 40., width) - resisted(x, width);
        assert_eq!(resisted(0., width), 0.);
        assert!(0. < step(0.) && step(0.) < 40., "{}", step(0.));
        assert!(step(360.) < step(0.), "it resists no more further out");
        assert_eq!(resisted(-40., width), -resisted(40., width));
    }
}
