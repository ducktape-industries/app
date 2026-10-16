//! iOS: the cameras an iPhone has, and the app's audio session, whose
//! routes are the microphones and speakers. A camera runs as on macOS
//! ([`crate::apple`]: the permission, the capture session), its pictures
//! turned upright for the portrait screen ([`upright`]).
//!
//! The audio session is a call's while a microphone holds it: play and
//! record, on the loudspeaker unless a headset is in, Bluetooth headsets
//! and AirPods taken; the chime alone plays without recording
//! ([`configured`]). cpal opens the one audio unit the session routes,
//! so the devices listed are the session's routes ([`audio_devices`]): a
//! microphone is one of its inputs, made the preferred one when picked; a
//! speaker is the loudspeaker (an override when picked) or what the
//! current route plays on. A Bluetooth headset is microphone and speaker
//! at once: picking it for either moves both. The session is active while
//! a microphone is open or the chime plays ([`activate`]); the last one
//! to end deactivates it, so the person's music plays on.
//!
//! A call, an alarm or another app taking the microphone ends the piece
//! as Gone, with the reason; a camera another has is Busy, at open or
//! taken from this one. The app going to the background ends either as
//! Gone (the system stops the camera there; Try again opens it). A route
//! that moves under an open microphone is told to it: a picked
//! microphone no longer heard is Gone, the monitor stops on the
//! loudspeaker ([`crate::backend::moved`]).
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_av_foundation::{
    AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDevicePosition,
    AVCaptureDeviceTypeBuiltInUltraWideCamera, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureSessionInterruptionReason, AVCaptureSessionInterruptionReasonKey,
    AVCaptureVideoDataOutput,
};
use objc2_avf_audio::{
    AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayAndRecord,
    AVAudioSessionCategoryPlayback, AVAudioSessionInterruptionNotification,
    AVAudioSessionInterruptionType, AVAudioSessionInterruptionTypeKey,
    AVAudioSessionMediaServicesWereLostNotification,
    AVAudioSessionMediaServicesWereResetNotification, AVAudioSessionModeDefault,
    AVAudioSessionPort, AVAudioSessionPortAirPlay, AVAudioSessionPortBluetoothHFP,
    AVAudioSessionPortBluetoothLE, AVAudioSessionPortBuiltInReceiver,
    AVAudioSessionPortBuiltInSpeaker, AVAudioSessionPortCarAudio, AVAudioSessionPortDescription,
    AVAudioSessionPortHDMI, AVAudioSessionPortHeadphones, AVAudioSessionPortHeadsetMic,
    AVAudioSessionPortOverride, AVAudioSessionRouteChangeNotification,
    AVAudioSessionRouteChangePreviousRouteKey, AVAudioSessionRouteChangeReason,
    AVAudioSessionRouteChangeReasonKey, AVAudioSessionRouteDescription,
    AVAudioSessionSetActiveOptions,
};
use objc2_foundation::{
    NSArray, NSError, NSNotification, NSNotificationCenter, NSNotificationName, NSNumber, NSString,
};

use crate::backend::{AudioDevice, AudioKind, CameraDevice, Device, Facing, MicFeed, Status};

pub(crate) use crate::apple::{camera_allowed, microphone_allowed, open};

// ---------- what a person is told ----------

/// The app's own page in Settings, where its camera and microphone
/// switches are: UIKit's `UIApplicationOpenSettingsURLString`. One page
/// for both.
pub(crate) fn settings_page(_: crate::Kind) -> Option<&'static str> {
    Some("app-settings:")
}

/// How a person allows a camera or a microphone the system refused: on
/// the app's page ([`settings_page`]).
pub(crate) fn allow(kind: crate::Kind) -> &'static str {
    match kind {
        crate::Kind::Camera => "Open Settings, turn Camera on, then try again.",
        crate::Kind::Microphone => "Open Settings, turn Microphone on, then try again.",
    }
}

// ---------- the cameras ----------

/// The cameras the phone lists now, the front one first (a call shows the
/// person): each wide-angle camera and the back ultra-wide one. Listing
/// asks for no permission. An iPhone takes no plugged-in camera, so none
/// is External.
pub(crate) fn cameras() -> Result<Vec<CameraDevice>, String> {
    autoreleasepool(|_| {
        let video = crate::apple::video()?;
        // SAFETY: device type constants there since iOS 10 and 13; the
        // app's floor is 15
        let kinds = unsafe {
            NSArray::from_slice(&[
                AVCaptureDeviceTypeBuiltInWideAngleCamera,
                AVCaptureDeviceTypeBuiltInUltraWideCamera,
            ])
        };
        // SAFETY: a discovery session over constant types, any position
        let found = unsafe {
            AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                &kinds,
                Some(video),
                AVCaptureDevicePosition::Unspecified,
            )
        };
        // SAFETY: as above
        let mut listed: Vec<CameraDevice> = unsafe { found.devices() }
            .iter()
            .map(|device| {
                // SAFETY: plain properties of a listed device
                let (id, name) = unsafe { (device.uniqueID(), device.localizedName()) };
                let id = id.to_string();
                CameraDevice {
                    device: Device {
                        default: false,
                        name: name.to_string(),
                        id: id.clone(),
                    },
                    facing: facing(&device),
                    handle: id,
                }
            })
            .collect();
        // stable: the system's order within each side
        listed.sort_by_key(|camera| camera.facing != Facing::Front);
        if let Some(first) = listed.first_mut() {
            first.device.default = true;
        }
        Ok(listed)
    })
}

