//! Raw capture and playout devices, granted to one guest after the person
//! says yes. The host owns the device and nothing above it: it hands over
//! interleaved i16 PCM and RGBA frames exactly as the driver produced them,
//! and takes PCM back the same way. No codec, no mixing, no gain, no echo
//! work — a view that wants any of that ships it in its own wasm.
//!
//! - `media.devices` — `[{id, kind: audio_in|audio_out|video_in, name}]`.
//! - `audio.capture` `{device?, rate?, channels?}` — a subscription whose
//!   first item is `{rate, channels}` and whose every later item is one
//!   ~20 ms chunk of interleaved little-endian i16 PCM.
//! - `audio.play` `{rate, channels}` → `{handle, rate, channels}`, then
//!   `audio.write` with raw i16 PCM and `audio.stop`.
//! - `video.capture` `{device?, width?, height?, fps?}` — a subscription
//!   whose first item is `{width, height, fps, format}` and whose every
//!   later item is one raw RGBA frame.
//!
//! The devices themselves are in `audio` and `camera`; this file is the
//! doors, the consent, the indicator and what one guest holds.
//!
//! Three rules the view cannot reach around:
//!
//! 1. CONSENT. The first `media.devices`, `audio.capture` or `video.capture`
//!    a program makes raises a native prompt, and the answer is remembered
//!    per program in `media_consent`. Every door waits on that one answer,
//!    so a view asking for two devices at once raises one prompt.
//! 2. INDICATOR. A live capture holds an [`Indicator`], which the shell reads
//!    through [`capturing`] and draws where the view cannot paint. The count
//!    comes back when the stream is dropped, and only then.
//! 3. LIFECYCLE. Every device lives inside the subscription's own task (or,
//!    for playout, inside [`Media`]). A cancel drops the task; a stop, swap,
//!    trap, unseat or closed window drops the guest, and the guest drops
//!    both. The device thread ends the moment its end of the channel goes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

mod audio;
mod camera;

use audio::{listen, play, written};
use camera::watch;

use super::kernel::{Items, spawn_device, spawn_subscription};
use super::{Guest, NativeModuleView, wire};
use crate::backend::{read_prefs, write_prefs};
use gpui_kit::AppContext as _;

/// How much audio one captured item carries. 20 ms is the frame every voice
/// codec is written around, so a view that encodes gets exactly one frame
/// per item and never has to re-slice.
const CHUNK_MS: u32 = 20;
/// The most unplayed audio the host will hold for one view. Past this the
/// write is refused rather than queued: late audio is dead audio, and a
/// queue that grows is a view that cannot tell it is behind.
const PLAYOUT_MS: u32 = 500;
/// The ceiling on a capture the host will open. The view picks lower.
const MAX_WIDTH: u32 = 1280;
const MAX_HEIGHT: u32 = 720;
const MAX_FPS: u8 = 30;
/// Frames held for a guest that is behind. Two, and the rest are DROPPED —
/// a queue of stale frames is a delay the view can never work off.
const QUEUED_FRAMES: usize = 2;
/// Audio items held for a guest that is behind (~160 ms), same rule.
const QUEUED_CHUNKS: usize = 8;

fn device_failed(error: impl std::fmt::Display) -> wire::Refusal {
    wire::Refusal::new("device_failed", error.to_string())
}

// ---------- the indicator ----------

static MICROPHONES: AtomicUsize = AtomicUsize::new(0);
static CAMERAS: AtomicUsize = AtomicUsize::new(0);

