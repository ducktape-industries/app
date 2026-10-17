//! Calls: a camera or microphone piece that `publish`es into a live track,
//! and a remote piece (`wire::Node::Remote`) that plays one. The view
//! places the pieces; this host captures, encodes, publishes, subscribes,
//! decodes, draws and plays (media never crosses the view wire).
//!
//! With feature `call`:
//! - a publishing camera piece shares its device with its preview: the
//!   [`ducktape_av::Camera`] `av.rs` made draws, and a thread of ours reads
//!   the same feed's tap, encodes at the ladder's rung, chunks and pushes
//!   onto a host-owned publication (`runtime::HostLive`, stamped and
//!   granted as the view's own requests are); the published picture is
//!   never mirrored;
//! - a publishing microphone piece's meter is fed by the app's one voice
//!   I/O (`ducktape_av::voice`, echo-cancelled); a thread encodes its
//!   frames onto a datagram publication; `muted` drops the capture (the
//!   device released while no other holds it) and keeps the track up;
//! - a remote video piece is a `Camera::remote` whose feed a thread fills
//!   from the subscription (reassembled, decoded); a remote audio piece is
//!   a voice on the mixer, its packets moved by a thread, `speaking` from
//!   its level with the meter's hysteresis;
//! - every piece's `MediaState.track` comes from what its track heard,
//!   told through `on_state` on each change ([`Calls::poll`], 30 Hz).
//!
//! A view may have [`MAX_REMOTE`] remote pieces; its own devices' caps
//! are `av.rs`'s. Without `call`, a remote piece says the build has no
//! calls and `publish` publishes nothing.

use super::*;
#[cfg(feature = "call")]
use gpui_kit::{EntityId, Task};
#[cfg(feature = "call")]
use inner::{Publisher, Remote};

/// The pieces of a tree that publish or play a live track, by authored
/// path, and the poll that tells their states.
#[derive(Default)]
pub(super) struct Calls {
    #[cfg(feature = "call")]
    publishers: HashMap<AuthoredPath, Publisher>,
    #[cfg(feature = "call")]
    remotes: HashMap<AuthoredPath, Remote>,
    #[cfg(feature = "call")]
    _poll: Option<Task<()>>,
}

/// The most remote pieces a view has open at once: a room's stage and
/// everyone's voice.
#[cfg(feature = "call")]
pub(super) const MAX_REMOTE: usize = 16;

impl Calls {
    /// Keeps the pieces `root` still has; the others' tracks end.
    pub(super) fn retain(&mut self, root: &wire::Node) {
        #[cfg(feature = "call")]
        if !self.publishers.is_empty() || !self.remotes.is_empty() {
            let mut kept = std::collections::HashSet::new();
            commands::walk_authored_paths(root, None, &mut Vec::new(), &mut |node, path| {
                if matches!(
                    node,
                    wire::Node::Camera { .. }
                        | wire::Node::Microphone { .. }
                        | wire::Node::Remote { .. }
                ) {
                    kept.insert(path.clone());
                }
            });
            self.publishers.retain(|path, _| kept.contains(path));
            self.remotes.retain(|path, _| kept.contains(path));
        }
        let _ = root;
    }

    /// Every track ends: the view stopped, or its tree goes.
    pub(super) fn release(&mut self) {
        #[cfg(feature = "call")]
        {
            self.publishers.clear();
            self.remotes.clear();
            self._poll = None;
        }
    }

    /// `state` of the piece at `path`, as its track stands.
    #[cfg_attr(not(feature = "call"), allow(dead_code))]
    pub(super) fn adorn(&self, path: &AuthoredPath, state: &mut wire::MediaState) {
        #[cfg(feature = "call")]
        if let Some(publisher) = self.publishers.get(path) {
            publisher.adorn(state);
        }
        let _ = (path, state);
    }
}

impl ViewTree {
    /// A remote piece: the picture or the voice of someone in the room,
    /// or the line that says this build has no calls.
    pub(super) fn remote(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Remote { id, style, .. } = node else {
            unreachable!()
        };
        let element = div()
            .refine_style(&self.style(*style))
            .id(native_id(id))
            .overflow_hidden();
        #[cfg(feature = "call")]
        let element = element.children(self.remote_piece(node, cx));
        #[cfg(not(feature = "call"))]
        let element = {
            let _ = cx;
            element
                .flex()
                .items_center()
                .justify_center()
                .child("This build has no calls")
        };
        element.into_any_element()
    }
}