/// Which way `device` looks, where the phone says it sits.
fn facing(device: &AVCaptureDevice) -> Facing {
    // SAFETY: a plain property of a listed device
    match unsafe { device.position() } {
        AVCaptureDevicePosition::Front => Facing::Front,
        AVCaptureDevicePosition::Back => Facing::Back,
        _ => Facing::External,
    }
}

/// Turns `output`'s pictures upright for the portrait screen (a phone's
/// camera sensor lies on its side) and leaves mirroring to the piece:
/// the system mirrors nothing. Called once the output is in its session,
/// which makes the connection.
pub(crate) fn upright(output: &AVCaptureVideoDataOutput) {
    let Ok(video) = crate::apple::video() else {
        return;
    };
    // SAFETY: the connection's documented setters, each after the check
    // its documentation names (a setter it refuses throws)
    unsafe {
        let Some(connection) = output.connectionWithMediaType(video) else {
            return;
        };
        #[expect(
            deprecated,
            reason = "videoRotationAngle is iOS 17's; the app runs from iOS 15"
        )]
        if connection.isVideoOrientationSupported() {
            use objc2_av_foundation::AVCaptureVideoOrientation;
            connection.setVideoOrientation(AVCaptureVideoOrientation::Portrait);
        }
        if connection.isVideoMirroringSupported() {
            connection.setAutomaticallyAdjustsVideoMirroring(false);
            connection.setVideoMirrored(false);
        }
    }
}

/// What a camera's session interrupted says: Busy while a call or
/// another app has the camera (at open, or taking it from this one), else
/// Gone with the line a person reads.
pub(crate) fn interrupted(notification: &NSNotification) -> Status {
    // SAFETY: an AVFoundation constant, there since iOS 9
    let key = unsafe { AVCaptureSessionInterruptionReasonKey };
    let reason = number(notification, key).map(AVCaptureSessionInterruptionReason);
    let why = match reason {
        Some(
            AVCaptureSessionInterruptionReason::AudioDeviceInUseByAnotherClient
            | AVCaptureSessionInterruptionReason::VideoDeviceInUseByAnotherClient,
        ) => return Status::Busy,
        Some(AVCaptureSessionInterruptionReason::VideoDeviceNotAvailableInBackground) => {
            "The camera stopped while the app was in the background."
        }
        Some(
            AVCaptureSessionInterruptionReason::VideoDeviceNotAvailableWithMultipleForegroundApps,
        ) => "The camera works only with the app on the whole screen.",
        Some(AVCaptureSessionInterruptionReason::VideoDeviceNotAvailableDueToSystemPressure) => {
            "The phone stopped the camera to cool down."
        }
        _ => "The phone stopped the camera.",
    };
    Status::Gone(why.into())
}

/// The integer under `key` in `notification`'s user info.
fn number(notification: &NSNotification, key: &NSString) -> Option<isize> {
    let info = notification.userInfo()?;
    let value = info.objectForKey(key)?;
    Some(value.downcast::<NSNumber>().ok()?.integerValue())
}

// ---------- the audio session ----------

/// How many hold the session active ([`Active`]), and how many of them
/// are microphones.
struct Holds {
    all: usize,
    microphones: usize,
}

static ACTIVE: Mutex<Holds> = Mutex::new(Holds {
    all: 0,
    microphones: 0,
});

/// Whether the session records: while a microphone holds it, or one is
/// coming (`microphone`). The chime alone plays without: a session that
/// records raises the microphone's prompt when it goes active, and a
/// speaker test asks for no microphone.
fn records(holds: &Holds, microphone: bool) -> bool {
    microphone || holds.microphones > 0
}

/// The app's audio session as listing and routing find it: in the
/// category its holds need, and while nothing holds it set up for the
/// next microphone (whose inputs it lists).
fn session() -> Result<Retained<AVAudioSession>, String> {
    let holds = ACTIVE.lock().expect("audio session");
    configured(holds.all == 0 || records(&holds, false))
}

/// The app's audio session set for a call (`record`: play and record, on
/// the loudspeaker unless a headset is in, Bluetooth headsets
/// (hands-free) and headphones taken) or for the chime alone (plain
/// playback, where the system routes it), in the default mode: video
/// chat mode's voice processing is for an audio unit cpal does not open,
/// and it lowers the sound. Set only when it is not already: setting it
/// again is a route change. Setting it asks for nothing (the
/// microphone's prompt comes when a recording session goes active).
fn configured(record: bool) -> Result<Retained<AVAudioSession>, String> {
    // SAFETY: AVFAudio constants, there since iOS 3, 6 and 5
    let (category, mode) = unsafe {
        let category = match record {
            true => AVAudioSessionCategoryPlayAndRecord,
            false => AVAudioSessionCategoryPlayback,
        };
        (
            category.ok_or("iOS lacks the audio category")?,
            AVAudioSessionModeDefault.ok_or("iOS has no default audio mode")?,
        )
    };
    let options = match record {
        true => {
            AVAudioSessionCategoryOptions::DefaultToSpeaker
                | AVAudioSessionCategoryOptions::AllowBluetoothHFP
                | AVAudioSessionCategoryOptions::AllowBluetoothA2DP
        }
        false => AVAudioSessionCategoryOptions::empty(),
    };
    // SAFETY: the app's one session, its documented calls (any thread)
    unsafe {
        let session = AVAudioSession::sharedInstance();
        let set = *session.category() == *category
            && *session.mode() == *mode
            && session.categoryOptions().contains(options);
        if !set {
            session
                .setCategory_mode_options_error(category, mode, options)
                .map_err(|error| said(&error))?;
        }
        Ok(session)
    }
}