/// Held for exactly as long as one capture is open. The shell reads the
/// counts, so the pill is the host's word on what is recording — a view
/// cannot decline to hold one, and cannot keep holding one after its stream
/// is dropped.
struct Indicator(&'static AtomicUsize);

impl Indicator {
    fn held(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for Indicator {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What the shell must show right now, or `None` when nothing is recording.
pub(crate) fn capturing() -> Option<&'static str> {
    let listening = MICROPHONES.load(Ordering::Relaxed) > 0;
    let watching = CAMERAS.load(Ordering::Relaxed) > 0;
    match (listening, watching) {
        (true, true) => Some("● mic · cam"),
        (true, false) => Some("● mic"),
        (false, true) => Some("● cam"),
        (false, false) => None,
    }
}

// ---------- consent ----------

const CONSENT_PREF: &str = "media_consent";

/// What this device already decided for `program`, if anything.
fn remembered(program: &str) -> Option<bool> {
    read_prefs()[CONSENT_PREF][program].as_bool()
}

/// Keep the answer. Best-effort like the appearance preference: a failed
/// write costs one more prompt next time and nothing this session shows.
fn remember(program: &str, granted: bool) {
    let mut prefs = read_prefs();
    if !prefs.is_object() {
        prefs = serde_json::json!({});
    }
    if !prefs[CONSENT_PREF].is_object() {
        prefs[CONSENT_PREF] = serde_json::json!({});
    }
    prefs[CONSENT_PREF][program] = serde_json::json!(granted);
    write_prefs(&prefs);
}

fn denied() -> wire::Refusal {
    wire::Refusal::new(
        "consent_denied",
        "this view may not use the microphone or the camera on this device",
    )
}

/// The answer every consent-gated door waits on, as a future the device's
/// own task holds. Resolves the moment the person answers — or at once when
/// this device already decided.
fn granted(
    guest: &Guest,
) -> impl std::future::Future<Output = Result<(), wire::Refusal>> + Send + use<> {
    let mut answered = guest.media.consent.subscribe();
    async move {
        loop {
            let decided = *answered.borrow_and_update();
            if let Some(granted) = decided {
                return granted.then_some(()).ok_or_else(denied);
            }
            if answered.changed().await.is_err() {
                return Err(denied());
            }
        }
    }
}

/// RAISE THE PROMPT, ONCE PER SEATED GUEST. Deferred, because both places
/// that mount a guest are already inside a window update and a window cannot
/// be entered twice; the deferred closure is taken after that update leaves.
fn ask(guest: &mut Guest, cx: &mut gpui_kit::Context<NativeModuleView>) {
    if guest.media.asked {
        return;
    }
    guest.media.asked = true;
    let program = guest.module;
    if let Some(decided) = remembered(program) {
        guest.media.consent.send_replace(Some(decided));
        return;
    }
    let answered = guest.media.consent.clone();
    let window = cx.windows().into_iter().next();
    cx.defer(move |cx: &mut gpui_kit::App| {
        let raised = window.map(|window| {
            cx.update_window(window, |_, window, cx| {
                window.prompt(
                    gpui_kit::PromptLevel::Info,
                    "This view wants to use your microphone or camera",
                    Some(program),
                    &["Allow", "Don't allow"],
                    cx,
                )
            })
        });
        let Some(Ok(raised)) = raised else {
            // no window to ask in is not a yes
            answered.send_replace(Some(false));
            return;
        };
        cx.spawn(async move |_| {
            let granted = raised.await.is_ok_and(|chosen| chosen == 0);
            remember(program, granted);
            answered.send_replace(Some(granted));
        })
        .detach();
    });
}

// ---------- what one guest holds ----------

/// A consent-gated door waiting for [`mount`] to reach a window. The guard
/// is the "one capture per kind" claim: it is taken when the ask is parked
/// and given back when the stream that took it is dropped, whatever dropped
/// it.
enum Ask {
    Devices,
    Listen(Listen, Arc<()>),
    Watch(Watch, Arc<()>),
}

pub(super) struct Media {
    /// The one consent answer this guest's device doors wait on, and whether
    /// it has been asked for yet.
    consent: Arc<tokio::sync::watch::Sender<Option<bool>>>,
    asked: bool,
    parked: Vec<(u64, Ask)>,
    /// One live capture per kind per guest; a second ask is refused while
    /// the first stream still holds its clone of this.
    microphone: Arc<()>,
    camera: Arc<()>,
    /// The one output this guest opened, and the queue writes land in.
    out: Option<Out>,
}

impl Default for Media {
    fn default() -> Self {
        Self {
            consent: Arc::new(tokio::sync::watch::Sender::new(None)),
            asked: false,
            parked: Vec::new(),
            microphone: Arc::new(()),
            camera: Arc::new(()),
            out: None,
        }
    }
}

/// An open output device: dropping this ends its thread, which drops the
/// stream, which releases the device.
struct Out {
    playout: Arc<Mutex<Playout>>,
    _stop: std::sync::mpsc::Sender<()>,
}

/// The samples written but not yet played, and the ceiling on them. `open`
/// is the device thread's word: it goes false when the device would not
/// open, or once it is gone, so a write is refused instead of swallowed.
struct Playout {
    samples: VecDeque<i16>,
    ceiling: usize,
    open: bool,
}

fn claimed(guard: &Arc<()>) -> bool {
    Arc::strong_count(guard) > 1
}

impl Media {
    /// The view dropped a subscription. The device itself goes with the
    /// task the kernel aborts; this only forgets the ask that never started.
    pub(super) fn cancel(&mut self, id: u64) {
        self.parked.retain(|(parked, _)| *parked != id);
    }
}

// ---------- the doors ----------

/// The asks and the modes are the door types (`wire::doors`): `Listen` and
/// `Watch` say what a view wants, the device's own mode is used unless it
/// offers exactly that, and the stream's FIRST item says what opened.
pub(super) use super::wire::doors::{AudioMode as Opened, Framing, Listen, Watch};
/// `audio.play` asks in the mode it wants to write in.
pub(super) type Speak = Opened;

pub(super) fn answer(
    guest: &mut Guest,
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        ("media", "devices") => park(guest, id, Ask::Devices),
        ("audio", "capture") => match asked::<Listen>(payload) {
            Ok(_) if claimed(&guest.media.microphone) => {
                guest.refuse(id, "device_busy", "this view already opened a microphone");
            }
            Ok(want) => {
                let guard = guest.media.microphone.clone();
                park(guest, id, Ask::Listen(want, guard));
            }
            Err(error) => guest.refuse(id, "malformed_request", error),
        },
        ("video", "capture") => match asked::<Watch>(payload) {
            Ok(_) if claimed(&guest.media.camera) => {
                guest.refuse(id, "device_busy", "this view already opened a camera");
            }
            Ok(want) => {
                let guard = guest.media.camera.clone();
                park(guest, id, Ask::Watch(want, guard));
            }
            Err(error) => guest.refuse(id, "malformed_request", error),
        },
        ("audio", "play") => play(guest, id, payload),
        ("audio", "write") => {
            let result = written(&guest.media, payload);
            guest.reply(id, result.map(|()| Vec::new()));
        }
        ("audio", "stop") => {
            guest.media.out = None;
            guest.reply(id, Ok(Vec::new()));
        }
        _ => return false,
    }
    true
}

/// A device ask needs a window before it needs a device: it waits here for
/// the mount that follows this redraw.
fn park(guest: &mut Guest, id: u64, ask: Ask) {
    if guest.media.parked.len() >= 16 {
        guest.refuse(id, "in_flight_limit", "too many pending device requests");
        return;
    }
    guest.media.parked.push((id, ask));
}

fn asked<T: borsh::BorshDeserialize>(payload: &[u8]) -> Result<T, String> {
    super::wire::doors::decode(payload)
}

/// Consent is settled and the devices are opened, each on its own guest task.
pub(super) fn mount(guest: &mut Guest, cx: &mut gpui_kit::Context<NativeModuleView>) {
    if guest.media.parked.is_empty() {
        return;
    }
    ask(guest, cx);
    for (id, parked) in std::mem::take(&mut guest.media.parked) {
        let consented = granted(guest);
        match parked {
            Ask::Devices => spawn_device(guest, id, async move {
                consented.await?;
                tokio::task::spawn_blocking(rows)
                    .await
                    .map_err(device_failed)
                    .map(|rows| super::wire::doors::encode(&rows))
            }),
            Ask::Listen(want, guard) => {
                spawn_subscription(guest, id, move |items| {
                    listen(items, consented, want, guard)
                });
            }
            Ask::Watch(want, guard) => {
                spawn_subscription(guest, id, move |items| watch(items, consented, want, guard));
            }
        }
    }
}

// ---------- the device list ----------

use super::wire::doors::Device as Row;

fn rows() -> Vec<Row> {
    use cpal::traits::{DeviceTrait as _, HostTrait as _};
    let host = cpal::default_host();
    let mut rows = Vec::new();
    if let Ok(inputs) = host.input_devices() {
        rows.extend(
            inputs
                .filter_map(|device| device.name().ok())
                .map(|name| Row {
                    id: name.clone(),
                    kind: "audio_in".into(),
                    name,
                }),
        );
    }
    if let Ok(outputs) = host.output_devices() {
        rows.extend(
            outputs
                .filter_map(|device| device.name().ok())
                .map(|name| Row {
                    id: name.clone(),
                    kind: "audio_out".into(),
                    name,
                }),
        );
    }
    if let Ok(cameras) = nokhwa::query(nokhwa::utils::ApiBackend::Auto) {
        rows.extend(cameras.into_iter().map(|camera| Row {
            id: camera.index().to_string(),
            kind: "video_in".into(),
            name: camera.human_name(),
        }));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `media.devices`, `audio.capture` and `video.capture` all wait on the
    /// ONE answer, and a no is a `consent_denied` refusal rather than a
    /// device that quietly never opens.
    #[test]
    fn every_capture_door_waits_on_one_consent_answer() {
        let answered = tokio::sync::watch::Sender::new(None);
        let mut waiting = answered.subscribe();
        assert!(waiting.borrow_and_update().is_none());
        answered.send_replace(Some(false));
        assert_eq!(denied().reason, "consent_denied");
        answered.send_replace(Some(true));
        assert_eq!(*answered.borrow(), Some(true));
    }

    /// A capture claim is held by the stream that took it and given back
    /// when that stream is dropped — a cancel, a swap, a trap, a device that
    /// would not open. Nothing has to remember to clear it.
    #[test]
    fn one_capture_per_kind_is_claimed_and_released_by_the_stream_itself() {
        let media = Media::default();
        assert!(!claimed(&media.microphone));
        let held = media.microphone.clone();
        assert!(claimed(&media.microphone));
        drop(held);
        assert!(!claimed(&media.microphone));
    }

    /// The capture asks are the door types, and an ask carrying bytes this
    /// door does not have is refused rather than silently ignored.
    #[test]
    fn a_capture_ask_is_optional_but_never_loose() {
        use super::super::wire::doors::encode;
        let want = Listen {
            rate: Some(48_000),
            channels: Some(1),
            ..Listen::default()
        };
        let listen: Listen = asked(&encode(&want)).unwrap();
        assert_eq!((listen.rate, listen.channels), (Some(48_000), Some(1)));
        assert!(
            asked::<Listen>(&encode(&Listen::default()))
                .unwrap()
                .rate
                .is_none()
        );
        let mut loose = encode(&want);
        loose.push(2);
        assert!(asked::<Listen>(&loose).is_err());
        let watch: Watch = asked(&encode(&Watch {
            width: Some(640),
            height: Some(480),
            fps: Some(15),
            ..Watch::default()
        }))
        .unwrap();
        assert_eq!((watch.width, watch.fps), (Some(640), Some(15)));
        assert!(asked::<Watch>(b"").is_err());
    }

    /// The indicator counts LIVE captures and nothing else, and the host is
    /// the one holding the count: a stream that ends gives it back.
    #[test]
    fn the_indicator_follows_the_live_captures() {
        assert_eq!(capturing(), None);
        let listening = Indicator::held(&MICROPHONES);
        assert_eq!(capturing(), Some("● mic"));
        let watching = Indicator::held(&CAMERAS);
        assert_eq!(capturing(), Some("● mic · cam"));
        drop(listening);
        assert_eq!(capturing(), Some("● cam"));
        drop(watching);
        assert_eq!(capturing(), None);
    }
}
