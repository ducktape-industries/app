//! The camera and microphone pieces (`wire::Node::Camera`,
//! `wire::Node::Microphone`) on this host, and every call the rest of the
//! app makes into the av feature. With feature `av` a camera piece is a
//! `ducktape_av::Camera` and a microphone piece a `ducktape_av::Meter`
//! (crates/av, where every device call lives), one per node, kept by the
//! node's authored path from the frame that first has it to the frame
//! that no longer does (its device released then), and made again when
//! its device changes; without it each piece says the build has none and
//! the methods are refused `media_unsupported`. Callers need no `cfg`:
//! - the tree: [`ViewTree::camera`] and [`ViewTree::microphone`] draw a
//!   piece, [`Media::retain`] keeps a frame's pieces;
//! - a view's frame: [`declared`] refuses a piece from a view whose
//!   manifest does not declare its capability;
//! - the methods: [`cameras`] and [`microphones`] answer `camera.devices`
//!   and `microphone.devices` (`runtime/kernel.rs`).

use super::*;
use view_wire::methods::{Capability, refusal};

/// Why `frame` is refused, if it carries a camera piece and its view's
/// manifest does not declare `camera`, or a microphone piece and it does
/// not declare `microphone`. The refusal ends the view, as the web
/// piece's does.
pub(crate) fn declared(frame: &wire::Frame, capabilities: &[Capability]) -> Result<(), String> {
    let pieces = [
        (Capability::Camera, wire::media::carries_camera(frame)),
        (
            Capability::Microphone,
            wire::media::carries_microphone(frame),
        ),
    ];
    for (capability, carried) in pieces {
        if carried && !capabilities.contains(&capability) {
            let name = capability.as_str();
            return Err(format!(
                "the view places a {name} piece, and its manifest does not declare `{name}`"
            ));
        }
    }
    Ok(())
}

/// Whether `link` is one of the system's privacy settings pages for the
/// camera or the microphone, the only links besides `duck://` and
/// `https://` that `link.open` takes: exactly the wire's two, and only on
/// macOS, where a refused device's state says the page opens
/// (`MediaState::settings`). Handed to the system as they are.
pub(crate) fn settings_link(link: &str) -> bool {
    cfg!(target_os = "macos")
        && [
            wire::media::SETTINGS_CAMERA,
            wire::media::SETTINGS_MICROPHONE,
        ]
        .contains(&link)
}

#[cfg(not(feature = "av"))]
fn unsupported() -> wire::Error {
    wire::Error::new(
        refusal::MEDIA_UNSUPPORTED,
        "this build has no camera or microphone",
    )
}

/// `camera.devices`: the cameras listed now.
/// ponytail: listed on the window thread; `spawn_blocking` and the
/// kernel's replies are the upgrade if a system lists slowly.
pub(crate) fn cameras() -> Result<Vec<u8>, wire::Error> {
    #[cfg(feature = "av")]
    return ducktape_av::cameras()
        .map(|found| {
            view_wire::methods::encode(&found.into_iter().filter_map(device).collect::<Vec<_>>())
        })
        .map_err(|error| wire::Error::new(refusal::HOST_FAULT, error));
    #[cfg(not(feature = "av"))]
    Err(unsupported())
}

/// `microphone.devices`: the microphones and speakers listed now.
pub(crate) fn microphones() -> Result<Vec<u8>, wire::Error> {
    #[cfg(feature = "av")]
    return ducktape_av::audio_devices()
        .map(|(inputs, outputs)| {
            view_wire::methods::encode(&view_wire::methods::AudioDevices {
                inputs: inputs.into_iter().filter_map(device).collect(),
                outputs: outputs.into_iter().filter_map(device).collect(),
            })
        })
        .map_err(|error| wire::Error::new(refusal::HOST_FAULT, error));
    #[cfg(not(feature = "av"))]
    Err(unsupported())
}

/// A listed device as the wire names it, its name cleaned as a window
/// title; none for an id past `MAX_DEVICE_BYTES`, which no piece could
/// send back.
#[cfg(feature = "av")]
fn device(found: ducktape_av::Device) -> Option<view_wire::methods::Device> {
    (found.id.len() <= wire::MAX_DEVICE_BYTES).then(|| view_wire::methods::Device {
        name: crate::runtime::shown_title(&found.name),
        id: found.id,
        default: found.default,
    })
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
        kind: Kind,
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
            fit,
            on_state,
            ..
        } = node
        else {
            unreachable!()
        };
        let mirror = *mirror;
        let fit = match fit {
            wire::Fit::Contain => ducktape_av::Fit::Contain,
            wire::Fit::Cover => ducktape_av::Fit::Cover,
        };
        let opened = (device.clone(), None);
        let device = device.clone();
        let piece = self.piece(
            opened,
            *on_state,
            Kind::Camera,
            |cx| Device::Camera(cx.new(|cx| ducktape_av::Camera::new(device, mirror, fit, cx))),
            cx,
        );
        let Device::Camera(camera) = &piece.device else {
            unreachable!("a camera's path holds a camera")
        };
        let camera = camera.clone();
        camera.update(cx, |camera, cx| {
            camera.set_mirror(mirror);
            camera.set_fit(fit, cx);
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
            Kind::Microphone,
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
                        wire::MicCommand::Chime => meter.chime(),
                    }
                }
            }
        });
        meter
    }
}

