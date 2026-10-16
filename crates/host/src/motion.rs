//! How a phone moves: each transition's duration and curve, named for what
//! moves, as the Expo build's libraries move it (mobile rev 266633c). A
//! pointer window never moves: its callers draw the still layout there.
//!
//! - A screen pushed or popped ([`PUSH`], [`POP`], the geometry in
//!   [`stack`]): expo-router's `Stack` is react-native-screens' native
//!   stack, UIKit's own push on iOS, which is not in any source; matched to
//!   react-native-screens' own copy of it, `simple_push`
//!   (`ios/RNSScreenStackAnimator.mm`: 0.5 s on UIKit's default spring, the
//!   screen under moving 30 % left under a black layer at 0.1). On Android
//!   the API 33+ default (`res/v33/anim-v33/rns_default_*.xml`): 450 ms on
//!   `fast_out_extra_slow_in`, both screens 10 %, the top one fading in its
//!   first 83 ms.
//! - A sheet ([`SHEET_IN`], [`SHEET_OUT`], [`SCRIM`]): `@gorhom/bottom-sheet`
//!   5.2.14's `ANIMATION_CONFIGS` (`src/constants.ts`): on iOS the spring
//!   mass 3, stiffness 1000, damping 500 (UIKit's default spring again,
//!   damping ratio 4.56), on Android 250 ms `Easing.out(Easing.exp)`; its
//!   backdrop's opacity follows the panel's position.
//! - An alert ([`ALERT_IN`], [`ALERT_OUT`]): the kit `Dialog` is a React
//!   Native `Modal` with `animationType="fade"`: Android's
//!   `catalyst_fade_in`/`_out`, 200 ms on `accelerate_interpolator`; iOS's
//!   cross-dissolve, UIKit's own. gpui scales no element, so iOS's alert
//!   grows into place by its fade alone.
//! - A toast ([`TOAST_IN`], [`TOAST_OUT`]): Expo's iOS `Toasts` comes and
//!   goes with no motion; Android's is the system toast, whose
//!   `toast_enter`/`toast_exit` fade over 500 ms (`config_longAnimTime`).
//!
//! Reduce Motion ([`reduced`], the OS's setting as gpui holds it,
//! `App::reduce_motion`) does what iOS does: nothing slides; a slide is the
//! short cross-fade [`FADE`] instead ([`Motion::under`]), and a fade stays.
//!
//! A [`Tween`] is one motion under way. It reads the executor's clock (a
//! test moves it with `advance_clock`) and asks for the next frame from the
//! render that draws it while it moves, never after its end
//! ([`Tween::frame`]): a still window draws nothing (docs/perf.md).

use gpui_kit::{App, Window};
use std::time::{Duration, Instant};

/// How a motion's time maps to its progress: 0 at its start, exactly 1 at
/// its end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// UIKit's default spring (mass 3, stiffness 1000, damping 500), as
    /// Reanimated runs it for a damping ratio of 1 or more: critically
    /// damped at ω₀ = √(1000 / 3) ≈ 18.3 s⁻¹, `1 − e^(−ω₀t)(1 + ω₀t)`, at
    /// rest to within 0.11 % after 0.5 s, so a motion on it ends there.
    Spring,
    /// Android's `fast_out_extra_slow_in`: the path
    /// `M 0,0 C 0.05,0 0.133333,0.06 0.166666,0.4 C 0.208333,0.82 0.25,1 1,1`.
    Emphasized,
    /// `Easing.out(Easing.exp)`: `1 − 2^(−10t)`.
    ExpOut,
    /// Android's `decelerate_quad`: `1 − (1 − t)²`.
    Decelerate,
    /// Android's `accelerate_quad`: `t²`.
    Accelerate,
    /// Quadratic ease-in-out: the cross-fade Reduce Motion puts in place of
    /// a slide.
    EaseInOut,
}

/// [`Curve::Spring`]'s ω₀, per second.
const SPRING_OMEGA: f32 = 18.257_418;

