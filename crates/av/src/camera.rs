//! A camera's pictures drawn into gpui: each picture its camera's thread
//! leaves becomes a `RenderImage` (the web page's paint path), fitted into
//! the element's box as an image is (gpui's `ObjectFit`), cut at its edges.
//!
//! Pictures are pulled by drawing: the thread converts a camera frame only
//! while the piece is drawn ([`CameraFeed::wanted`]). A redraw asked for
//! that never came (the window hidden or minimised: gpui draws nothing
//! then) turns the conversion off until the next draw turns it on again.
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::*;

use crate::backend::{self, CameraFeed};

/// One camera, open from [`Camera::new`] until dropped. Emits its
/// [`crate::State`].
pub struct Camera {
    feed: Arc<CameraFeed>,
    shown: Option<Arc<RenderImage>>,
    fit: ObjectFit,
    /// A redraw was asked for a waiting picture and has not come yet.
    asked: bool,
    told: crate::State,
    _poll: Task<()>,
}

impl EventEmitter<crate::State> for Camera {}

impl Camera {
    /// Opens `device` (an id [`crate::cameras`] listed; `None`: the
    /// default), its pictures flipped when `mirror` and fitted as `fit`.
    pub fn new(
        device: Option<String>,
        mirror: bool,
        fit: ObjectFit,
        cx: &mut Context<Self>,
    ) -> Self {
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

    pub fn set_fit(&mut self, fit: ObjectFit, cx: &mut Context<Self>) {
        if std::mem::discriminant(&self.fit) != std::mem::discriminant(&fit) {
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
        let fit = again(&self.fit);
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let Some(image) = image else { return };
                let at = fit.get_bounds(bounds, image.size(0));
                // a covering picture is cut at the box's edges
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let _ = window.paint_image(at, at, Corners::default(), image, 0, false);
                });
            },
        )
        .size_full()
    }
}

/// `fit` once more: gpui's `ObjectFit` is not `Clone`.
fn again(fit: &ObjectFit) -> ObjectFit {
    match fit {
        ObjectFit::Fill => ObjectFit::Fill,
        ObjectFit::Contain => ObjectFit::Contain,
        ObjectFit::Cover => ObjectFit::Cover,
        ObjectFit::ScaleDown => ObjectFit::ScaleDown,
        ObjectFit::None => ObjectFit::None,
    }
}
