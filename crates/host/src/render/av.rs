//! The camera and microphone pieces (`wire::Node::Camera`,
//! `wire::Node::Microphone`) on this host, and every call the rest of the
//! app makes into the av feature. With feature `av` a camera piece is a
//! `ducktape_av::Camera` and a microphone piece a `ducktape_av::Meter`
//! (crates/av, where every device call and every OS's words live), one
//! per node, kept by the node's authored path from the frame that first
//! has it to the frame that no longer does (its device released then, at
//! once), and made again when its kind or device changes; a view opens a
//! device once, and at most `MAX_PIECES` of a kind. Without it each
//! piece says the build has none, and the methods are unknown requests
//! (the kernel has no arm for them).
//! - the tree: [`ViewTree::camera`] and [`ViewTree::microphone`] draw a
//!   piece, [`Media::retain`] keeps a frame's pieces, and
//!   [`ViewTree::release_media`] closes them all when the view stops;
//! - the methods: [`cameras`] and [`microphones`] answer `camera.devices`
//!   and `microphone.devices`, [`chime`] `microphone.chime`
//!   (`runtime/kernel.rs`); [`settings_page`] is the system's page
//!   `host.show` brings up for a refused device, which the shell opens.

use super::*;
#[cfg(feature = "av")]
use view_wire::methods::{self, refusal};

/// The system's page `host.show` brings up for a refused camera's or
/// microphone's settings place, where this host has one (macOS's privacy
/// panes, iOS's page of the app: `ducktape_av::settings_page`), as a
/// refused device's state says
/// (`MediaState::opens_settings`). The shell hands it to the system. The
/// reader's account is the shell's own place, no page of the system's.
pub fn settings_page(place: view_wire::methods::Place) -> Option<&'static str> {
    #[cfg(feature = "av")]
    return match place {
        methods::Place::Account => None,
        methods::Place::CameraSettings => ducktape_av::settings_page(ducktape_av::Kind::Camera),
        methods::Place::MicrophoneSettings => {
            ducktape_av::settings_page(ducktape_av::Kind::Microphone)
        }
    };
    #[cfg(not(feature = "av"))]
    {
        let _ = place;
        None
    }
}

/// `camera.devices`: the cameras listed now, each with which way it
/// looks; listed on a blocking thread (each camera's node is opened to
/// ask what it is), never the window's.
#[cfg(feature = "av")]
pub async fn cameras() -> Result<Vec<u8>, wire::Error> {
    blocking(listed_cameras).await
}

#[cfg(feature = "av")]
fn listed_cameras() -> Result<Vec<u8>, wire::Error> {
    let found =
        ducktape_av::cameras().map_err(|error| wire::Error::new(refusal::HOST_FAULT, error))?;
    let listed: Vec<methods::CameraDevice> = found
        .into_iter()
        .map(|camera| {
            let (id, name, default) = device(camera.device);
            let facing = match camera.facing {
                ducktape_av::Facing::Front => methods::Facing::Front,
                ducktape_av::Facing::Back => methods::Facing::Back,
                ducktape_av::Facing::External => methods::Facing::External,
            };
            methods::CameraDevice {
                id,
                name,
                default,
                facing,
            }
        })
        .collect();
    Ok(methods::encode(&listed))
}

/// `microphone.devices`: the microphones and speakers listed now, each
/// with what it is; listed on a blocking thread, never the window's.
#[cfg(feature = "av")]
pub async fn microphones() -> Result<Vec<u8>, wire::Error> {
    blocking(listed_microphones).await
}