impl Curve {
    /// The progress `time` (a fraction of the motion, 0 to 1) into a motion
    /// of `duration` on this curve.
    pub fn at(self, time: f32, duration: Duration) -> f32 {
        let t = time.clamp(0., 1.);
        if t >= 1. {
            return 1.;
        }
        match self {
            Curve::Spring => {
                let settle = |seconds: f32| {
                    let x = SPRING_OMEGA * seconds;
                    1. - (-x).exp() * (1. + x)
                };
                let seconds = duration.as_secs_f32();
                // scaled so the end is exactly 1: the last frame jumps no
                // fraction of a point
                settle(t * seconds) / settle(seconds)
            }
            Curve::Emphasized => {
                let (a, b) = EMPHASIZED;
                if t < a[3].0 {
                    bezier(a, t)
                } else {
                    bezier(b, t)
                }
            }
            Curve::ExpOut => 1. - 2f32.powf(-10. * t),
            Curve::Decelerate => 1. - (1. - t) * (1. - t),
            Curve::Accelerate => t * t,
            Curve::EaseInOut => {
                if t < 0.5 {
                    2. * t * t
                } else {
                    1. - (2. - 2. * t).powi(2) / 2.
                }
            }
        }
    }
}

/// `fast_out_extra_slow_in`'s two cubic segments, each from its start
/// point through its two controls to its end.
type Segment = [(f32, f32); 4];
const EMPHASIZED: (Segment, Segment) = (
    [(0., 0.), (0.05, 0.), (0.133_333, 0.06), (0.166_666, 0.4)],
    [(0.166_666, 0.4), (0.208_333, 0.82), (0.25, 1.), (1., 1.)],
);

/// The height of `segment` where its x is `x` (its x rises along it).
fn bezier(segment: Segment, x: f32) -> f32 {
    let at = |s: f32, axis: fn(&(f32, f32)) -> f32| {
        let [p0, p1, p2, p3] = segment.map(|point| axis(&point));
        let r = 1. - s;
        r * r * r * p0 + 3. * r * r * s * p1 + 3. * r * s * s * p2 + s * s * s * p3
    };
    let (mut low, mut high) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (low + high) / 2.;
        if at(mid, |p| p.0) < x {
            low = mid;
        } else {
            high = mid;
        }
    }
    at((low + high) / 2., |p| p.1)
}

/// A motion: how long it takes, on what curve, and whether it moves
/// something across the screen (Reduce Motion takes those away).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// How long it takes.
    pub duration: Duration,
    /// How its time maps to its progress.
    pub curve: Curve,
    /// Whether it moves something across the screen.
    pub slides: bool,
}

impl Motion {
    /// What Reduce Motion leaves of this motion: a slide becomes [`FADE`];
    /// a fade is kept.
    pub fn under(self, cx: &App) -> Self {
        match self.slides && reduced(cx) {
            true => FADE,
            false => self,
        }
    }
}

const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// What Reduce Motion puts in place of a slide: a short cross-fade.
pub const FADE: Motion = Motion {
    duration: ms(200),
    curve: Curve::EaseInOut,
    slides: false,
};

/// A screen pushed over the one it was on.
#[cfg(not(target_os = "android"))]
pub const PUSH: Motion = Motion {
    duration: ms(500),
    curve: Curve::Spring,
    slides: true,
};
#[cfg(target_os = "android")]
pub const PUSH: Motion = Motion {
    duration: ms(450),
    curve: Curve::Emphasized,
    slides: true,
};
/// A screen popped off the one under it; the rest of a pop the finger let
/// go of.
pub const POP: Motion = PUSH;

/// A sheet coming up, and a release settling at a detent.
#[cfg(not(target_os = "android"))]
pub const SHEET_IN: Motion = Motion {
    duration: ms(500),
    curve: Curve::Spring,
    slides: true,
};
#[cfg(target_os = "android")]
pub const SHEET_IN: Motion = Motion {
    duration: ms(250),
    curve: Curve::ExpOut,
    slides: true,
};
/// A sheet going down.
pub const SHEET_OUT: Motion = SHEET_IN;
/// The scrim under a sheet: it fades at the panel's own progress
/// (`BottomSheetBackdrop` interpolates the panel's position), so it is the
/// sheet's motion, and under Reduce Motion the sheet's fade.
pub const SCRIM: Motion = SHEET_IN;

/// An alert (and its scrim) fading in.
pub const ALERT_IN: Motion = Motion {
    duration: ms(200),
    curve: Curve::Accelerate,
    slides: false,
};
/// An alert (and its scrim) fading out.
pub const ALERT_OUT: Motion = ALERT_IN;

