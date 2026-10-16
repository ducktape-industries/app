//! The camera and microphone pieces (`wire::Node::Camera`,
//! `wire::Node::Microphone`) on this host, and every call the rest of the
//! app makes into the av feature. With feature `av` a camera piece is a
//! `ducktape_av::Camera` and a microphone piece a `ducktape_av::Meter`
//! (crates/av, where every device call and every OS's words live), one
//! per node, kept by the node's authored path from the frame that first
//! has it to the frame that no longer does (its device released then),
//! and made again when its device changes; without it each piece says the
//! build has none, and the methods are unknown requests (the kernel has
//! no arm for them).
//! - the tree: [`ViewTree::camera`] and [`ViewTree::microphone`] draw a
//!   piece, [`Media::retain`] keeps a frame's pieces;
//! - the methods: [`cameras`] and [`microphones`] answer `camera.devices`
//!   and `microphone.devices`, [`chime`] `microphone.chime`
//!   (`runtime/kernel.rs`); [`settings_page`] is the system's page
//!   `host.show` brings up for a refused device, which the shell opens.

use super::*;
#[cfg(feature = "av")]
use view_wire::methods::{self, refusal};

/// The system's page `host.show` brings up for a refused camera's or
/// microphone's settings place, where this host has one (macOS's privacy
/// panes, `ducktape_av::settings_page`), as a refused device's state says
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
/// looks.
/// ponytail: listed on the window thread; `spawn_blocking` and the
/// kernel's replies are the upgrade if a system lists slowly.
#[cfg(feature = "av")]
pub fn cameras() -> Result<Vec<u8>, wire::Error> {
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
/// with what it is.
#[cfg(feature = "av")]
pub fn microphones() -> Result<Vec<u8>, wire::Error> {
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
    match tokio::task::spawn_blocking(move || ducktape_av::play_chime(output.as_deref())).await {
        Ok(Ok(())) => Ok(Vec::new()),
        Ok(Err(error)) => Err(wire::Error::new(refusal::HOST_FAULT, error)),
        Err(error) => Err(wire::Error::new(refusal::HOST_FAULT, error.to_string())),
    }
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
}

/// One piece's device, what it was opened with, and the route its state
/// goes to.
#[cfg(feature = "av")]
struct Piece {
    device: Device,
    /// The devices it opened, and for a microphone the output: another
    /// makes the piece again.
    opened: (Option<String>, Option<String>),
    revision: u64,
    on_state: std::rc::Rc<std::cell::Cell<Option<u32>>>,
    _state: Subscription,
}

#[cfg(feature = "av")]
enum Device {
    Camera(Entity<ducktape_av::Camera>),
    Meter(Entity<ducktape_av::Meter>),
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
}

impl ViewTree {
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
        let element = element.child(self.camera_piece(node, cx));
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
            element.child(self.meter(node, color, cx))
        };
        #[cfg(not(feature = "av"))]
        let element = {
            let _ = (window, cx);
            placeholder(element, "This build has no microphone")
        };
        element.into_any_element()
    }

    /// The piece under this path, made when it is new or its devices
    /// changed, its state routed to `on_state`.
    #[cfg(feature = "av")]
    fn piece(
        &mut self,
        opened: (Option<String>, Option<String>),
        on_state: Option<u32>,
        kind: ducktape_av::Kind,
        make: impl FnOnce(&mut Context<Self>) -> Device,
        cx: &mut Context<Self>,
    ) -> &mut Piece {
        let path = self.authored_path.clone();
        if self
            .media
            .pieces
            .get(&path)
            .is_none_or(|piece| piece.opened != opened)
        {
            // the old device let go before the new one opens, which waits
            // for it to have closed (ducktape-av's one holder at a time)
            self.media.pieces.remove(&path);
            let route = std::rc::Rc::new(std::cell::Cell::new(on_state));
            let heard = route.clone();
            let tell = move |_: &mut Self, state: &ducktape_av::State, cx: &mut Context<Self>| {
                if let Some(handler) = heard.get() {
                    let state = told(state, kind);
                    cx.emit(wire::Event::Media { handler, state });
                }
            };
            let device = make(cx);
            let _state = match &device {
                Device::Camera(camera) => {
                    cx.subscribe(camera, move |this, _, state, cx| tell(this, state, cx))
                }
                Device::Meter(meter) => {
                    cx.subscribe(meter, move |this, _, state, cx| tell(this, state, cx))
                }
            };
            let piece = Piece {
                device,
                opened,
                revision: 0,
                on_state: route,
                _state,
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
    ) -> Entity<ducktape_av::Camera> {
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
        let opened = (device.clone(), None);
        let device = device.clone();
        let piece = self.piece(
            opened,
            *on_state,
            ducktape_av::Kind::Camera,
            |cx| Device::Camera(cx.new(|cx| ducktape_av::Camera::new(device, mirror, fit(), cx))),
            cx,
        );
        let Device::Camera(camera) = &piece.device else {
            unreachable!("a camera's path holds a camera")
        };
        let camera = camera.clone();
        camera.update(cx, |camera, cx| {
            camera.set_mirror(mirror);
            camera.set_fit(fit(), cx);
        });
        camera
    }

    #[cfg(feature = "av")]
    fn meter(
        &mut self,
        node: &wire::Node,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> Entity<ducktape_av::Meter> {
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
        let opened = (device.clone(), output.clone());
        let (device, output) = opened.clone();
        let piece = self.piece(
            opened,
            *on_state,
            ducktape_av::Kind::Microphone,
            |cx| Device::Meter(cx.new(|cx| ducktape_av::Meter::new(device, output, color, cx))),
            cx,
        );
        let Device::Meter(meter) = &piece.device else {
            unreachable!("a microphone's path holds a meter")
        };
        let meter = meter.clone();
        let apply = piece.revision != *revision;
        piece.revision = *revision;
        meter.update(cx, |meter, cx| {
            meter.set_color(color, cx);
            meter.set_monitor(*monitor);
            if apply {
                for command in commands {
                    match command {
                        wire::MicCommand::Record { seconds } => meter.record(*seconds),
                        wire::MicCommand::Stop => meter.stop(),
                    }
                }
            }
        });
        meter
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
        Status::Gone => (wire::MediaStatus::Gone, String::new()),
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

    #[cfg(not(feature = "av"))]
    fn camera() -> wire::Node {
        wire::Node::Camera {
            id: wire::ElementIdWire::Name("camera".into()),
            style: crate::render::plain_style(),
            device: None,
            mirror: true,
            object_fit: wire::ImageObjectFit::Cover,
            on_state: None,
        }
    }

    #[cfg(not(feature = "av"))]
    fn microphone() -> wire::Node {
        wire::Node::Microphone {
            id: wire::ElementIdWire::Name("mic".into()),
            style: crate::render::plain_style(),
            device: None,
            output: None,
            monitor: false,
            revision: 0,
            commands: Vec::new(),
            on_state: None,
        }
    }

    #[cfg(not(feature = "av"))]
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

    /// Each settings place brings up its own device's page, where the
    /// system has one, and the reader's account none.
    #[cfg(feature = "av")]
    #[test]
    fn a_settings_place_is_its_own_devices_page() {
        use ducktape_av::Kind;
        use view_wire::methods::Place;
        let camera = settings_page(Place::CameraSettings);
        let microphone = settings_page(Place::MicrophoneSettings);
        assert_eq!(camera, ducktape_av::settings_page(Kind::Camera));
        assert_eq!(microphone, ducktape_av::settings_page(Kind::Microphone));
        assert!(camera.is_none() || camera != microphone);
        assert_eq!(settings_page(Place::Account), None);
    }

    /// A device's state reaches its view in the wire's words, bounded;
    /// a refusal says how to allow the device here, and only macOS offers
    /// its settings page.
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
    }
}
