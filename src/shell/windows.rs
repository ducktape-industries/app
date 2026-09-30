//! Where an OS window goes: a pop-out off its source window (`cascade`,
//! `unseated`), the launcher centred (`centered`). Opening one is
//! `entities::Windows`'.

use super::layout;
use gpui_kit::{Bounds, Pixels, Size, point, px, size};

/// A window's size before the person resizes it.
pub(in crate::shell) const WINDOW_SIZE: (f32, f32) = (1280., 800.);

/// How far a pop-out steps down and right of the window it left.
const CASCADE: f32 = 32.;

/// The narrowest and lowest a pop-out goes, whatever its view: its own
/// title strip and bar are not laid out under this.
pub(in crate::shell) const POPOUT_MIN: f32 = 480.;

/// The narrowest a pop-out holding `module`'s view goes: the view's own
/// minimum once it is drawn, never under [`POPOUT_MIN`].
pub(in crate::shell) fn popout_min(module: &str) -> f32 {
    crate::runtime::min_width(module).map_or(POPOUT_MIN, |min_width| min_width.max(POPOUT_MIN))
}

/// Where a pop-out opens: stepped off its source window so it does not
/// land exactly on top of it, and pulled back inside `display` when the
/// step would push it off an edge.
pub(in crate::shell) fn cascade(
    source: Bounds<Pixels>,
    display: Option<Bounds<Pixels>>,
) -> Bounds<Pixels> {
    let extent = size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1));
    let mut origin = point(source.origin.x + px(CASCADE), source.origin.y + px(CASCADE));
    if let Some(display) = display {
        let right = display.origin.x + display.size.width - extent.width;
        let bottom = display.origin.y + display.size.height - extent.height;
        origin.x = origin.x.min(right).max(display.origin.x);
        origin.y = origin.y.min(bottom).max(display.origin.y);
    }
    Bounds::new(origin, extent)
}

/// Where the launcher sits when the desk shrinks to it: centred in
/// `display`, and never past its top-left corner when it doesn't fit.
pub(in crate::shell) fn centered(extent: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let origin = point(
        display.origin.x + ((display.size.width - extent.width) / 2.).max(px(0.)),
        display.origin.y + ((display.size.height - extent.height) / 2.).max(px(0.)),
    );
    Bounds::new(origin, extent)
}

/// Where a window leaving the desk opens: on the screen right where it sat,
/// under the console's bar rather than over it, at least `min_w` wide
/// ([`popout_min`]), and inside `display`. With no place on the desk to
/// keep, it cascades.
pub(in crate::shell) fn unseated(
    source: Bounds<Pixels>,
    frame: Option<layout::Frame>,
    display: Option<Bounds<Pixels>>,
    min_w: f32,
) -> Bounds<Pixels> {
    let Some(frame) = frame else {
        return cascade(source, display);
    };
    let extent = size(px(frame.w.max(min_w)), px(frame.h.max(POPOUT_MIN)));
    let mut origin = point(
        source.origin.x + px(frame.x),
        source.origin.y + px(super::layers::BAR + frame.y),
    );
    if let Some(display) = display {
        let right = display.origin.x + display.size.width - extent.width;
        let bottom = display.origin.y + display.size.height - extent.height;
        origin.x = origin.x.min(right).max(display.origin.x);
        origin.y = origin.y.min(bottom).max(display.origin.y);
    }
    Bounds::new(origin, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn a_window_leaving_the_desk_opens_where_it_sat_under_the_bar() {
        let seat = layout::Frame {
            x: 100.,
            y: 50.,
            w: 900.,
            h: 600.,
        };
        let display = frame(0., 0., 2560., 1440.);
        let at = unseated(
            frame(200., 100., 1280., 800.),
            Some(seat),
            Some(display),
            POPOUT_MIN,
        );
        assert_eq!(
            at,
            frame(300., 100. + crate::shell::layers::BAR + 50., 900., 600.)
        );
        assert!(
            at.origin.y >= px(100. + crate::shell::layers::BAR),
            "the console's bar stays uncovered"
        );
        // no narrower than its view: a 500 px frame of a view laid out from 680
        let narrow = layout::Frame { w: 500., ..seat };
        let at = unseated(
            frame(200., 100., 1280., 800.),
            Some(narrow),
            Some(display),
            680.,
        );
        assert_eq!(at.size.width, px(680.));
    }

    #[test]
    fn a_popout_is_never_narrower_than_its_view_nor_480() {
        assert_eq!(
            popout_min("popout-unseated-view"),
            POPOUT_MIN,
            "not drawn yet"
        );
        crate::runtime::seat_for_test("popout-wide-view", 640);
        assert_eq!(popout_min("popout-wide-view"), 640.);
        crate::runtime::seat_for_test("popout-narrow-view", 320);
        assert_eq!(popout_min("popout-narrow-view"), POPOUT_MIN);
    }

    #[test]
    fn a_popout_steps_off_its_source_window() {
        let display = frame(0., 0., 1600., 1000.);
        let at = cascade(frame(160., 100., 1280., 800.), Some(display));
        assert_eq!(at, frame(192., 132., 1280., 800.));
    }

    #[test]
    fn a_popout_stays_on_the_display() {
        let display = frame(0., 0., 1600., 1000.);
        let at = cascade(frame(320., 200., 1280., 800.), Some(display));
        assert_eq!(at, frame(320., 200., 1280., 800.));
        let small = frame(0., 0., 1024., 700.);
        assert_eq!(
            cascade(frame(0., 0., 1024., 700.), Some(small)).origin,
            point(px(0.), px(0.))
        );
    }

    #[test]
    fn the_launcher_comes_up_centred_on_its_display() {
        let launcher = size(px(960.), px(640.));
        // a second display to the right, under a 25px menu bar
        let display = frame(1920., 25., 2560., 1415.);
        assert_eq!(
            centered(launcher, display),
            frame(1920. + 800., 25. + 387.5, 960., 640.)
        );
        // too small to hold it: pinned to the corner, not pushed off it
        let small = frame(0., 0., 800., 600.);
        assert_eq!(centered(launcher, small).origin, point(px(0.), px(0.)));
    }
}
