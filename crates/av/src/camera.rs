//! A camera's pictures drawn into gpui: each picture its camera's thread
//! leaves becomes a `RenderImage` (the web page's paint path), drawn whole
//! and centred in the element's box, or covering it cut centred.
//!
//! Pictures are pulled by drawing: the thread converts a camera frame only
//! while the piece is drawn ([`CameraFeed::wanted`]). A redraw asked for
//! that never came (the window hidden or minimised: gpui draws nothing
//! then) turns the conversion off until the next draw turns it on again.
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::*;

use crate::backend::{self, CameraFeed};

/// How a picture fills the piece's box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// whole, centred
    #[default]
    Contain,
    /// the box covered, the picture cut centred
    Cover,
}

/// One camera, open from [`Camera::new`] until dropped. Emits its
/// [`crate::State`].
pub struct Camera {
    feed: Arc<CameraFeed>,
    shown: Option<Arc<RenderImage>>,
    fit: Fit,
    /// A redraw was asked for a waiting picture and has not come yet.
    asked: bool,
    told: crate::State,
    _poll: Task<()>,
}

impl EventEmitter<crate::State> for Camera {}

impl Camera {
    /// Opens `device` (an id [`crate::cameras`] listed; `None`: the
    /// default), its pictures flipped when `mirror`.
    pub fn new(device: Option<String>, mirror: bool, fit: Fit, cx: &mut Context<Self>) -> Self {
        let feed = Arc::new(CameraFeed::default());
        feed.mirror.store(mirror, Ordering::Relaxed);
        feed.wanted.store(true, Ordering::Relaxed);
        backend::start_camera(device, feed.clone());
        let _poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::POLL).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            feed,
            shown: None,
            fit,
            asked: false,
            told: crate::State::default(),
            _poll,
        }
    }

    /// Flips the pictures from the next one on.
    pub fn set_mirror(&self, mirror: bool) {
        self.feed.mirror.store(mirror, Ordering::Relaxed);
    }

    pub fn set_fit(&mut self, fit: Fit, cx: &mut Context<Self>) {
        if self.fit != fit {
            self.fit = fit;
            cx.notify();
        }
    }

    /// The state as last emitted.
    pub fn state(&self) -> &crate::State {
        &self.told
    }

    /// A new state emitted; a redraw asked when a picture waits. Asked
    /// once and not drawn since: nothing draws the piece, so the thread
    /// stops converting until a draw comes.
    fn poll(&mut self, cx: &mut Context<Self>) {
        let (status, device) = self.feed.status.lock().expect("feed status").clone();
        let state = crate::State {
            status,
            device,
            ..Default::default()
        };
        if state != self.told {
            self.told = state.clone();
            cx.emit(state);
            cx.notify();
        }
        if self.feed.picture.lock().expect("feed picture").is_some() {
            match self.asked {
                false => {
                    self.asked = true;
                    cx.notify();
                }
                true => self.feed.wanted.store(false, Ordering::Relaxed),
            }
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.feed.stop.store(true, Ordering::Relaxed);
    }
}

impl Render for Camera {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // drawn: the pictures are wanted again
        self.asked = false;
        self.feed.wanted.store(true, Ordering::Relaxed);
        // the newest picture in, the one it replaces out of the atlas
        let fresh = self.feed.picture.lock().expect("feed picture").take();
        if let Some(picture) = fresh
            && let Some(pixels) =
                image::ImageBuffer::from_raw(picture.width, picture.height, picture.bgra)
        {
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
            if let Some(old) = self.shown.replace(image) {
                let _ = window.drop_image(old);
            }
        }
        let image = self.shown.clone();
        let fit = self.fit;
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let Some(image) = image else { return };
                let size = image.size(0);
                let (width, height) = (size.width.0 as f32, size.height.0 as f32);
                let at = match fit {
                    Fit::Contain => contained(bounds, width, height),
                    Fit::Cover => covered(bounds, width, height),
                };
                // a covering picture is cut at the box's edges
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let _ = window.paint_image(at, at, Corners::default(), image, 0, false);
                });
            },
        )
        .size_full()
    }
}

/// The largest box of `width` by `height`'s shape inside `bounds`, centred.
pub(crate) fn contained(bounds: Bounds<Pixels>, width: f32, height: f32) -> Bounds<Pixels> {
    let (box_w, box_h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    if width <= 0. || height <= 0. || box_w <= 0. || box_h <= 0. {
        return bounds;
    }
    let scale = (box_w / width).min(box_h / height);
    let (w, h) = (width * scale, height * scale);
    Bounds::new(
        bounds.origin + point(px((box_w - w) / 2.), px((box_h - h) / 2.)),
        size(px(w), px(h)),
    )
}

/// The smallest box of `width` by `height`'s shape that covers `bounds`,
/// centred on it: the parts past its edges are cut.
pub(crate) fn covered(bounds: Bounds<Pixels>, width: f32, height: f32) -> Bounds<Pixels> {
    let (box_w, box_h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    if width <= 0. || height <= 0. || box_w <= 0. || box_h <= 0. {
        return bounds;
    }
    let scale = (box_w / width).max(box_h / height);
    let (w, h) = (width * scale, height * scale);
    Bounds::new(
        bounds.origin + point(px((box_w - w) / 2.), px((box_h - h) / 2.)),
        size(px(w), px(h)),
    )
}

#[cfg(test)]
mod tests {
    use super::{contained, covered};
    use gpui_kit::{Bounds, Pixels, point, px, size};

    /// A wide picture in a tall box fills the width, centred top to bottom.
    #[test]
    fn a_picture_is_letterboxed() {
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(100.), px(200.)));
        let at = contained(bounds, 1280., 720.);
        assert_eq!(at.size.width, px(100.));
        assert_eq!(at.size.height, px(56.25));
        assert_eq!(at.origin, point(px(10.), px(20. + (200. - 56.25) / 2.)));
        let tall = contained(bounds, 10., 40.);
        assert_eq!(tall.size, size(px(50.), px(200.)));
        assert_eq!(tall.origin.x, px(35.));
    }

    /// A wide picture in a tall box fills the height, cut at the sides,
    /// centred; a box of the picture's own shape is filled exactly.
    #[test]
    fn a_picture_covers_its_tile() {
        let near = |a: Pixels, b: f32| (a.as_f32() - b).abs() < 0.01;
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(100.), px(200.)));
        let at = covered(bounds, 1280., 720.);
        assert!(
            near(at.size.height, 200.) && near(at.origin.y, 20.),
            "{at:?}"
        );
        assert!(near(at.size.width, 355.555_56), "{at:?}");
        assert!(near(at.origin.x, 10. - (355.555_56 - 100.) / 2.), "{at:?}");
        let same = Bounds::new(point(px(0.), px(0.)), size(px(160.), px(90.)));
        let at = covered(same, 1280., 720.);
        assert!(near(at.size.width, 160.) && near(at.size.height, 90.) && near(at.origin.x, 0.));
    }
}