#[cfg(feature = "av")]
fn listed_microphones() -> Result<Vec<u8>, wire::Error> {
    let (inputs, outputs) = ducktape_av::audio_devices()
        .map_err(|error| wire::Error::new(refusal::HOST_FAULT, error))?;
    let listed = |found: Vec<ducktape_av::AudioDevice>| {
        found
            .into_iter()
            .map(|audio| {
                let (id, name, default) = device(audio.device);
                let kind = match audio.kind {
                    ducktape_av::AudioKind::Speaker => methods::AudioKind::Speaker,
                    ducktape_av::AudioKind::Earpiece => methods::AudioKind::Earpiece,
                    ducktape_av::AudioKind::Headphones => methods::AudioKind::Headphones,
                    ducktape_av::AudioKind::Other => methods::AudioKind::Other,
                };
                methods::AudioDevice {
                    id,
                    name,
                    default,
                    kind,
                }
            })
            .collect()
    };
    Ok(methods::encode(&methods::AudioDevices {
        inputs: listed(inputs),
        outputs: listed(outputs),
    }))
}

/// `microphone.chime`: the test chime on `output` (the system's speaker
/// when `None`), answered once it has sounded; played on a blocking
/// thread, apart from any microphone piece.
#[cfg(feature = "av")]
pub async fn chime(output: Option<String>) -> Result<Vec<u8>, wire::Error> {
    blocking(move || {
        ducktape_av::play_chime(output.as_deref())
            .map(|()| Vec::new())
            .map_err(|error| wire::Error::new(refusal::HOST_FAULT, error))
    })
    .await
}

/// `answer` run on a blocking thread, its answer awaited.
#[cfg(feature = "av")]
async fn blocking(
    answer: impl FnOnce() -> Result<Vec<u8>, wire::Error> + Send + 'static,
) -> Result<Vec<u8>, wire::Error> {
    tokio::task::spawn_blocking(answer)
        .await
        .unwrap_or_else(|error| Err(wire::Error::new(refusal::HOST_FAULT, error.to_string())))
}

/// A listed device's id, name (cleaned as a window title) and whether the
/// system picks it.
#[cfg(feature = "av")]
fn device(found: ducktape_av::Device) -> (String, String, bool) {
    (
        found.id,
        crate::runtime::shown_title(&found.name),
        found.default,
    )
}

/// The pieces of a tree, by authored path.
#[derive(Default)]
pub(super) struct Media {
    #[cfg(feature = "av")]
    pieces: HashMap<AuthoredPath, Piece>,
    /// The frame drawn came with the person's press or key: only then
    /// does a microphone's monitor come on or a recording start, each a
    /// sound (as `microphone.chime` needs a press).
    #[cfg(feature = "av")]
    pressed: bool,
}

/// The most pieces of one kind a view has open at once: its own tile, a
/// device check's preview, a picker's few. Past it a piece opens nothing
/// and says so, as does a second piece on a device the view has open:
/// each open is a thread and a stream, which no frame multiplies.
#[cfg(feature = "av")]
const MAX_PIECES: usize = 4;

/// One piece: what it opened, the device it names, and the route its
/// state goes to.
#[cfg(feature = "av")]
struct Piece {
    opened: Opened,
    /// The camera or microphone it names (`None`: the system's default);
    /// another makes the piece again. A microphone's speaker is not one:
    /// another moves its sound, the microphone staying open.
    device: Option<String>,
    revision: u64,
    /// A microphone's monitor as the host let it on (at a press).
    monitor: bool,
    on_state: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    _state: Option<Subscription>,
    /// Lets the device go the moment the piece drops, before gpui
    /// releases its entity: the piece made in its place then waits for
    /// the device to close (ducktape-av's one holder at a time), never
    /// finding it busy.
    _release: Option<ducktape_av::Release>,
}

#[cfg(feature = "av")]
enum Opened {
    Camera(Entity<ducktape_av::Camera>),
    Meter(Entity<ducktape_av::Meter>),
    /// Nothing opened: the view has this device open in another piece, or
    /// [`MAX_PIECES`] of its kind.
    Refused(ducktape_av::Kind),
}

