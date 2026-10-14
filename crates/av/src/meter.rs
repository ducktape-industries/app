//! A microphone's level drawn into gpui: a bar from the left, redrawn by
//! the meter alone (its view never draws for it), and the recording and
//! the monitor it runs.
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui_kit::*;

use crate::backend::{self, Ask, MicFeed};

/// One microphone, open from [`Meter::new`] until dropped. Emits its
/// [`crate::State`].
pub struct Meter {
    feed: Arc<MicFeed>,
    /// The bar's colour.
    color: Hsla,
    /// The level drawn: up at once, down slowly, so a word is seen.
    shown: f32,
    speaking: Speaking,
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
            speaking: Speaking::default(),
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
        *self.feed.ask.lock().expect("feed ask") = Some(Ask::Record(seconds.max(1)));
    }

    /// Ends a recording or its playback.
    pub fn stop(&self) {
        *self.feed.ask.lock().expect("feed ask") = Some(Ask::Stop);
    }

    /// Plays the test chime on the speaker; replaces a recording or a
    /// playback under way.
    pub fn chime(&self) {
        *self.feed.ask.lock().expect("feed ask") = Some(Ask::Chime);
    }

    pub fn state(&self) -> &crate::State {
        &self.told
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let (status, device) = self.feed.status.lock().expect("feed status").clone();
        let playback = *self.feed.playback.lock().expect("feed playback");
        let level = self.feed.level();
        let speaking = match status {
            crate::Status::Live => self.speaking.heard(level, std::time::Instant::now()),
            _ => {
                self.speaking = Speaking::default();
                false
            }
        };
        let state = crate::State {
            status,
            device,
            playback,
            speaking,
        };
        if state != self.told {
            self.told = state.clone();
            cx.emit(state);
        }
        let next = level.max(self.shown * 0.85);
        if (next - self.shown).abs() > 0.004 {
            self.shown = next;
            cx.notify();
        }
    }
}

/// "Speaking", from the level with hysteresis: on after the level has held
/// over -40 dBFS for 150 ms, off after 600 ms under it, so a breath between
/// words does not flicker it and a click does not raise it.
#[derive(Default)]
struct Speaking {
    on: bool,
    /// Since when the level has said the other thing.
    crossed: Option<std::time::Instant>,
}

/// -40 dBFS on the feed's 0..1 scale (-60..0 dBFS).
const SPEAKING_LEVEL: f32 = 20. / 60.;
const SPEAKING_ON: std::time::Duration = std::time::Duration::from_millis(150);
const SPEAKING_OFF: std::time::Duration = std::time::Duration::from_millis(600);

impl Speaking {
    fn heard(&mut self, level: f32, now: std::time::Instant) -> bool {
        let loud = level >= SPEAKING_LEVEL;
        if loud == self.on {
            self.crossed = None;
            return self.on;
        }
        let since = *self.crossed.get_or_insert(now);
        let hold = if loud { SPEAKING_ON } else { SPEAKING_OFF };
        if now.duration_since(since) >= hold {
            self.on = loud;
            self.crossed = None;
        }
        self.on
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

#[cfg(test)]
mod tests {
    use super::Speaking;
    use std::time::{Duration, Instant};

    /// Speaking rises after 150 ms over -40 dBFS, holds through a short
    /// pause, and falls after 600 ms under it; a click raises nothing.
    #[test]
    fn speaking_has_hysteresis() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut speaking = Speaking::default();
        // a click: loud for 100 ms only
        assert!(!speaking.heard(0.8, at(0)));
        assert!(!speaking.heard(0.8, at(100)));
        assert!(!speaking.heard(0.1, at(133)));
        // a word: loud from 200 ms on
        assert!(!speaking.heard(0.5, at(200)));
        assert!(speaking.heard(0.5, at(350)));
        // a breath of 400 ms between words keeps it
        assert!(speaking.heard(0.1, at(400)));
        assert!(speaking.heard(0.1, at(799)));
        assert!(speaking.heard(0.5, at(800)));
        // silence: off 600 ms after it began
        assert!(speaking.heard(0.1, at(900)));
        assert!(speaking.heard(0.1, at(1499)));
        assert!(!speaking.heard(0.1, at(1500)));
    }
}