/// The id of the phone's loudspeaker among the speakers: always there,
/// whatever the route (an override reaches it).
const SPEAKER: &str = "speaker";

/// One listed route: the device, and the session input picking it
/// prefers (a microphone itself; a Bluetooth headset's, as a speaker).
struct Route {
    audio: AudioDevice,
    prefers: Option<Retained<AVAudioSessionPortDescription>>,
}

/// The microphones (`input`) or speakers the session routes to now, each
/// one a piece can name back (`backend::nameable`). Microphones: its
/// inputs, the one it hears now the default. Speakers: the loudspeaker,
/// what the route plays on now besides it (headphones, AirPlay) or what
/// the loudspeaker was picked over ([`REPLACED`]), and each Bluetooth
/// headset among the inputs. The earpiece is not one: the call session
/// plays on the loudspeaker instead.
fn routes(session: &AVAudioSession, input: bool) -> Vec<Route> {
    let mut listed = match input {
        true => inputs(session),
        false => outputs(session),
    };
    listed.retain(|route| crate::backend::nameable(&route.audio.device));
    listed
}

/// A session port as a listed device: its UID, its name, what it is.
fn row(port: &AVAudioSessionPortDescription, input: bool, default: bool) -> AudioDevice {
    // SAFETY: plain properties of a port the session listed
    let (id, name, kind) = unsafe { (port.UID(), port.portName(), port.portType()) };
    AudioDevice {
        device: Device {
            id: id.to_string(),
            name: name.to_string(),
            default,
        },
        kind: kind_of(&kind, input),
    }
}

/// A port's UID, which names it among the routes.
fn uid(port: &AVAudioSessionPortDescription) -> String {
    // SAFETY: a plain property of a port the session listed
    unsafe { port.UID() }.to_string()
}

/// The inputs the session has now, each preferred when picked.
fn available(session: &AVAudioSession) -> Vec<Retained<AVAudioSessionPortDescription>> {
    // SAFETY: the session's documented read
    let ports = unsafe { session.availableInputs() };
    ports.map(|ports| ports.to_vec()).unwrap_or_default()
}

/// The microphones: the session's inputs, the one it hears now the
/// default.
fn inputs(session: &AVAudioSession) -> Vec<Route> {
    // SAFETY: the session's documented reads
    let heard: Vec<String> = unsafe { session.currentRoute().inputs() }
        .iter()
        .map(|port| uid(&port))
        .collect();
    available(session)
        .into_iter()
        .map(|port| Route {
            audio: row(&port, true, heard.contains(&uid(&port))),
            prefers: Some(port),
        })
        .collect()
}

/// The speakers: the loudspeaker, what the route plays on now besides it,
/// and each Bluetooth headset among the inputs (picking it prefers its
/// microphone, which moves its sound there too).
fn outputs(session: &AVAudioSession) -> Vec<Route> {
    // SAFETY: AVFAudio constants
    let (loudspeaker, headset) = unsafe {
        (
            AVAudioSessionPortBuiltInSpeaker,
            AVAudioSessionPortBluetoothHFP,
        )
    };
    // SAFETY: a plain property of a port the session listed
    let speaker =
        |port: &AVAudioSessionPortDescription| is(&*unsafe { port.portType() }, &[loudspeaker]);
    // SAFETY: the session's documented reads
    let playing = unsafe { session.currentRoute().outputs() }.to_vec();
    let mut listed = vec![Route {
        audio: AudioDevice {
            device: Device {
                id: SPEAKER.into(),
                name: "Speaker".into(),
                default: playing.iter().any(|port| speaker(port)),
            },
            kind: AudioKind::Speaker,
        },
        prefers: None,
    }];
    for port in playing.iter().filter(|port| !speaker(port)) {
        listed.push(Route {
            audio: row(port, false, true),
            prefers: None,
        });
    }
    // what the loudspeaker was picked over, while it plays in its place
    let replaced = REPLACED.lock().expect("replaced output").clone();
    if let Some(audio) = replaced
        && listed[0].audio.device.default
        && !listed
            .iter()
            .any(|route| route.audio.device == audio.device)
    {
        listed.push(Route {
            audio,
            prefers: None,
        });
    }
    for port in available(session) {
        let id = uid(&port);
        // SAFETY: a plain property of a port the session listed
        let kind = unsafe { port.portType() };
        if is(&kind, &[headset]) && !listed.iter().any(|route| route.audio.device.id == id) {
            listed.push(Route {
                audio: row(&port, false, false),
                prefers: Some(port),
            });
        }
    }
    listed
}