#[cfg(feature = "av")]
impl Opened {
    fn kind(&self) -> ducktape_av::Kind {
        match self {
            Opened::Camera(_) => ducktape_av::Kind::Camera,
            Opened::Meter(_) => ducktape_av::Kind::Microphone,
            Opened::Refused(kind) => *kind,
        }
    }
}

impl Media {
    /// Keeps the pieces `root` still has; the others' devices close.
    pub(super) fn retain(&mut self, root: &wire::Node) {
        #[cfg(feature = "av")]
        if !self.pieces.is_empty() {
            let mut kept = std::collections::HashSet::new();
            commands::walk_authored_paths(root, None, &mut Vec::new(), &mut |node, path| {
                if matches!(
                    node,
                    wire::Node::Camera { .. } | wire::Node::Microphone { .. }
                ) {
                    kept.insert(path.clone());
                }
            });
            self.pieces.retain(|path, _| kept.contains(path));
        }
        let _ = root;
    }

    /// Every piece's device closes: the view stopped, or its tree goes.
    pub(super) fn release(&mut self) {
        #[cfg(feature = "av")]
        self.pieces.clear();
    }

    /// Why a view may not open another `kind` piece on `device` at `path`,
    /// past the ones it has open elsewhere; `None` when it may.
    #[cfg(feature = "av")]
    fn refusal(
        &self,
        path: &AuthoredPath,
        kind: ducktape_av::Kind,
        device: &Option<String>,
    ) -> Option<&'static str> {
        let open: Vec<&Piece> = self
            .pieces
            .iter()
            .filter(|(at, piece)| {
                *at != path
                    && piece.opened.kind() == kind
                    && !matches!(piece.opened, Opened::Refused(_))
            })
            .map(|(_, piece)| piece)
            .collect();
        if open.iter().any(|piece| piece.device == *device) {
            Some("This view has this device open in another piece.")
        } else if open.len() >= MAX_PIECES {
            Some("This view has as many of these open as it may.")
        } else {
            None
        }
    }
}

impl ViewTree {
    /// The seat says whether the frame it hands over came with the
    /// person's press or key ([`Media`]'s `pressed`).
    pub fn set_pressed(&mut self, pressed: bool) {
        #[cfg(feature = "av")]
        {
            self.media.pressed = pressed;
        }
        let _ = pressed;
    }

    /// The view stopped while its tree stays (a stand-in over it): every
    /// camera and microphone it opened closes.
    pub fn release_media(&mut self) {
        self.media.release();
    }

    /// A camera piece: its pictures in the node's box, or the line that
    /// says this build has no camera.
    pub(super) fn camera(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Camera { id, style, .. } = node else {
            unreachable!()
        };
        let element = div()
            .refine_style(&self.style(*style))
            .id(native_id(id))
            .overflow_hidden();
        #[cfg(feature = "av")]
        let element = element.children(self.camera_piece(node, cx));
        #[cfg(not(feature = "av"))]
        let element = {
            let _ = cx;
            placeholder(element, "This build has no camera")
        };
        element.into_any_element()
    }

    /// A microphone piece: its level in the node's box, or the line that
    /// says this build has no microphone.
    pub(super) fn microphone(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Microphone { id, style, .. } = node else {
            unreachable!()
        };
        let style = self.style(*style);
        let element = div()
            .refine_style(&style)
            .id(native_id(id))
            .overflow_hidden();
        #[cfg(feature = "av")]
        let element = {
            let color = style.text.color.unwrap_or(window.text_style().color);
            element.children(self.meter(node, color, cx))
        };
        #[cfg(not(feature = "av"))]
        let element = {
            let _ = (window, cx);
            placeholder(element, "This build has no microphone")
        };
        element.into_any_element()
    }