#[cfg(feature = "call")]
mod inner {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::backend::live::media::{
        AudioNumbering, Chunker, FRESH, Frame, GOP, Ladder, Reassembler, Rung,
    };
    use crate::backend::live::{Data, Mode, Object, Pushed, Start};
    use crate::runtime::{HostLive, HostPublication, Subscribed, TrackHeard};
    use ducktape_av::codec::{AudioEncoder, VideoDecoder, VideoEncoder};
    use ducktape_av::{CameraFeed, MicFeed, Speaking, Status, voice};
    use view_wire::methods::LiveTrack;

    /// A voice track's bitrate.
    const VOICE_BITRATE: u32 = 24_000;
    /// A video track's priority and a voice track's (SPEC D3).
    const VIDEO_PRIORITY: u8 = 128;
    const VOICE_PRIORITY: u8 = 64;
    /// How long a microphone thread waits for a frame before it looks at
    /// its track and its mute again.
    const MIC_WAIT: Duration = Duration::from_millis(100);
    const LESS_PICTURE: &str = "sending less picture";

    /// What a piece's thread says of its track (and, a remote piece's, of
    /// its playback).
    #[derive(Clone, Default, PartialEq)]
    pub(super) struct Standing {
        /// A remote piece's playback: Live once something played.
        pub(super) status: Status,
        pub(super) track: wire::TrackState,
        pub(super) detail: String,
        /// This build encodes or decodes nothing of the kind: `Unsupported`.
        pub(super) unsupported: bool,
    }

    /// A publishing piece's track, run by a thread, until dropped.
    pub(in crate::render) struct Publisher {
        track: LiveTrack,
        /// The device entity the thread reads: another means a new thread.
        entity: EntityId,
        standing: Arc<Mutex<Standing>>,
        told: Standing,
        muted: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    }

    impl Drop for Publisher {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    impl Publisher {
        pub(super) fn adorn(&self, state: &mut wire::MediaState) {
            let standing = lock(&self.standing);
            state.track = standing.track.clone();
            if standing.unsupported {
                state.status = wire::MediaStatus::Unsupported;
            }
            if !standing.detail.is_empty() {
                state.detail = standing.detail.clone();
            }
        }
    }

    /// A remote piece: what plays it, and its thread's word.
    pub(in crate::render) struct Remote {
        track: LiveTrack,
        plays: Plays,
        standing: Arc<Mutex<Standing>>,
        speaking: Speaking,
        told: Option<wire::MediaState>,
        on_state: Option<u32>,
        /// Ends the subscription, and with it the thread.
        _stopper: Option<tokio::task::AbortHandle>,
    }

    impl Drop for Remote {
        /// The piece goes: its subscription ends, and its thread with it.
        fn drop(&mut self) {
            if let Some(stopper) = self._stopper.take() {
                stopper.abort();
            }
        }
    }

    /// A remote audio piece's voice on the speaker, with the voice I/O it
    /// plays through, once its thread made them (`None` when this build
    /// decodes none).
    type Played = Arc<OnceLock<Option<(Arc<voice::Io>, Arc<voice::Voice>)>>>;

    enum Plays {
        Video(Entity<ducktape_av::Camera>),
        Audio(Played),
        /// Nothing: the view has [`MAX_REMOTE`] open.
        Refused,
    }