/// Whether `port` is one of `types` (a constant the system lacks is none).
fn is(port: &AVAudioSessionPort, types: &[Option<&'static AVAudioSessionPort>]) -> bool {
    types.iter().flatten().any(|kind| **kind == *port)
}

/// What a session port is: only what is sure. A loudspeaker, the
/// earpiece, a screen's or a car's speakers, AirPlay (which the
/// microphone hears); a headset's microphone, headphones, a Bluetooth
/// headset or hearing aid; anything else (the built-in microphone, a
/// Bluetooth A2DP output, which may be a speaker) says Other.
fn kind_of(port: &AVAudioSessionPort, input: bool) -> AudioKind {
    // SAFETY: AVFAudio constants
    let worn = unsafe {
        [
            AVAudioSessionPortHeadsetMic,
            AVAudioSessionPortHeadphones,
            AVAudioSessionPortBluetoothHFP,
            AVAudioSessionPortBluetoothLE,
        ]
    };
    // SAFETY: as above
    let speakers = unsafe {
        [
            AVAudioSessionPortBuiltInSpeaker,
            AVAudioSessionPortHDMI,
            AVAudioSessionPortAirPlay,
            AVAudioSessionPortCarAudio,
        ]
    };
    if is(port, &worn) {
        AudioKind::Headphones
    } else if input {
        AudioKind::Other
    } else if is(port, &speakers) {
        AudioKind::Speaker
    // SAFETY: as above
    } else if is(port, &[unsafe { AVAudioSessionPortBuiltInReceiver }]) {
        AudioKind::Earpiece
    } else {
        AudioKind::Other
    }
}

/// The microphones and speakers the session routes to now.
pub(crate) fn audio_devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    autoreleasepool(|_| {
        let session = session()?;
        let audio = |input| {
            routes(&session, input)
                .into_iter()
                .map(|route| route.audio)
                .collect()
        };
        Ok((audio(true), audio(false)))
    })
}

/// Routes the session to the microphone (`input`) or speaker `id` it
/// lists now, before cpal opens its audio unit; `None` leaves the route
/// as it is (another piece's pick holds). Nothing when it lists no such
/// device: an id is resolved only against this listing. Picking the
/// loudspeaker remembers what it is picked over ([`REPLACED`]); picking
/// another speaker lifts it. A route the system refuses is logged and
/// the unit opens where the session is.
pub(crate) fn route(id: Option<&str>, input: bool) -> Option<()> {
    let Some(id) = id else {
        return Some(());
    };
    autoreleasepool(|_| {
        let session = session().ok()?;
        let picked = routes(&session, input)
            .into_iter()
            .find(|route| route.audio.device.id == id)?;
        // SAFETY: the session's documented setters, in its category
        let routed = unsafe {
            if input {
                session.setPreferredInput_error(picked.prefers.as_deref())
            } else if id == SPEAKER {
                remember(&session);
                session.overrideOutputAudioPort_error(AVAudioSessionPortOverride::Speaker)
            } else {
                *REPLACED.lock().expect("replaced output") = None;
                session
                    .overrideOutputAudioPort_error(AVAudioSessionPortOverride::None)
                    .and_then(|()| match &picked.prefers {
                        Some(port) => session.setPreferredInput_error(Some(port)),
                        None => Ok(()),
                    })
            }
        };
        if let Err(error) = routed {
            tracing::warn!(target: "ducktape::av", error = %said(&error), "the audio route stayed");
        }
        Some(())
    })
}

/// The speaker the loudspeaker was picked over (headphones without a
/// microphone, Bluetooth headphones, AirPlay), listed while the
/// loudspeaker plays in its place: the session lists only what plays,
/// and picking it lifts the override. Forgotten when another speaker is
/// picked, or a device comes or goes ([`forget_on_change`]).
static REPLACED: Mutex<Option<AudioDevice>> = Mutex::new(None);

/// Remembers what the route plays on now as the loudspeaker is picked
/// over it ([`REPLACED`]): not a Bluetooth headset (listed among the
/// inputs anyway) nor the earpiece (the call session plays on the
/// loudspeaker instead). What was remembered stays while the loudspeaker
/// plays already.
fn remember(session: &AVAudioSession) {
    // SAFETY: AVFAudio constants
    let (loudspeaker, kept) = unsafe {
        (
            [AVAudioSessionPortBuiltInSpeaker],
            [
                AVAudioSessionPortBluetoothHFP,
                AVAudioSessionPortBuiltInReceiver,
            ],
        )
    };
    // SAFETY: the session's documented read, and plain properties of the
    // ports it lists
    let playing: Vec<_> = unsafe { session.currentRoute().outputs() }
        .iter()
        .map(|port| (unsafe { port.portType() }, port))
        .collect();
    if playing.iter().any(|(kind, _)| is(kind, &loudspeaker)) {
        return;
    }
    *REPLACED.lock().expect("replaced output") = playing
        .iter()
        .find(|(kind, _)| !is(kind, &kept))
        .map(|(_, port)| row(port, false, false));
    forget_on_change();
}

