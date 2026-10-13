//! A microphone's level drawn into gpui: a bar from the left, redrawn by
//! the meter alone (its view never draws for it), and the recording and
//! the monitor it runs.
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::*;

use crate::backend::{self, MicFeed};

/// One microphone, open from [`Meter::new`] until dropped. Emits its
/// [`crate::State`].
pub struct Meter {
    feed: Arc<MicFeed>,
    /// The bar's colour.
    color: Hsla,
    /// The level drawn: up at once, down slowly, so a word is seen.
    shown: f32,
    told: crate::State,
    _poll: Task<()>,
}

impl EventEmitter<crate::State> for Meter {}

impl Meter {
    /// Opens `device` (an input [`crate::audio_devices`] listed; `None`:
    /// the default), recordings and the monitor playing on `output` (an
    /// output it listed; `None`: the default).
    pub fn new(
        device: Option<String>,
        output: Option<String>,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> Self {
        let feed = Arc::new(MicFeed::default());
        backend::start_microphone(device, output, feed.clone());
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
            color,
            shown: 0.,
            told: crate::State::default(),
            _poll,
        }
    }

    pub fn set_color(&mut self, color: Hsla, cx: &mut Context<Self>) {
        if self.color != color {
            self.color = color;
            cx.notify();
        }
    }

    /// Plays what the microphone hears straight back while on.
    pub fn set_monitor(&self, monitor: bool) {
        self.feed.monitor.store(monitor, Ordering::Relaxed);
    }

    /// Records `seconds`, then plays the recording back; replaces one
    /// under way.
    pub fn record(&self, seconds: u8) {
        *self.feed.ask.lock().expect("feed ask") = Some(seconds.max(1));
    }

    /// Ends a recording or its playback.
    pub fn stop(&self) {
        *self.feed.ask.lock().expect("feed ask") = Some(0);
    }

    pub fn state(&self) -> &crate::State {
        &self.told
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let (status, device) = self.feed.status.lock().expect("feed status").clone();
        let playback = *self.feed.playback.lock().expect("feed playback");
        let state = crate::State {
            status,
            device,
            playback,
        };
        if state != self.told {
            self.told = state.clone();
            cx.emit(state);
        }
        let level = self.feed.level();
        let next = level.max(self.shown * 0.85);
        if (next - self.shown).abs() > 0.004 {
            self.shown = next;
            cx.notify();
        }
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.feed.stop.store(true, Ordering::Relaxed);
    }
}

impl Render for Meter {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().h_full().w(relative(self.shown)).bg(self.color))
    }
}