    impl Calls {
        /// A publishing camera piece at `path`: the publication, kept
        /// while the track and the camera stay; gone with `publish`.
        pub(in crate::render) fn camera(
            &mut self,
            path: &AuthoredPath,
            publish: Option<&LiveTrack>,
            camera: &Entity<ducktape_av::Camera>,
            live: Option<&HostLive>,
            cx: &mut Context<ViewTree>,
        ) {
            let Some((track, live)) = publish.zip(live) else {
                self.publishers.remove(path);
                return;
            };
            if self
                .publishers
                .get(path)
                .is_some_and(|held| held.track == *track && held.entity == camera.entity_id())
            {
                return;
            }
            self.publishers.remove(path);
            let feed = camera.read(cx).feed().clone();
            let publication = live.publish(track, Mode::Stream, VIDEO_PRIORITY);
            let publisher = Publisher::new(track.clone(), camera.entity_id());
            let (standing, stop) = (publisher.standing.clone(), publisher.stop.clone());
            spawn("call-camera", move || {
                publish_camera(feed, publication, standing, stop);
            });
            self.publishers.insert(path.clone(), publisher);
            self.ensure_poll(cx);
        }

        /// A publishing microphone piece at `path` (`node`): its meter fed
        /// by the voice I/O, its frames published; `muted` drops the
        /// capture.
        pub(in crate::render) fn microphone(
            &mut self,
            path: &AuthoredPath,
            node: &wire::Node,
            meter: &Entity<ducktape_av::Meter>,
            live: Option<&HostLive>,
            cx: &mut Context<ViewTree>,
        ) {
            let wire::Node::Microphone {
                device,
                output,
                publish,
                muted,
                ..
            } = node
            else {
                unreachable!()
            };
            let (muted, device, output) = (*muted, device.clone(), output.clone());
            let Some((track, live)) = publish.as_ref().zip(live) else {
                self.publishers.remove(path);
                return;
            };
            if let Some(held) = self
                .publishers
                .get(path)
                .filter(|held| held.track == *track && held.entity == meter.entity_id())
            {
                held.muted.store(muted, Ordering::Relaxed);
                return;
            }
            self.publishers.remove(path);
            let feed = meter.read(cx).feed().clone();
            let publication = live.publish(track, Mode::Datagram, VOICE_PRIORITY);
            let publisher = Publisher::new(track.clone(), meter.entity_id());
            publisher.muted.store(muted, Ordering::Relaxed);
            let (standing, stop, mute) = (
                publisher.standing.clone(),
                publisher.stop.clone(),
                publisher.muted.clone(),
            );
            spawn("call-microphone", move || {
                let io = voice::hold(device, output);
                publish_microphone(io, feed, publication, standing, stop, mute);
            });
            self.publishers.insert(path.clone(), publisher);
            self.ensure_poll(cx);
        }