/// Forgets [`REPLACED`] once a device comes or goes: the system lifts
/// the loudspeaker's override at such a route change, and what it was
/// picked over may be gone. Observed once, for the app's life.
fn forget_on_change() {
    static OBSERVED: std::sync::Once = std::sync::Once::new();
    OBSERVED.call_once(|| {
        let forget = RcBlock::new(|notification: NonNull<NSNotification>| {
            // SAFETY: the notification the center lends for this call
            if came_or_went(unsafe { notification.as_ref() }) {
                *REPLACED.lock().expect("replaced output") = None;
            }
        });
        // SAFETY: an AVFAudio constant; an observer block on the posting
        // thread, never removed
        if let Some(name) = unsafe { AVAudioSessionRouteChangeNotification } {
            let observer =
                unsafe {
                    NSNotificationCenter::defaultCenter()
                        .addObserverForName_object_queue_usingBlock(Some(name), None, None, &forget)
                };
            // kept for the app's life
            std::mem::forget(observer);
        }
    });
}

/// Whether a route change came of a device coming or going.
fn came_or_went(notification: &NSNotification) -> bool {
    // SAFETY: an AVFAudio constant
    let Some(key) = (unsafe { AVAudioSessionRouteChangeReasonKey }) else {
        return false;
    };
    let reason =
        number(notification, key).map(|reason| AVAudioSessionRouteChangeReason(reason as usize));
    matches!(
        reason,
        Some(
            AVAudioSessionRouteChangeReason::NewDeviceAvailable
                | AVAudioSessionRouteChangeReason::OldDeviceUnavailable
        )
    )
}

/// cpal's one audio unit, the session routed first to `id`, one of its
/// routes ([`route`]: an id it does not list opens nothing).
pub(crate) fn find(host: &cpal::Host, id: Option<&str>, input: bool) -> Option<cpal::Device> {
    use cpal::traits::HostTrait as _;
    route(id, input)?;
    match input {
        true => host.default_input_device(),
        false => host.default_output_device(),
    }
}

/// The session routes cpal's one stream in place: a route change
/// invalidates it, though it plays on (its ends come from [`activate`]).
pub(crate) const ROUTED_IN_PLACE: bool = true;

/// The name a person knows the opened microphone by: the port the
/// session hears ([`heard`]), cpal's unit naming none.
pub(crate) fn input_name(input: &cpal::Device) -> String {
    use cpal::traits::DeviceTrait as _;
    heard().map(|heard| heard.name).unwrap_or_else(|| {
        input
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_default()
    })
}

/// The microphone the session hears now, as listed.
pub(crate) fn heard() -> Option<Device> {
    autoreleasepool(|_| {
        // SAFETY: the session's documented reads
        let inputs = unsafe { AVAudioSession::sharedInstance().currentRoute().inputs() };
        inputs
            .firstObject()
            .map(|port| row(&port, true, true).device)
    })
}

/// Whether a route change moved the sound onto the loudspeaker from
/// elsewhere (the headphones gone, the loudspeaker picked over them).
/// Where the change does not say what played before, the loudspeaker
/// playing now is enough.
fn fell(notification: &NSNotification) -> bool {
    // SAFETY: AVFAudio constants, and the session's and its routes'
    // documented reads
    unsafe {
        let loudspeaker = |route: &AVAudioSessionRouteDescription| {
            let outputs = route.outputs();
            outputs
                .iter()
                .any(|port| is(&port.portType(), &[AVAudioSessionPortBuiltInSpeaker]))
        };
        let before = AVAudioSessionRouteChangePreviousRouteKey.and_then(|key| {
            let info = notification.userInfo()?;
            info.objectForKey(key)?
                .downcast::<AVAudioSessionRouteDescription>()
                .ok()
        });
        let now = AVAudioSession::sharedInstance().currentRoute();
        loudspeaker(&now) && before.is_none_or(|before| !loudspeaker(&before))
    }
}

/// The session held active, for a microphone or the chime; with a
/// microphone's feed, its ends (an interruption, the media services
/// lost) are told to it as Gone, and its route's moves
/// ([`crate::backend::moved`]). The last one dropped deactivates the
/// session, telling the other apps they may play again.
pub(crate) struct Active {
    microphone: bool,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

/// The session active from now until the returned hold drops, recording
/// only for a microphone ([`records`]); `microphone` the open
/// microphone's feed, which hears how it ends and where its route moves,
/// and the microphone its piece named (`None`: the default). A call that
/// has the audio refuses it (Busy).
pub(crate) fn activate(
    microphone: Option<(&Arc<MicFeed>, Option<&str>)>,
) -> Result<Active, Status> {
    {
        let mut holds = ACTIVE.lock().expect("audio session");
        let session = configured(records(&holds, microphone.is_some())).map_err(Status::Failed)?;
        if holds.all == 0 {
            // SAFETY: the session's documented activation, off the main
            // thread (it can block)
            unsafe { session.setActive_error(true) }.map_err(|error| refused(&error))?;
        }
        holds.all += 1;
        holds.microphones += usize::from(microphone.is_some());
    }
    Ok(Active {
        microphone: microphone.is_some(),
        observers: microphone
            .map(|(feed, named)| watch(feed, named))
            .unwrap_or_default(),
    })
}

impl Drop for Active {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.observers {
            // SAFETY: an observer this hold added
            unsafe { center.removeObserver(observer.as_ref()) };
        }
        let mut holds = ACTIVE.lock().expect("audio session");
        holds.all -= 1;
        holds.microphones -= usize::from(self.microphone);
        if holds.all == 0 {
            // SAFETY: the session's documented deactivation, every stream
            // of this hold's closed before it drops
            let ended = unsafe {
                AVAudioSession::sharedInstance().setActive_withOptions_error(
                    false,
                    AVAudioSessionSetActiveOptions::NotifyOthersOnDeactivation,
                )
            };
            if let Err(error) = ended {
                tracing::warn!(target: "ducktape::av", error = %said(&error), "the audio session stayed active");
            }
        }
    }
}