    /// The piece under this path, made when it is new, names another kind
    /// or device, or was refused and may open now; its state routed to
    /// `on_state`.
    #[cfg(feature = "av")]
    fn piece(
        &mut self,
        kind: ducktape_av::Kind,
        device: Option<String>,
        on_state: Option<u32>,
        make: impl FnOnce(&mut Context<Self>) -> Opened,
        cx: &mut Context<Self>,
    ) -> &mut Piece {
        let path = self.authored_path.clone();
        let refusal = self.media.refusal(&path, kind, &device);
        let again = self.media.pieces.get(&path).is_none_or(|piece| {
            piece.opened.kind() != kind
                || piece.device != device
                || matches!(piece.opened, Opened::Refused(_)) && refusal.is_none()
        });
        if again {
            // the old device let go (its `Release`) before the new one
            // opens, which waits for it to have closed
            self.media.pieces.remove(&path);
            let route = std::rc::Rc::new(std::cell::Cell::new(on_state));
            let heard = route.clone();
            let tell = move |_: &mut Self, state: &ducktape_av::State, cx: &mut Context<Self>| {
                if let Some(handler) = heard.get() {
                    let state = told(state, kind);
                    cx.emit(wire::Event::Media { handler, state });
                }
            };
            let opened = match refusal {
                Some(why) => {
                    if let Some(handler) = on_state {
                        let state = wire::MediaState {
                            status: wire::MediaStatus::Failed,
                            detail: why.into(),
                            ..Default::default()
                        };
                        cx.emit(wire::Event::Media { handler, state });
                    }
                    Opened::Refused(kind)
                }
                None => make(cx),
            };
            let (_state, _release) = match &opened {
                Opened::Camera(camera) => (
                    Some(cx.subscribe(camera, move |this, _, state, cx| tell(this, state, cx))),
                    Some(camera.read(cx).release()),
                ),
                Opened::Meter(meter) => (
                    Some(cx.subscribe(meter, move |this, _, state, cx| tell(this, state, cx))),
                    Some(meter.read(cx).release()),
                ),
                Opened::Refused(_) => (None, None),
            };
            let piece = Piece {
                opened,
                device,
                revision: 0,
                monitor: false,
                on_state: route,
                _state,
                _release,
            };
            self.media.pieces.insert(path.clone(), piece);
        }
        let piece = self.media.pieces.get_mut(&path).expect("made above");
        piece.on_state.set(on_state);
        piece
    }