        /// The poll that tells the pieces' states, while any piece lives.
        fn ensure_poll(&mut self, cx: &mut Context<ViewTree>) {
            if self._poll.is_some() {
                return;
            }
            self._poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let alive = this.update(cx, |tree, cx| tree.poll_calls(cx));
                    if !alive.unwrap_or(false) {
                        break;
                    }
                }
            }));
        }
    }

    impl Publisher {
        fn new(track: LiveTrack, entity: EntityId) -> Self {
            Self {
                track,
                entity,
                standing: Arc::default(),
                told: Standing::default(),
                muted: Arc::new(AtomicBool::new(false)),
                stop: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl ViewTree {
        /// A remote piece at this path: made for its track and kind, told
        /// its state through `on_state`.
        pub(in crate::render) fn remote_piece(
            &mut self,
            node: &wire::Node,
            cx: &mut Context<Self>,
        ) -> Option<Entity<ducktape_av::Camera>> {
            let wire::Node::Remote {
                track,
                media,
                on_state,
                ..
            } = node
            else {
                unreachable!()
            };
            let path = self.authored_path.clone();
            let video = matches!(media, wire::RemoteMedia::Video { .. });
            let again = self.calls.remotes.get(&path).is_none_or(|held| {
                held.track != *track
                    || matches!(held.plays, Plays::Video(_)) != video
                    || matches!(held.plays, Plays::Refused)
                        && self.calls.remotes.len() <= MAX_REMOTE
            });
            if again {
                self.calls.remotes.remove(&path);
                let open = self
                    .calls
                    .remotes
                    .values()
                    .filter(|held| !matches!(held.plays, Plays::Refused))
                    .count();
                let remote = match (open < MAX_REMOTE, self.live.clone()) {
                    (true, Some(live)) => make_remote(track, media, &live, cx),
                    _ => Remote::refused(track.clone()),
                };
                self.calls.remotes.insert(path.clone(), remote);
                self.calls.ensure_poll(cx);
            }
            let remote = self.calls.remotes.get_mut(&path).expect("made above");
            remote.on_state = *on_state;
            match &remote.plays {
                Plays::Video(camera) => {
                    let camera = camera.clone();
                    if let wire::RemoteMedia::Video { object_fit } = media {
                        let fit = super::super::pictures::primitive_object_fit(*object_fit);
                        camera.update(cx, |camera, cx| camera.set_fit(fit, cx));
                    }
                    Some(camera)
                }
                Plays::Audio(_) | Plays::Refused => None,
            }
        }

        /// Each piece's state told on change; `false` once no piece lives
        /// (the poll ends).
        fn poll_calls(&mut self, cx: &mut Context<Self>) -> bool {
            let now = Instant::now();
            let mut events = Vec::new();
            for (path, publisher) in &mut self.calls.publishers {
                let standing = lock(&publisher.standing).clone();
                if standing == publisher.told {
                    continue;
                }
                publisher.told = standing;
                // the device's own state, as its piece last emitted it,
                // with the track's
                if let Some((handler, mut state)) = self.media.last_told(path) {
                    publisher.adorn(&mut state);
                    events.push(wire::Event::Media { handler, state });
                }
            }
            for remote in self.calls.remotes.values_mut() {
                let standing = lock(&remote.standing).clone();
                let mut state = super::av::told(
                    &ducktape_av::State {
                        status: standing.status,
                        ..Default::default()
                    },
                    ducktape_av::Kind::Camera,
                );
                state.track = standing.track.clone();
                if standing.unsupported {
                    state.status = wire::MediaStatus::Unsupported;
                }
                if let Plays::Audio(played) = &remote.plays
                    && let Some(Some((io, voice))) = played.get()
                {
                    // the speaker that would not open is the piece's fault
                    if let Some((status, detail)) = speaker_fault(io.speaker()) {
                        state.status = status;
                        state.detail = detail;
                    }
                    let live = state.status == wire::MediaStatus::Live;
                    state.speaking = live && remote.speaking.heard(voice.level(), now);
                }
                if matches!(remote.plays, Plays::Refused) {
                    state.status = wire::MediaStatus::Failed;
                    state.detail = "This view has as many remote pieces open as it may.".into();
                }
                if remote.told.as_ref() != Some(&state) {
                    remote.told = Some(state.clone());
                    if let Some(handler) = remote.on_state {
                        events.push(wire::Event::Media { handler, state });
                    }
                }
            }
            for event in events {
                cx.emit(event);
            }
            let alive = !self.calls.publishers.is_empty() || !self.calls.remotes.is_empty();
            if !alive {
                self.calls._poll = None;
            }
            alive
        }
    }

    impl Remote {
        fn refused(track: LiveTrack) -> Self {
            Self {
                track,
                plays: Plays::Refused,
                standing: Arc::default(),
                speaking: Speaking::default(),
                told: None,
                on_state: None,
                _stopper: None,
            }
        }
    }

    /// A remote piece on `track`, its thread started.
    fn make_remote(
        track: &LiveTrack,
        media: &wire::RemoteMedia,
        live: &HostLive,
        cx: &mut Context<ViewTree>,
    ) -> Remote {
        let standing: Arc<Mutex<Standing>> = Arc::default();
        // a picture from the group the node holds (its keyframe); a voice
        // from the next packet, never a second of stale voice first
        let from = match media {
            wire::RemoteMedia::Video { .. } => Start::Latest,
            wire::RemoteMedia::Audio { .. } => Start::Next,
        };
        let mut subscription = live.subscribe(track, from);
        let stopper = subscription.stopper();
        let plays = match media {
            wire::RemoteMedia::Video { object_fit } => {
                let fit = super::super::pictures::primitive_object_fit(*object_fit);
                let mut feed = None;
                let camera = cx.new(|cx| {
                    let (camera, fed) = ducktape_av::Camera::remote(fit, cx);
                    feed = Some(fed);
                    camera
                });
                let feed = feed.expect("made above");
                let word = standing.clone();
                spawn("call-video", move || {
                    play_video(&mut subscription, feed, word);
                });
                Plays::Video(camera)
            }
            wire::RemoteMedia::Audio { output } => {
                let played: Played = Arc::default();
                let (word, heard, output) = (standing.clone(), played.clone(), output.clone());
                spawn("call-audio", move || {
                    // the voice I/O is made here, off the window thread
                    // (its echo canceller is the cost)
                    let io = voice::hold(None, output);
                    let voice = match io.voice() {
                        Ok(voice) => Some((io, Arc::new(voice))),
                        Err(_) => {
                            lock(&word).unsupported = true;
                            None
                        }
                    };
                    let voice = heard.get_or_init(|| voice).clone();
                    play_audio(&mut subscription, voice.map(|(_, voice)| voice), word);
                });
                Plays::Audio(played)
            }
        };
        Remote {
            track: track.clone(),
            plays,
            standing,
            speaking: Speaking::default(),
            told: None,
            on_state: None,
            _stopper: Some(stopper),
        }
    }

    fn spawn(name: &str, body: impl FnOnce() + Send + 'static) {
        if let Err(error) = std::thread::Builder::new().name(name.into()).spawn(body) {
            tracing::error!(target: "ducktape::call", %error, "{name} thread did not start");
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A speaker that is not playing, in the speaker's own words; `None`
    /// while it plays or is not asked yet.
    fn speaker_fault(status: Status) -> Option<(wire::MediaStatus, String)> {
        let said = |why: &str| -> String {
            why.chars()
                .filter(crate::runtime::plain)
                .take(200)
                .collect()
        };
        Some(match status {
            Status::Starting | Status::Live => return None,
            Status::Denied => (
                wire::MediaStatus::Denied,
                "The system refused the speaker.".into(),
            ),
            Status::Busy => (
                wire::MediaStatus::Busy,
                "Another app is using the speaker.".into(),
            ),
            Status::NoDevice => (wire::MediaStatus::NoDevice, "No speaker found.".into()),
            Status::Gone(why) => (wire::MediaStatus::Gone, said(&why)),
            Status::Failed(why) => (wire::MediaStatus::Failed, said(&why)),
        })
    }

    /// One line onto a proof tap (`DUCKTAPE_AV_FAKE_TAP`), nothing without.
    fn tap(tap: &mut Option<std::fs::File>, line: std::fmt::Arguments) {
        if let Some(tap) = tap {
            use std::io::Write as _;
            let _ = tap.write_fmt(line).and_then(|()| tap.write_all(b"\n"));
        }
    }

    /// The camera's pictures encoded at the ladder's rung and published,
    /// while subscribers listen; a cut steps the ladder down.
    fn publish_camera(
        feed: Arc<CameraFeed>,
        mut publication: HostPublication,
        standing: Arc<Mutex<Standing>>,
        stop: Arc<AtomicBool>,
    ) {
        let started = Instant::now();
        let mut ladder = Ladder::new(started);
        let mut encoder = match encoder_at(ladder.rung()) {
            Ok(encoder) => Some(encoder),
            Err(why) => {
                // the codec's own reason: turned off in Settings, not
                // fetched, no encoder on this device
                let mut word = lock(&standing);
                word.unsupported = true;
                word.detail = format!("no picture sent: {why}");
                None
            }
        };
        feed.tapped.store(encoder.is_some(), Ordering::Relaxed);
        let mut sent = ducktape_av::fake::tap("sent.log");
        let mut chunker = Chunker::default();
        let mut subscribers = 0u32;
        let mut up = false;
        let mut want_key = false;
        let mut next_frame = started;
        while !stop.load(Ordering::Relaxed) {
            while let Some(heard) = publication.heard() {
                let mut word = lock(&standing);
                match &heard {
                    TrackHeard::Up { subscribers: n } => {
                        if !up {
                            // a fresh session or grant: start with a keyframe
                            want_key = true;
                            chunker.cut();
                        }
                        if *n > subscribers
                            && chunker.age(Instant::now()).is_none_or(|age| age > FRESH)
                        {
                            want_key = true;
                        }
                        subscribers = *n;
                        up = true;
                    }
                    TrackHeard::Waiting | TrackHeard::Ended { .. } | TrackHeard::None => {
                        up = false;
                    }
                }
                word.track = heard;
            }
            let now = Instant::now();
            if let Some(rung) = ladder.tick(now)
                && let Some(encoder) = &mut encoder
            {
                apply(encoder, rung);
                lock(&standing).detail = less(&ladder);
            }
            if next_frame > now {
                std::thread::sleep((next_frame - now).min(Duration::from_millis(20)));
                continue;
            }
            next_frame = now + Duration::from_millis(1000 / u64::from(ladder.rung().fps));
            let picture = feed.tap.lock().expect("feed tap").take();
            let (Some(encoder), Some(mut picture)) = (&mut encoder, picture) else {
                continue;
            };
            if !up || subscribers == 0 {
                continue;
            }
            if want_key || chunker.age(now).is_some_and(|age| age > GOP) {
                encoder.keyframe();
                want_key = false;
            }
            if feed.mirror.load(Ordering::Relaxed) {
                picture.flip_horizontal();
            }
            let packet = match encoder.encode(&picture, now - started) {
                Ok(Some(packet)) => packet,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(target: "ducktape::call", %error, "a picture did not encode");
                    continue;
                }
            };
            let frame = Frame {
                ts_ms: (now - started).as_millis() as u32,
                keyframe: packet.keyframe,
                data: packet.data,
            };
            let objects = chunker.push(&frame, now);
            tap(
                &mut sent,
                format_args!(
                    "frame {} key {} bytes {} objects {} rung {}x{}",
                    frame.ts_ms,
                    frame.keyframe,
                    frame.data.len(),
                    objects.len(),
                    ladder.rung().width,
                    ladder.rung().height
                ),
            );
            for object in objects {
                match publication.push(object) {
                    Pushed::Queued | Pushed::Dropped => {}
                    Pushed::Cut(_) => {
                        tap(&mut sent, format_args!("cut"));
                        chunker.cut();
                        if let Some(rung) = ladder.cut(now) {
                            apply(encoder, rung);
                        }
                        want_key = true;
                        lock(&standing).detail = less(&ladder);
                        break;
                    }
                    Pushed::Closed => break,
                }
            }
        }
        feed.tapped.store(false, Ordering::Relaxed);
    }

    fn encoder_at(rung: Rung) -> Result<VideoEncoder, String> {
        VideoEncoder::new(rung.width, rung.height, rung.bitrate, rung.fps)
    }

    /// The encoder at `rung`; its next packet is a keyframe.
    fn apply(encoder: &mut VideoEncoder, rung: Rung) {
        let _ = encoder.set_size(rung.width, rung.height, rung.fps);
        let _ = encoder.set_bitrate(rung.bitrate);
    }

    fn less(ladder: &Ladder) -> String {
        match ladder.lowered() {
            true => LESS_PICTURE.into(),
            false => String::new(),
        }
    }

    /// The voice I/O's frames encoded and published while the piece is
    /// not muted; its status and level into the meter's feed.
    fn publish_microphone(
        io: Arc<voice::Io>,
        feed: Arc<MicFeed>,
        mut publication: HostPublication,
        standing: Arc<Mutex<Standing>>,
        stop: Arc<AtomicBool>,
        muted: Arc<AtomicBool>,
    ) {
        let mut encoder = match AudioEncoder::new(VOICE_BITRATE) {
            Ok(encoder) => Some(encoder),
            Err(_) => {
                lock(&standing).unsupported = true;
                None
            }
        };
        let mut numbering = AudioNumbering::default();
        let mut capture: Option<voice::Capture> = None;
        let mut up = false;
        while !stop.load(Ordering::Relaxed) {
            while let Some(heard) = publication.heard() {
                up = matches!(heard, TrackHeard::Up { .. });
                lock(&standing).track = heard;
            }
            let (status, name) = io.status();
            *feed.status.lock().expect("feed status") = (status, name);
            match (muted.load(Ordering::Relaxed), capture.is_some()) {
                (false, false) => capture = Some(io.capture()),
                (true, true) => {
                    capture = None;
                    feed.level.store(0f32.to_bits(), Ordering::Relaxed);
                }
                _ => {}
            }
            let Some(held) = &capture else {
                std::thread::sleep(MIC_WAIT);
                continue;
            };
            let Some(frame) = held.recv_timeout(MIC_WAIT) else {
                continue;
            };
            feed.level.store(io.level().to_bits(), Ordering::Relaxed);
            let (Some(encoder), true) = (&mut encoder, up) else {
                continue;
            };
            let body = match encoder.encode(&frame) {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!(target: "ducktape::call", %error, "a frame did not encode");
                    continue;
                }
            };
            let (group, object) = numbering.next();
            if publication.push(Object {
                group,
                object,
                body,
                end: false,
            }) == Pushed::Closed
            {
                break;
            }
        }
    }

    /// The track's objects reassembled and decoded into the remote
    /// camera's feed, until the subscription ends.
    fn play_video(
        subscription: &mut crate::runtime::HostSubscription,
        feed: Arc<CameraFeed>,
        standing: Arc<Mutex<Standing>>,
    ) {
        let mut decoder = VideoDecoder::new().ok();
        if decoder.is_none() {
            lock(&standing).unsupported = true;
        }
        let mut shown = ducktape_av::fake::tap("video.log");
        let mut got = ducktape_av::fake::tap("recv.log");
        let mut frames = Reassembler::default();
        while let Some(item) = subscription.recv() {
            match &item {
                Subscribed::Heard(heard) => tap(&mut got, format_args!("heard {heard:?}")),
                Subscribed::Data(Data::Object { group, body, .. }) => {
                    tap(&mut got, format_args!("object {group} {}", body.len()));
                }
                Subscribed::Data(Data::GroupEnd { group, complete }) => {
                    tap(&mut got, format_args!("end {group} {complete}"));
                }
            }
            match item {
                Subscribed::Heard(heard) => {
                    let mut word = lock(&standing);
                    if let TrackHeard::Ended { code } = &heard {
                        word.status = Status::Gone(code.clone());
                    }
                    if matches!(heard, TrackHeard::Up { .. }) {
                        // a fresh subscription replays from a keyframe
                        frames.reset();
                        decoder = decoder.and_then(|_| VideoDecoder::new().ok());
                    }
                    word.track = heard;
                }
                Subscribed::Data(data) => {
                    let Some(frame) = frames.push(data) else {
                        continue;
                    };
                    let Some(decoder) = &mut decoder else {
                        continue;
                    };
                    match decoder.decode(&frame.data) {
                        Ok(Some(picture)) => {
                            if shown.is_some()
                                && let Some(n) = ducktape_av::fake::read_stamp(&picture)
                            {
                                tap(&mut shown, format_args!("{n}"));
                            }
                            *feed.picture.lock().expect("feed picture") = Some(picture);
                            feed.status.lock().expect("feed status").0 = Status::Live;
                            lock(&standing).status = Status::Live;
                        }
                        Ok(None) => {}
                        Err(_) => frames.reset(),
                    }
                }
            }
        }
    }

    /// The track's packets onto the voice, until the subscription ends.
    fn play_audio(
        subscription: &mut crate::runtime::HostSubscription,
        voice: Option<Arc<voice::Voice>>,
        standing: Arc<Mutex<Standing>>,
    ) {
        while let Some(item) = subscription.recv() {
            match item {
                Subscribed::Heard(heard) => {
                    let mut word = lock(&standing);
                    if let TrackHeard::Ended { code } = &heard {
                        word.status = Status::Gone(code.clone());
                    }
                    word.track = heard;
                }
                Subscribed::Data(Data::Object {
                    group,
                    object,
                    body,
                }) => {
                    if let Some(voice) = &voice {
                        voice.push(
                            group * u64::from(crate::backend::live::media::AUDIO_GROUP)
                                + u64::from(object),
                            body,
                        );
                    }
                    let mut word = lock(&standing);
                    if word.status != Status::Live {
                        word.status = Status::Live;
                    }
                }
                Subscribed::Data(Data::GroupEnd { .. }) => {}
            }
        }
    }
}
