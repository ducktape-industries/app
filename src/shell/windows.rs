//! Where an OS window goes: a pop-out off its source window (`cascade`,
//! `unseated`), the launcher centred (`centered`); and how one leaves the
//! screen (`remove`). Opening one is `entities::Windows`'.

use super::layout;
use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};

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
    let origin = point(source.origin.x + px(CASCADE), source.origin.y + px(CASCADE));
    inside(origin, extent, display)
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
/// past the console's chrome (`inset`: under the bar, right of the
/// sidebar) rather than over it, at least `min_w` wide ([`popout_min`]),
/// and inside `display`. With no place on the desk to keep, it cascades.
pub(in crate::shell) fn unseated(
    source: Bounds<Pixels>,
    inset: super::layers::Inset,
    frame: Option<layout::Frame>,
    display: Option<Bounds<Pixels>>,
    min_w: f32,
) -> Bounds<Pixels> {
    let Some(frame) = frame else {
        return cascade(source, display);
    };
    let extent = size(px(frame.w.max(min_w)), px(frame.h.max(POPOUT_MIN)));
    let origin = point(
        source.origin.x + px(inset.left + frame.x),
        source.origin.y + px(inset.top + frame.y),
    );
    inside(origin, extent, display)
}

/// `extent` at `origin`, pulled back inside `display` where it would hang
/// off an edge, and never past its top-left corner.
fn inside(
    mut origin: Point<Pixels>,
    extent: Size<Pixels>,
    display: Option<Bounds<Pixels>>,
) -> Bounds<Pixels> {
    if let Some(display) = display {
        let right = display.origin.x + display.size.width - extent.width;
        let bottom = display.origin.y + display.size.height - extent.height;
        origin.x = origin.x.min(right).max(display.origin.x);
        origin.y = origin.y.min(bottom).max(display.origin.y);
    }
    Bounds::new(origin, extent)
}

/// Lets go of a window's input: the keys, and whatever its last draw
/// held.
pub(in crate::shell) fn release_window_input(
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) {
    window.blur(cx);
    window.draw(cx).clear(cx);
}

/// Takes a window off the screen, its input let go first. Deferred:
/// releasing input draws the window, and the caller is most often in the
/// middle of updating it. `Windows::forget_closed` then forgets it.
pub(in crate::shell) fn remove(window: gpui_kit::AnyWindowHandle, cx: &mut gpui_kit::App) {
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            release_window_input(window, cx);
            window.remove_window();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn a_window_leaving_the_desk_opens_where_it_sat_under_the_bar() {
        use crate::backend::Layout;
        use crate::shell::layers::chrome_inset;
        let bar = chrome_inset(Layout::MenuBar);
        let seat = layout::Frame {
            x: 100.,
            y: 50.,
            w: 900.,
            h: 600.,
        };
        let display = frame(0., 0., 2560., 1440.);
        let at = unseated(
            frame(200., 100., 1280., 800.),
            bar,
            Some(seat),
            Some(display),
            POPOUT_MIN,
        );
        assert_eq!(at, frame(300., 100. + bar.top + 50., 900., 600.));
        assert!(
            at.origin.y >= px(100. + bar.top),
            "the console's bar stays uncovered"
        );
        // the sidebar's layout: right of the column, at the window's top
        let side = chrome_inset(Layout::Sidebar);
        let at = unseated(
            frame(200., 100., 1280., 800.),
            side,
            Some(seat),
            Some(display),
            POPOUT_MIN,
        );
        assert_eq!(at, frame(200. + side.left + 100., 150., 900., 600.));
        // no narrower than its view: a 500 px frame of a view laid out from 680
        let narrow = layout::Frame { w: 500., ..seat };
        let at = unseated(
            frame(200., 100., 1280., 800.),
            bar,
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

    /// A pop-out steps 32 px off its source window, unless that would
    /// take it off the display: then it stays where the source is.
    #[test]
    fn a_popout_cascades_off_its_source_and_stays_on_the_display() {
        let display = frame(0., 0., 1600., 1000.);
        let small = frame(0., 0., 1024., 700.);
        for (source, on, at) in [
            (frame(160., 100., 1280., 800.), display, (192., 132.)),
            (frame(320., 200., 1280., 800.), display, (320., 200.)),
            (frame(0., 0., 1024., 700.), small, (0., 0.)),
        ] {
            assert_eq!(
                cascade(source, Some(on)).origin,
                point(px(at.0), px(at.1)),
                "{source:?} on {on:?}"
            );
        }
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