/// Ends `feed`'s microphone, told Gone with the reason once its streams
/// closed ([`MicFeed::end`]), when the session is interrupted (a call, an
/// alarm, another app's audio, the app suspended) or its media services
/// are lost or reset, and where its route moved (a
/// headset in or out, the loudspeaker picked: the microphone its piece
/// `named` no longer heard, the monitor on the loudspeaker): the
/// observers, until removed.
fn watch(
    feed: &Arc<MicFeed>,
    named: Option<&str>,
) -> Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>> {
    let center = NSNotificationCenter::defaultCenter();
    let observe = |name: Option<&'static NSNotificationName>,
                   why: fn(&NSNotification) -> Option<&'static str>| {
        let feed = feed.clone();
        let gone = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: the notification the center lends for this call
            if let Some(why) = why(unsafe { notification.as_ref() }) {
                feed.end(Status::Gone(why.into()));
            }
        });
        // SAFETY: an observer block on the posting thread; removed when
        // the hold drops
        name.map(|name| unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &gone)
        })
    };
    // SAFETY: AVFAudio constants
    let (interruption, lost, reset) = unsafe {
        (
            AVAudioSessionInterruptionNotification,
            AVAudioSessionMediaServicesWereLostNotification,
            AVAudioSessionMediaServicesWereResetNotification,
        )
    };
    let routed = {
        let (feed, named) = (feed.clone(), named.map(str::to_owned));
        RcBlock::new(move |notification: NonNull<NSNotification>| {
            let moved = crate::backend::Moved {
                heard: heard(),
                // SAFETY: the notification the center lends for this call
                to_loudspeaker: fell(unsafe { notification.as_ref() }),
            };
            crate::backend::moved(&feed, named.as_deref(), moved);
        })
    };
    // SAFETY: an AVFAudio constant; an observer block on the posting
    // thread, removed when the hold drops
    let route = unsafe { AVAudioSessionRouteChangeNotification }.map(|name| unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &routed)
    });
    [
        observe(interruption, interruption_began),
        observe(lost, |_| Some("The phone's audio stopped.")),
        observe(reset, |_| Some("The phone's audio stopped.")),
        route,
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Why the microphone stopped, for an interruption that began; none for
/// one that ended. iOS no longer says when the app's suspension was the
/// reason.
fn interruption_began(notification: &NSNotification) -> Option<&'static str> {
    // SAFETY: an AVFAudio constant
    let kind = unsafe { AVAudioSessionInterruptionTypeKey }?;
    let began = number(notification, kind)? as usize == AVAudioSessionInterruptionType::Began.0;
    began.then_some(crate::MICROPHONE_TAKEN)
}

/// A session error, as a status: a call that has the audio is Busy.
fn refused(error: &NSError) -> Status {
    const INSUFFICIENT_PRIORITY: u32 = u32::from_be_bytes(*b"!pri");
    const CANNOT_INTERRUPT_OTHERS: u32 = u32::from_be_bytes(*b"!int");
    match u32::try_from(error.code()) {
        Ok(INSUFFICIENT_PRIORITY | CANNOT_INTERRUPT_OTHERS) => Status::Busy,
        _ => Status::Failed(said(error)),
    }
}