/// A toast fading in.
pub const TOAST_IN: Motion = Motion {
    duration: ms(500),
    curve: Curve::Decelerate,
    slides: false,
};
/// A toast fading out.
pub const TOAST_OUT: Motion = Motion {
    duration: ms(500),
    curve: Curve::Accelerate,
    slides: false,
};

/// Whether the OS asks for reduced motion.
pub fn reduced(cx: &App) -> bool {
    cx.reduce_motion()
}

/// The clock motions run on: the executor's, which a test moves.
pub fn now(cx: &App) -> Instant {
    cx.background_executor().now()
}

/// How a stack's two screens move: the top one (pushed in, or popped off)
/// over the one under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    /// How far the top screen travels, a fraction of the width.
    pub over: f32,
    /// How far the screen under it moves the other way.
    pub under: f32,
    /// The black laid over the screen under it, at its most.
    pub dim: f32,
    /// When the top screen fades (in on a push, out on a pop), as fractions
    /// of the motion's time: from the first, over the second. `None`: it
    /// stays opaque.
    pub fade: Option<(f32, f32)>,
}

/// Where a stack's screens are, a moment into a move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// The top screen's offset to the right, a fraction of the width.
    pub top: f32,
    /// The screen under it, to the right (it sits to the left, negative).
    pub under: f32,
    /// The black over the screen under it.
    pub dim: f32,
    /// The top screen's opacity.
    pub opacity: f32,
}

impl Slide {
    /// Where the screens are at `progress` (eased, 0 at the move's start, 1
    /// at its end) and `time` (its share of the motion's time) into a push,
    /// or a pop when `pop`.
    pub fn at(self, progress: f32, time: f32, pop: bool) -> Placed {
        let shown = self.fade.map_or(1., |(from, over)| {
            let faded = ((time - from) / over).clamp(0., 1.);
            match pop {
                true => 1. - faded,
                false => faded,
            }
        });
        // how far the top screen is from its place over the stack
        let out = match pop {
            true => progress,
            false => 1. - progress,
        };
        Placed {
            top: self.over * out,
            under: -self.under * (1. - out),
            dim: self.dim * (1. - out),
            opacity: shown,
        }
    }
}

/// iOS's push and pop (react-native-screens `simple_push`).
#[cfg(not(target_os = "android"))]
const PUSHED: Slide = Slide {
    over: 1.,
    under: 0.3,
    dim: 0.1,
    fade: None,
};
#[cfg(not(target_os = "android"))]
const POPPED: Slide = PUSHED;
/// Android's (react-native-screens' v33 defaults over 450 ms): the pushed
/// screen fades in from 50 ms over 83; a popped one fades out from 35.
#[cfg(target_os = "android")]
const PUSHED: Slide = Slide {
    over: 0.1,
    under: 0.1,
    dim: 0.,
    fade: Some((50. / 450., 83. / 450.)),
};
#[cfg(target_os = "android")]
const POPPED: Slide = Slide {
    fade: Some((35. / 450., 83. / 450.)),
    ..PUSHED
};
/// Reduce Motion's: nothing moves; the top screen fades.
const CROSS_FADE: Slide = Slide {
    over: 0.,
    under: 0.,
    dim: 0.,
    fade: Some((0., 1.)),
};

/// A push (or a pop when `pop`): its motion and how its screens move.
pub fn stack(pop: bool, cx: &App) -> (Motion, Slide) {
    let motion = match pop {
        true => POP,
        false => PUSH,
    }
    .under(cx);
    let slide = match (motion.slides, pop) {
        (false, _) => CROSS_FADE,
        (true, true) => POPPED,
        (true, false) => PUSHED,
    };
    (motion, slide)
}

/// One motion under way, from one value to another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    /// The motion, as Reduce Motion left it.
    pub motion: Motion,
    /// When it started, on [`now`]'s clock.
    pub started: Instant,
    /// Its value at its start.
    pub from: f32,
    /// Its value at its end.
    pub to: f32,
}

impl Tween {
    /// `motion` (what Reduce Motion leaves of it) from 0 to 1, now.
    pub fn start(motion: Motion, cx: &App) -> Self {
        Self::between(0., 1., motion, cx)
    }