    #[cfg(feature = "av")]
    fn camera_piece(
        &mut self,
        node: &wire::Node,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ducktape_av::Camera>> {
        let wire::Node::Camera {
            device,
            mirror,
            object_fit,
            on_state,
            ..
        } = node
        else {
            unreachable!()
        };
        let mirror = *mirror;
        let fit = || super::pictures::primitive_object_fit(*object_fit);
        let opens = device.clone();
        let piece = self.piece(
            ducktape_av::Kind::Camera,
            device.clone(),
            *on_state,
            |cx| Opened::Camera(cx.new(|cx| ducktape_av::Camera::new(opens, mirror, fit(), cx))),
            cx,
        );
        let Opened::Camera(camera) = &piece.opened else {
            return None;
        };
        let camera = camera.clone();
        camera.update(cx, |camera, cx| {
            camera.set_mirror(mirror);
            camera.set_fit(fit(), cx);
        });
        Some(camera)
    }

    #[cfg(feature = "av")]
    fn meter(
        &mut self,
        node: &wire::Node,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ducktape_av::Meter>> {
        let wire::Node::Microphone {
            device,
            output,
            monitor,
            revision,
            commands,
            on_state,
            ..
        } = node
        else {
            unreachable!()
        };
        let pressed = self.media.pressed;
        let (opens, plays) = (device.clone(), output.clone());
        let piece = self.piece(
            ducktape_av::Kind::Microphone,
            device.clone(),
            *on_state,
            |cx| Opened::Meter(cx.new(|cx| ducktape_av::Meter::new(opens, plays, color, cx))),
            cx,
        );
        let Opened::Meter(meter) = &piece.opened else {
            return None;
        };
        let meter = meter.clone();
        let apply = piece.revision != *revision;
        piece.revision = *revision;
        // a sound comes on only at a press: the monitor turning on, a
        // recording starting (it plays back). Off and Stop need none
        piece.monitor = *monitor && (piece.monitor || pressed);
        let monitor = piece.monitor;
        meter.update(cx, |meter, cx| {
            meter.set_color(color, cx);
            meter.set_output(output.clone());
            meter.set_monitor(monitor);
            if apply {
                for command in commands {
                    match command {
                        wire::MicCommand::Record { seconds } if pressed => meter.record(*seconds),
                        wire::MicCommand::Record { .. } => {}
                        wire::MicCommand::Stop => meter.stop(),
                    }
                }
            }
        });
        Some(meter)
    }

    /// The cameras and microphones this tree has open, in no order.
    #[cfg(all(feature = "av", any(test, feature = "test-support")))]
    pub fn media_for_test(
        &self,
    ) -> (
        Vec<Entity<ducktape_av::Camera>>,
        Vec<Entity<ducktape_av::Meter>>,
    ) {
        let mut cameras = Vec::new();
        let mut meters = Vec::new();
        for piece in self.media.pieces.values() {
            match &piece.opened {
                Opened::Camera(camera) => cameras.push(camera.clone()),
                Opened::Meter(meter) => meters.push(meter.clone()),
                Opened::Refused(_) => {}
            }
        }
        (cameras, meters)
    }
}

/// What a view hears of its piece's device: a refusal says how to allow
/// the device here, and whether its settings page opens (`link.open` of
/// the wire's settings links).
#[cfg(feature = "av")]
fn told(state: &ducktape_av::State, kind: ducktape_av::Kind) -> wire::MediaState {
    use ducktape_av::{Playback, Status};
    let mut opens_settings = false;
    let (status, detail) = match &state.status {
        Status::Starting => (wire::MediaStatus::Starting, String::new()),
        Status::Live => (wire::MediaStatus::Live, String::new()),
        Status::Denied => {
            opens_settings = ducktape_av::settings_page(kind).is_some();
            (
                wire::MediaStatus::Denied,
                ducktape_av::allow(kind).to_owned(),
            )
        }
        Status::Busy => (wire::MediaStatus::Busy, String::new()),
        Status::Gone(why) => (wire::MediaStatus::Gone, why.clone()),
        Status::NoDevice => (wire::MediaStatus::NoDevice, String::new()),
        Status::Failed(why) => (wire::MediaStatus::Failed, why.clone()),
    };
    // one line, at most 200 bytes, as the wire says (a window title's
    // 80 chars would cut a fix line short)
    let mut detail: String = detail.chars().filter(crate::runtime::plain).collect();
    while detail.len() > 200 {
        detail.pop();
    }
    let detail = detail.trim().to_owned();
    wire::MediaState {
        status,
        device: crate::runtime::shown_title(&state.device),
        playback: match state.playback {
            Playback::Idle => wire::Playback::Idle,
            Playback::Recording => wire::Playback::Recording,
            Playback::Playing => wire::Playback::Playing,
        },
        detail,
        speaking: state.speaking,
        opens_settings,
    }
}

/// A piece's box saying this build has no such device.
#[cfg(not(feature = "av"))]
fn placeholder(element: Stateful<Div>, line: &'static str) -> Stateful<Div> {
    element.flex().items_center().justify_center().child(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera_on(id: &str, device: Option<&str>, on_state: Option<u32>) -> wire::Node {
        wire::Node::Camera {
            id: wire::ElementIdWire::Name(id.into()),
            style: crate::render::plain_style(),
            device: device.map(Into::into),
            mirror: true,
            object_fit: wire::ImageObjectFit::Cover,
            on_state,
        }
    }

    /// A microphone piece `id` on `device`, playing on `output`, its
    /// monitor and commands as given at `revision`.
    fn microphone_on(
        id: &str,
        device: Option<&str>,
        output: Option<&str>,
        monitor: bool,
        revision: u64,
        commands: Vec<wire::MicCommand>,
    ) -> wire::Node {
        wire::Node::Microphone {
            id: wire::ElementIdWire::Name(id.into()),
            style: crate::render::plain_style(),
            device: device.map(Into::into),
            output: output.map(Into::into),
            monitor,
            revision,
            commands,
            on_state: None,
        }
    }

    #[cfg(not(feature = "av"))]
    fn camera() -> wire::Node {
        camera_on("camera", None, None)
    }

    #[cfg(not(feature = "av"))]
    fn microphone() -> wire::Node {
        microphone_on("mic", None, None, false, 0, Vec::new())
    }

    fn holder(children: Vec<wire::Node>) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("root".into())),
            style: crate::render::plain_style(),
            interactivity: Default::default(),
            children,
        })
    }

    /// A build without av draws both pieces as their lines.
    #[cfg(not(feature = "av"))]
    #[gpui_kit::test]
    fn a_build_without_av_draws_the_pieces_as_lines(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::px;
        cx.update(gpui_kit::init);
        let window = cx.open_window(gpui_kit::size(px(200.), px(100.)), |_, _| {
            ViewTree::new(holder(vec![camera(), microphone()]))
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    /// A tree of `children` in a window of its own, drawn once; the
    /// events it emits from then on collected.
    #[cfg(feature = "av")]
    fn drawn(
        children: Vec<wire::Node>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<ViewTree>,
        gpui_kit::VisualTestContext,
        std::rc::Rc<std::cell::RefCell<Vec<wire::Event>>>,
    ) {
        use gpui_kit::px;
        cx.update(gpui_kit::init);
        let window = cx.open_window(gpui_kit::size(px(400.), px(300.)), |_, _| {
            ViewTree::new(holder(children))
        });
        let tree = window.root(cx).unwrap();
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let into = heard.clone();
        cx.update(|cx| {
            cx.subscribe(&tree, move |_, event: &wire::Event, _| {
                into.borrow_mut().push(event.clone())
            })
            .detach()
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.draw(cx).clear(cx));
        native.run_until_parked();
        (tree, native, heard)
    }

    /// The tree's next frame, `children`, drawn; `pressed` as the seat
    /// says it.
    #[cfg(feature = "av")]
    fn next(
        tree: &Entity<ViewTree>,
        children: Vec<wire::Node>,
        pressed: bool,
        native: &mut gpui_kit::VisualTestContext,
    ) {
        native.cx.update(|cx| {
            tree.update(cx, |tree, cx| {
                tree.set_pressed(pressed);
                tree.replace(holder(children), &[], cx)
            })
        });
        native.update(|window, cx| window.draw(cx).clear(cx));
        native.run_until_parked();
    }

    #[cfg(feature = "av")]
    fn media(
        tree: &Entity<ViewTree>,
        native: &gpui_kit::VisualTestContext,
    ) -> (
        Vec<Entity<ducktape_av::Camera>>,
        Vec<Entity<ducktape_av::Meter>>,
    ) {
        tree.read_with(native, |tree, _| tree.media_for_test())
    }

    // Every piece below names a device no system lists: each opens
    // nothing (no camera light, no permission prompt on a Mac) and says
    // `NoDevice`.

    /// A view that puts a microphone where its camera was (one id, one
    /// place) gets a microphone: the piece is made again for its kind,
    /// not taken for the camera (which ended the app).
    #[cfg(feature = "av")]
    #[gpui_kit::test]
    fn a_place_that_changes_kind_holds_the_new_kind(cx: &mut gpui_kit::TestAppContext) {
        let (tree, mut native, _) = drawn(vec![camera_on("x", Some("no-such"), None)], cx);
        assert_eq!(media(&tree, &native).0.len(), 1);
        let microphone = microphone_on("x", Some("no-such"), None, false, 0, Vec::new());
        next(&tree, vec![microphone], false, &mut native);
        let (cameras, meters) = media(&tree, &native);
        assert!(cameras.is_empty());
        assert_eq!(meters.len(), 1);
    }

    /// Another device picked lets the old one go at once, while gpui
    /// still holds its piece: the new one, which waits for every device
    /// let go to close, never finds the old still open and busy.
    #[cfg(feature = "av")]
    #[gpui_kit::test]
    fn another_device_lets_the_old_go_at_once(cx: &mut gpui_kit::TestAppContext) {
        let (tree, mut native, _) = drawn(vec![camera_on("x", Some("no-such-a"), None)], cx);
        let old = media(&tree, &native).0.pop().expect("a camera");
        assert!(!old.read_with(&native, |camera, _| camera.let_go()));
        next(
            &tree,
            vec![camera_on("x", Some("no-such-b"), None)],
            false,
            &mut native,
        );
        assert!(old.read_with(&native, |camera, _| camera.let_go()));
        let new = media(&tree, &native).0.pop().expect("a camera");
        assert_ne!(new.entity_id(), old.entity_id());
        assert!(!new.read_with(&native, |camera, _| camera.let_go()));
    }

    /// Another speaker moves a microphone's sound: the microphone stays
    /// open (the same piece), told the new output.
    #[cfg(feature = "av")]
    #[gpui_kit::test]
    fn another_speaker_keeps_the_microphone_open(cx: &mut gpui_kit::TestAppContext) {
        let on = |output| microphone_on("mic", Some("no-such"), Some(output), false, 0, Vec::new());
        let (tree, mut native, _) = drawn(vec![on("speaker-a")], cx);
        let before = media(&tree, &native).1.pop().expect("a microphone");
        next(&tree, vec![on("speaker-b")], false, &mut native);
        let after = media(&tree, &native).1.pop().expect("a microphone");
        assert_eq!(after.entity_id(), before.entity_id());
        assert!(!after.read_with(&native, |meter, _| meter.let_go()));
    }

    /// A view opens a device once and at most four of a kind: a second
    /// piece on one device, or a fifth camera, opens nothing and is told
    /// why; it opens once the one in its way goes.
    #[cfg(feature = "av")]
    #[gpui_kit::test]
    fn a_view_opens_a_device_once_and_a_few_of_a_kind(cx: &mut gpui_kit::TestAppContext) {
        let twice = vec![
            camera_on("one", Some("no-such"), Some(1)),
            camera_on("two", Some("no-such"), Some(2)),
        ];
        // its window draws it as it opens: the pieces come in the next frame,
        // with the events heard
        let (tree, mut native, heard) = drawn(Vec::new(), cx);
        next(&tree, twice, false, &mut native);
        assert_eq!(media(&tree, &native).0.len(), 1);
        let refused = heard.borrow().iter().any(|event| {
            matches!(event, wire::Event::Media { handler: 2, state }
                if state.status == wire::MediaStatus::Failed && !state.detail.is_empty())
        });
        assert!(
            refused,
            "the second piece was not told: {:?}",
            heard.borrow()
        );
        next(
            &tree,
            vec![camera_on("two", Some("no-such"), Some(2))],
            false,
            &mut native,
        );
        assert_eq!(media(&tree, &native).0.len(), 1, "the one left opens");
        let five = (0..5)
            .map(|nth| camera_on(&format!("c{nth}"), Some(&format!("no-such-{nth}")), None))
            .collect();
        next(&tree, five, false, &mut native);
        assert_eq!(media(&tree, &native).0.len(), MAX_PIECES);
    }

    /// A microphone's monitor comes on, and a recording starts, only with
    /// a frame that came with the person's press or key (each a sound);
    /// a monitor let on stays on, and turning it off needs nothing.
    #[cfg(feature = "av")]
    #[gpui_kit::test]
    fn a_sound_starts_only_at_a_press(cx: &mut gpui_kit::TestAppContext) {
        use ducktape_av::Ask;
        let record = vec![wire::MicCommand::Record { seconds: 5 }];
        let mic = |monitor, revision, commands| {
            microphone_on("mic", Some("no-such"), None, monitor, revision, commands)
        };
        let (tree, mut native, _) = drawn(vec![mic(true, 1, record.clone())], cx);
        let meter = media(&tree, &native).1.pop().expect("a microphone");
        let told = |native: &gpui_kit::VisualTestContext| {
            meter.read_with(native, |meter, _| (meter.monitor(), meter.asked()))
        };
        assert_eq!(
            told(&native),
            (false, None),
            "a clock's frame started a sound"
        );
        next(&tree, vec![mic(true, 2, record.clone())], true, &mut native);
        assert_eq!(told(&native), (true, Some(Ask::Record(5))));
        next(&tree, vec![mic(true, 2, record)], false, &mut native);
        assert!(told(&native).0, "a monitor let on stays on");
        next(
            &tree,
            vec![mic(false, 3, vec![wire::MicCommand::Stop])],
            false,
            &mut native,
        );
        assert_eq!(told(&native), (false, Some(Ask::Stop)));
    }

    /// Each settings place brings up its own device's page, where the
    /// system has one (iOS has one page of the app for both), and the
    /// reader's account none.
    #[cfg(feature = "av")]
    #[test]
    fn a_settings_place_is_its_own_devices_page() {
        use ducktape_av::Kind;
        use view_wire::methods::Place;
        let camera = settings_page(Place::CameraSettings);
        let microphone = settings_page(Place::MicrophoneSettings);
        assert_eq!(camera, ducktape_av::settings_page(Kind::Camera));
        assert_eq!(microphone, ducktape_av::settings_page(Kind::Microphone));
        assert_eq!(settings_page(Place::Account), None);
    }

    /// A device's state reaches its view in the wire's words, bounded;
    /// a refusal says how to allow the device here, and its settings page
    /// opens where the system has one; a device gone says why, where the
    /// system said.
    #[cfg(feature = "av")]
    #[test]
    fn a_device_state_is_told_in_the_wires_words() {
        let state = told(
            &ducktape_av::State {
                status: ducktape_av::Status::Failed(format!("bad\n{}", "x".repeat(300))),
                device: "Cam\u{202e}era".into(),
                playback: ducktape_av::Playback::Playing,
                speaking: true,
            },
            ducktape_av::Kind::Microphone,
        );
        assert_eq!(state.status, wire::MediaStatus::Failed);
        assert!(state.detail.len() <= 200 && !state.detail.contains('\n'));
        assert_eq!(state.device, "Camera");
        assert_eq!(state.playback, wire::Playback::Playing);
        assert!(state.speaking && !state.opens_settings);
        let denied = told(
            &ducktape_av::State {
                status: ducktape_av::Status::Denied,
                ..Default::default()
            },
            ducktape_av::Kind::Camera,
        );
        assert_eq!(denied.status, wire::MediaStatus::Denied);
        assert_eq!(
            denied.opens_settings,
            ducktape_av::settings_page(ducktape_av::Kind::Camera).is_some()
        );
        assert_eq!(denied.detail, ducktape_av::allow(ducktape_av::Kind::Camera));
        assert!(denied.detail.len() <= 200 && denied.detail.ends_with("then try again."));
        let gone = told(
            &ducktape_av::State {
                status: ducktape_av::Status::Gone("A call took the microphone.".into()),
                ..Default::default()
            },
            ducktape_av::Kind::Microphone,
        );
        assert_eq!(gone.status, wire::MediaStatus::Gone);
        assert_eq!(gone.detail, "A call took the microphone.");
    }
}