/// A session error's words.
fn said(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_avf_audio::{AVAudioSessionPortBluetoothA2DP, AVAudioSessionPortBuiltInMic};
    use objc2_foundation::NSDictionary;

    /// A session port is what its type says, only what is sure.
    #[test]
    fn a_port_is_what_its_type_says() {
        let kind = |port: Option<&'static AVAudioSessionPort>, input| kind_of(port.unwrap(), input);
        // SAFETY: AVFAudio constants
        unsafe {
            assert_eq!(
                kind(AVAudioSessionPortBuiltInSpeaker, false),
                AudioKind::Speaker
            );
            assert_eq!(
                kind(AVAudioSessionPortBuiltInReceiver, false),
                AudioKind::Earpiece
            );
            assert_eq!(
                kind(AVAudioSessionPortHeadphones, false),
                AudioKind::Headphones
            );
            assert_eq!(
                kind(AVAudioSessionPortBluetoothHFP, false),
                AudioKind::Headphones
            );
            assert_eq!(kind(AVAudioSessionPortAirPlay, false), AudioKind::Speaker);
            assert_eq!(
                kind(AVAudioSessionPortBluetoothA2DP, false),
                AudioKind::Other
            );
            assert_eq!(kind(AVAudioSessionPortBuiltInMic, true), AudioKind::Other);
            assert_eq!(
                kind(AVAudioSessionPortHeadsetMic, true),
                AudioKind::Headphones
            );
            assert_eq!(
                kind(AVAudioSessionPortBluetoothHFP, true),
                AudioKind::Headphones
            );
            assert_eq!(kind(AVAudioSessionPortCarAudio, true), AudioKind::Other);
        }
    }

    /// A call that has the audio refuses the session as Busy; another
    /// error says its words.
    #[test]
    fn a_call_that_has_the_audio_is_busy() {
        let domain = NSString::from_str("NSOSStatusErrorDomain");
        let error = |code: &[u8; 4]| NSError::new(u32::from_be_bytes(*code) as isize, &domain);
        assert_eq!(refused(&error(b"!pri")), Status::Busy);
        assert_eq!(refused(&error(b"!int")), Status::Busy);
        assert!(matches!(refused(&error(b"what")), Status::Failed(_)));
    }

    /// A notification named `name` whose user info holds `value` under `key`.
    fn told(name: &NSNotificationName, key: &NSString, value: isize) -> Retained<NSNotification> {
        let value = NSNumber::new_isize(value);
        let info = NSDictionary::<NSString, NSNumber>::from_slices(&[key], &[&value]);
        // SAFETY: the same dictionary, its key and object types erased
        let info =
            unsafe { &*(&*info as *const NSDictionary<NSString, NSNumber>).cast::<NSDictionary>() };
        // SAFETY: a notification made here, read by the test alone
        unsafe { NSNotification::notificationWithName_object_userInfo(name, None, Some(info)) }
    }

    /// An interruption that began stops the microphone and says why; one
    /// that ended says nothing; a camera another has is Busy, and a
    /// camera stopped otherwise says its reason.
    #[test]
    fn an_interruption_says_why() {
        // SAFETY: AVFoundation and AVFAudio constants
        let (audio, kind, camera, reason) = unsafe {
            (
                AVAudioSessionInterruptionNotification.unwrap(),
                AVAudioSessionInterruptionTypeKey.unwrap(),
                objc2_av_foundation::AVCaptureSessionWasInterruptedNotification,
                AVCaptureSessionInterruptionReasonKey,
            )
        };
        let began = AVAudioSessionInterruptionType::Began.0 as isize;
        let ended = AVAudioSessionInterruptionType::Ended.0 as isize;
        assert!(interruption_began(&told(audio, kind, began)).is_some());
        assert_eq!(interruption_began(&told(audio, kind, ended)), None);
        let taken = AVCaptureSessionInterruptionReason::VideoDeviceInUseByAnotherClient.0;
        assert_eq!(interrupted(&told(camera, reason, taken)), Status::Busy);
        let away = AVCaptureSessionInterruptionReason::VideoDeviceNotAvailableInBackground.0;
        assert!(matches!(
            interrupted(&told(camera, reason, away)),
            Status::Gone(why) if why.contains("background")
        ));
    }

    /// One test at a time sets the app's one session.
    static ONE_SESSION: Mutex<()> = Mutex::new(());

    /// The chime alone plays in a session that does not record, so a
    /// speaker test never raises the microphone's prompt; a microphone,
    /// coming or holding, makes it a call's, in the default mode. Nothing
    /// is activated.
    #[test]
    fn only_a_microphone_makes_the_session_record() {
        let _one = ONE_SESSION.lock().unwrap();
        let holds = |all, microphones| Holds { all, microphones };
        assert!(!records(&holds(0, 0), false));
        assert!(!records(&holds(1, 0), false));
        assert!(records(&holds(0, 0), true));
        assert!(records(&holds(2, 1), false));
        // SAFETY: AVFAudio constants and the session's documented reads
        unsafe {
            let chime = configured(false).unwrap();
            assert_eq!(*chime.category(), *AVAudioSessionCategoryPlayback.unwrap());
            let call = configured(true).unwrap();
            assert_eq!(
                *call.category(),
                *AVAudioSessionCategoryPlayAndRecord.unwrap()
            );
            assert_eq!(*call.mode(), *AVAudioSessionModeDefault.unwrap());
            assert!(
                call.categoryOptions()
                    .contains(AVAudioSessionCategoryOptions::DefaultToSpeaker)
            );
        }
    }

    /// What the loudspeaker was picked over stays listed while the
    /// loudspeaker plays in its place, and picking it lifts the override;
    /// a device coming or going forgets it, the override's own route
    /// change does not. Nothing is activated.
    #[test]
    fn what_the_loudspeaker_replaced_stays_listed() {
        let _one = ONE_SESSION.lock().unwrap();
        let headphones = AudioDevice {
            device: Device {
                id: "test-headphones".into(),
                name: "Headphones".into(),
                default: false,
            },
            kind: AudioKind::Headphones,
        };
        let listed = || {
            let outputs = audio_devices().unwrap().1;
            outputs
                .iter()
                .any(|output| output.device == headphones.device)
        };
        // the simulator plays on its loudspeaker
        assert!(audio_devices().unwrap().1[0].device.default);
        *REPLACED.lock().unwrap() = Some(headphones.clone());
        assert!(listed());
        assert!(route(Some("test-headphones"), false).is_some());
        assert!(!listed());
        forget_on_change();
        // SAFETY: AVFAudio constants
        let (name, key) = unsafe {
            (
                AVAudioSessionRouteChangeNotification.unwrap(),
                AVAudioSessionRouteChangeReasonKey.unwrap(),
            )
        };
        let changed = |reason: AVAudioSessionRouteChangeReason| {
            *REPLACED.lock().unwrap() = Some(headphones.clone());
            let told = told(name, key, reason.0 as isize);
            NSNotificationCenter::defaultCenter().postNotification(&told);
            listed()
        };
        assert!(!changed(
            AVAudioSessionRouteChangeReason::OldDeviceUnavailable
        ));
        assert!(!changed(
            AVAudioSessionRouteChangeReason::NewDeviceAvailable
        ));
        assert!(changed(AVAudioSessionRouteChangeReason::Override));
        *REPLACED.lock().unwrap() = None;
    }

    /// A route change under an open microphone is told to its feed: from
    /// a route that played elsewhere to the loudspeaker, the monitor
    /// stops; the microphone its piece named, no longer heard, is gone.
    /// Nothing is activated.
    #[test]
    fn a_route_change_is_told_to_the_microphone() {
        let _one = ONE_SESSION.lock().unwrap();
        let feed = Arc::new(MicFeed::default());
        feed.status.lock().unwrap().0 = Status::Live;
        feed.ask_monitor(true);
        let observers = watch(&feed, Some("no-such-microphone"));
        // SAFETY: AVFAudio constants; an empty route, played on nothing
        let (name, key, before) = unsafe {
            (
                AVAudioSessionRouteChangeNotification.unwrap(),
                AVAudioSessionRouteChangePreviousRouteKey.unwrap(),
                AVAudioSessionRouteDescription::new(),
            )
        };
        let info = NSDictionary::<NSString, AVAudioSessionRouteDescription>::from_slices(
            &[key],
            &[&*before],
        );
        // SAFETY: the same dictionary, its key and object types erased
        let info = unsafe {
            &*(&*info as *const NSDictionary<NSString, AVAudioSessionRouteDescription>)
                .cast::<NSDictionary>()
        };
        // SAFETY: a notification made here, posted to this process alone
        let told =
            unsafe { NSNotification::notificationWithName_object_userInfo(name, None, Some(info)) };
        // the simulator plays on its loudspeaker
        assert!(audio_devices().unwrap().1[0].device.default);
        NSNotificationCenter::defaultCenter().postNotification(&told);
        assert!(!feed.monitoring());
        assert!(matches!(feed.ended(), Some(Status::Gone(_))));
        let center = NSNotificationCenter::defaultCenter();
        for observer in &observers {
            // SAFETY: an observer this test added
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }

    /// A piece left to the default leaves the route another picked: the
    /// preferred microphone stays. Nothing is activated.
    #[test]
    fn a_default_piece_leaves_the_route_as_it_is() {
        let _one = ONE_SESSION.lock().unwrap();
        let (inputs, _) = audio_devices().unwrap();
        let picked = &inputs[0].device.id;
        assert!(route(Some(picked), true).is_some());
        assert!(route(None, true).is_some());
        assert!(route(None, false).is_some());
        // SAFETY: the app's one session, its documented read and setter
        unsafe {
            let session = AVAudioSession::sharedInstance();
            let preferred = session.preferredInput().map(|port| uid(&port));
            assert_eq!(preferred.as_ref(), Some(picked));
            session.setPreferredInput_error(None).unwrap();
        }
    }

    /// The session lists the loudspeaker first among the speakers, routes
    /// to it, and routes to nothing it does not list. Nothing is opened
    /// or activated; the cameras are listed front first.
    #[test]
    fn the_session_lists_and_routes_only_its_own() {
        let _one = ONE_SESSION.lock().unwrap();
        let (inputs, outputs) = audio_devices().unwrap();
        eprintln!("inputs: {inputs:?}\noutputs: {outputs:?}");
        assert_eq!(
            (outputs[0].device.id.as_str(), outputs[0].kind),
            (SPEAKER, AudioKind::Speaker)
        );
        assert!(
            outputs
                .iter()
                .filter(|output| output.device.default)
                .count()
                <= 1
        );
        assert!(route(Some(SPEAKER), false).is_some());
        assert!(route(None, false).is_some());
        for id in ["no-such", "speaker ", "Speaker", ""] {
            assert!(route(Some(id), false).is_none(), "{id:?}");
            assert!(route(Some(id), true).is_none(), "{id:?}");
        }
        let cameras = cameras().unwrap();
        eprintln!("cameras: {cameras:?}");
        let fronts = cameras
            .iter()
            .take_while(|c| c.facing == Facing::Front)
            .count();
        assert!(cameras[fronts..].iter().all(|c| c.facing != Facing::Front));
        assert!(cameras.iter().skip(1).all(|camera| !camera.device.default));
    }
}