    /// `motion` (what Reduce Motion leaves of it) from `from` to `to`, now:
    /// the rest of a drag let go of, a release settling at a detent.
    pub fn between(from: f32, to: f32, motion: Motion, cx: &App) -> Self {
        Self {
            motion: motion.under(cx),
            started: now(cx),
            from,
            to,
        }
    }

    /// Its share of its time gone by `now`, 0 to 1.
    pub fn time(&self, now: Instant) -> f32 {
        let total = self.motion.duration.as_secs_f32();
        if total <= 0. {
            return 1.;
        }
        (now.saturating_duration_since(self.started).as_secs_f32() / total).clamp(0., 1.)
    }

    /// Its value at `now`: exactly `to` once it is done.
    pub fn at(&self, now: Instant) -> f32 {
        let eased = self.motion.curve.at(self.time(now), self.motion.duration);
        if eased >= 1. {
            return self.to;
        }
        self.from + (self.to - self.from) * eased
    }

    /// Whether it has ended by `now`.
    pub fn done(&self, now: Instant) -> bool {
        self.time(now) >= 1.
    }

    /// Its value now, from the render that draws it: the next frame asked
    /// for while it moves, none once it has ended.
    pub fn frame(&self, window: &Window, cx: &App) -> f32 {
        let now = now(cx);
        if !self.done(now) {
            window.request_animation_frame();
        }
        self.at(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURVES: [Curve; 6] = [
        Curve::Spring,
        Curve::Emphasized,
        Curve::ExpOut,
        Curve::Decelerate,
        Curve::Accelerate,
        Curve::EaseInOut,
    ];

    /// Every curve starts at 0, ends at exactly 1, and never goes back.
    #[test]
    fn every_curve_runs_from_zero_to_exactly_one_and_never_back() {
        for curve in CURVES {
            let duration = ms(500);
            assert!(
                curve.at(0., duration).abs() < 1e-3,
                "{curve:?} starts off 0"
            );
            assert_eq!(curve.at(1., duration), 1., "{curve:?} ends off 1");
            let mut last = 0.;
            for step in 0..=100 {
                let at = curve.at(step as f32 / 100., duration);
                assert!(at + 1e-4 >= last, "{curve:?} went back at {step}");
                last = at;
            }
        }
    }

    /// UIKit's spring is most of the way there at a third of its time, as
    /// the sheet and the push look on iOS; Android's path is too (its knee
    /// at a sixth).
    #[test]
    fn the_springs_and_the_path_front_load_their_travel() {
        let spring = Curve::Spring.at(0.3, ms(500));
        assert!((0.7..0.85).contains(&spring), "{spring}");
        let path = Curve::Emphasized.at(0.166_666, ms(450));
        assert!((path - 0.4).abs() < 1e-3, "{path}");
    }

    /// A push starts with the new screen off to the right and the old one
    /// in place, and ends with the new one in place and the old one 30 %
    /// left under the dim; a pop is the same move backwards.
    #[test]
    fn a_push_and_a_pop_mirror_each_other() {
        let push = PUSHED.at(0., 0., false);
        assert_eq!((push.top, push.under, push.dim), (PUSHED.over, 0., 0.));
        let pushed = PUSHED.at(1., 1., false);
        assert_eq!((pushed.top, pushed.opacity), (0., 1.));
        assert_eq!((pushed.under, pushed.dim), (-PUSHED.under, PUSHED.dim));
        for progress in [0., 0.25, 0.5, 1.] {
            let push = PUSHED.at(progress, progress, false);
            let pop = POPPED.at(1. - progress, 1. - progress, true);
            assert_eq!(
                (push.top, push.under, push.dim),
                (pop.top, pop.under, pop.dim)
            );
        }
        let gone = CROSS_FADE.at(1., 1., true);
        assert_eq!((gone.top, gone.under, gone.opacity), (0., 0., 0.));
    }

    /// A tween is exactly at its end once its time is up, and at its start
    /// at the start.
    #[test]
    fn a_tween_lands_exactly_on_its_end() {
        let started = Instant::now();
        let tween = Tween {
            motion: PUSH,
            started,
            from: 0.25,
            to: 1.,
        };
        assert_eq!(tween.at(started), 0.25);
        assert!(!tween.done(started));
        let end = started + PUSH.duration;
        assert!(tween.done(end));
        assert_eq!(tween.at(end), 1.);
        assert_eq!(tween.at(end + ms(1)), 1.);
    }
}