/// Which piece a state is told of: the platform's words for a refusal
/// name the device.
#[cfg(feature = "av")]
#[derive(Clone, Copy)]
enum Kind {
    Camera,
    Microphone,
}

/// How to allow a refused device on this platform, in one line; and
/// whether the system's settings page for it opens from the app
/// (`link.open` of the wire's settings links, macOS only).
#[cfg(feature = "av")]
fn allow(kind: Kind) -> (String, bool) {
    let (device, portal) = match kind {
        Kind::Camera => ("Camera", "camera portal"),
        Kind::Microphone => ("Microphone", "sound settings"),
    };
    match cfg!(target_os = "macos") {
        true => (
            format!(
                "Open System Settings \u{2192} Privacy & Security \u{2192} {device}, turn Ducktape on, then try again."
            ),
            true,
        ),
        false => (
            format!("Check the device's permissions (or your desktop's {portal}), then try again."),
            false,
        ),
    }
}

/// What a view hears of its piece's device.
#[cfg(feature = "av")]
fn told(state: &ducktape_av::State, kind: Kind) -> wire::MediaState {
    use ducktape_av::{Playback, Status};
    let mut settings = false;
    let (status, detail) = match &state.status {
        Status::Starting => (wire::MediaStatus::Starting, String::new()),
        Status::Live => (wire::MediaStatus::Live, String::new()),
        Status::Denied => {
            let (line, opens) = allow(kind);
            settings = opens;
            (wire::MediaStatus::Denied, line)
        }
        Status::Busy => (wire::MediaStatus::Busy, String::new()),
        Status::Gone => (wire::MediaStatus::Gone, String::new()),
        Status::NoDevice => (wire::MediaStatus::NoDevice, String::new()),
        Status::Failed(why) => (wire::MediaStatus::Failed, why.clone()),
    };
    // one line, at most 200 bytes, as the wire says
    let mut detail = crate::runtime::shown_title(&detail);
    while detail.len() > 200 {
        detail.pop();
    }
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
        settings,
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

    fn camera() -> wire::Node {
        wire::Node::Camera {
            id: wire::ElementIdWire::Name("camera".into()),
            style: crate::render::plain_style(),
            device: None,
            mirror: true,
            fit: wire::Fit::Cover,
            on_state: None,
        }
    }

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

    fn holder(children: Vec<wire::Node>) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name("root".into())),
            style: crate::render::plain_style(),
            interactivity: Default::default(),
            children,
        })
    }

    /// A camera piece needs `camera` and a microphone piece `microphone`,
    /// in a tree, a patch or a tooltip: neither capability stands in for
    /// the other.
    #[test]
    fn each_piece_needs_its_own_capability() {
        let both = [Capability::Camera, Capability::Microphone];
        for (piece, own, other) in [
            (camera(), Capability::Camera, Capability::Microphone),
            (microphone(), Capability::Microphone, Capability::Camera),
        ] {
            let frames = [
                wire::Frame {
                    root: Some(holder(vec![holder(vec![piece.clone()])])),
                    ..Default::default()
                },
                wire::Frame {
                    patches: vec![wire::Patch::Insert {
                        path: Vec::new(),
                        index: 0,
                        node: piece.clone(),
                    }],
                    ..Default::default()
                },
                wire::Frame {
                    tooltip_responses: vec![wire::TooltipResponse {
                        request: 1,
                        character_index: None,
                        content: Some(Box::new(piece.clone())),
                    }],
                    ..Default::default()
                },
            ];
            for frame in &frames {
                assert!(declared(frame, &[Capability::Host, other]).is_err());
                assert!(declared(frame, &[own]).is_ok());
                assert!(declared(frame, &both).is_ok());
            }
        }
        let plain = wire::Frame {
            root: Some(holder(Vec::new())),
            ..Default::default()
        };
        assert!(declared(&plain, &[]).is_ok());
    }

    /// Without the feature both methods are refused `media_unsupported`.
    #[cfg(not(feature = "av"))]
    #[test]
    fn a_build_without_av_refuses_the_device_lists() {
        for answer in [cameras(), microphones()] {
            assert_eq!(answer.unwrap_err().code, refusal::MEDIA_UNSUPPORTED);
        }
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
            Kind::Microphone,
        );
        assert_eq!(state.status, wire::MediaStatus::Failed);
        assert!(state.detail.len() <= 200 && !state.detail.contains('\n'));
        assert_eq!(state.device, "Camera");
        assert_eq!(state.playback, wire::Playback::Playing);
        assert!(state.speaking && !state.settings);
        let denied = told(
            &ducktape_av::State {
                status: ducktape_av::Status::Denied,
                ..Default::default()
            },
            Kind::Camera,
        );
        assert_eq!(denied.status, wire::MediaStatus::Denied);
        assert_eq!(denied.settings, cfg!(target_os = "macos"));
        let word = if cfg!(target_os = "macos") {
            "Camera, turn Ducktape on"
        } else {
            "camera portal"
        };
        assert!(denied.detail.contains(word), "{}", denied.detail);
        assert!(denied.detail.len() <= 200);
    }
}
