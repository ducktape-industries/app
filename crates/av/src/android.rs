//! Android: the camera through the NDK's Camera2, the permission through
//! the app's Activity, and the microphones and speakers as cpal's AAudio
//! host lists them, named and marked by what Android says of them
//! (`android_rules`, built for every OS's tests so they run on Linux).
//!
//! The JNI calls go through the app's Context and its Java VM, which the
//! phone's glue hands `ndk-context` once (cpal's AAudio host reads the
//! same). Only an Activity can ask the person for a permission, so the
//! glue also hands in how it asks ([`asks_through`]): the ask blocks the
//! device's thread until the person answers, never the window's.
//!
//! One opened camera is a Camera2 device and a capture session repeating
//! a preview request into an image reader of ours (YUV 4:2:0). The
//! reader's listener only says a frame came; the stream loop
//! ([`crate::backend::stream`]) takes the newest one on its own thread and
//! turns it into a picture, upright for the display, while one is wanted.
//! The device's disconnect (another app or the system took the camera:
//! Android closes a backgrounded app's camera) and its errors come the
//! same way. Dropping a [`Camera`] closes the session and the device (the
//! light off), then frees the rest.
//!
//! Each microphone and speaker stream opens on the device picked (AAudio's
//! own routing) and ends with it. While a microphone is open ([`activate`])
//! a watcher looks every half second at what Android tells an app only
//! when asked: whether it silenced the recording (a call, the app off the
//! screen), which ends the microphone, and whether the monitor's sound
//! fell onto the loudspeaker, which turns the monitor off.
use std::ffi::{CStr, CString, c_int, c_void};
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, HostTrait as _};
use jni::objects::{JObject, JString, JValue};
use ndk::media::image_reader::{AcquireResult, Image, ImageFormat, ImageReader};
use ndk_sys as ffi;

use crate::android_rules::{
    Plane, facing_of, names, plays_loud, recognizable, size, upright, yuv420_to_bgra,
};
use crate::backend::{
    AudioDevice, CameraDevice, Device, Facing, Got, MicFeed, Picture, Source, Status,
};

#[link(name = "camera2ndk")]
unsafe extern "C" {}

/// `call` with this thread's JNI env and the app's Context, as the glue
/// handed them to `ndk-context`; the thread is attached for good the
/// first time (a device's thread lives as long as its piece).
fn java<T>(
    call: impl FnOnce(&mut jni::Env, &JObject) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let context = ndk_context::android_context();
    // SAFETY: the VM the glue handed in, there for the life of the process
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast()) };
    vm.attach_current_thread(|env| {
        // SAFETY: the glue's global reference to the application Context,
        // kept for the life of the process. `from_raw` takes it as if it
        // were a local reference, which is harmless: JNI takes a global
        // reference wherever it takes a local one, and the wrapper deletes
        // nothing when dropped (local references go with their frame), so
        // the global one is only borrowed here
        let app = unsafe { JObject::from_raw(env, context.context().cast()) };
        call(env, &app)
    })
    .map_err(|error: jni::errors::Error| error.to_string())
}

/// The app's `AudioManager`.
fn audio_manager<'local>(
    env: &mut jni::Env<'local>,
    app: &JObject,
) -> jni::errors::Result<JObject<'local>> {
    let name = env.new_string("audio")?;
    env.call_method(
        app,
        jni::jni_str!("getSystemService"),
        jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[JValue::Object(&name)],
    )?
    .l()
}

/// A call's voice I/O is coming (`on`) or went: the `AudioManager`'s mode
/// set to in-communication (the system routes and tunes as for a call) and
/// back to normal, a hint beside the app's own echo cancelling
/// (`crate::voice`).
#[cfg(feature = "call")]
pub(crate) fn call(on: bool) {
    // AudioManager.MODE_NORMAL, MODE_IN_COMMUNICATION
    let mode = if on { 3 } else { 0 };
    let set = java(|env, app| {
        let manager = audio_manager(env, app)?;
        env.call_method(
            &manager,
            jni::jni_str!("setMode"),
            jni::jni_sig!("(I)V"),
            &[JValue::Int(mode)],
        )?;
        Ok(())
    });
    if let Err(error) = set {
        tracing::warn!(target: "ducktape::av", %error, "audio mode not set");
    }
}

// ---------- the permission ----------

/// How the app asks the person for an Android permission (by its manifest
/// name, `android.permission.CAMERA`): whether it is granted, asking first
/// when it is not, blocking until the person answers.
pub type Asker = fn(&str) -> bool;

static ASKER: OnceLock<Asker> = OnceLock::new();

/// Sets how the app asks for a permission: the phone's glue, once, with
/// its Activity's own ask. Until then nothing is granted.
pub fn asks_through(asker: Asker) {
    let _ = ASKER.set(asker);
}

fn allowed(permission: &str) -> bool {
    ASKER.get().is_some_and(|ask| ask(permission))
}

/// The camera permission, asked the first time.
pub(crate) fn camera_allowed() -> bool {
    allowed("android.permission.CAMERA")
}

/// The microphone permission, asked the first time.
pub(crate) fn microphone_allowed() -> bool {
    allowed("android.permission.RECORD_AUDIO")
}

/// The app's own page in Settings, which allows both: Android has no page
/// per permission for an app. As the `package:` URI that page's intent
/// carries (the glue opens it).
pub(crate) fn settings_page(_: crate::Kind) -> Option<&'static str> {
    static PAGE: OnceLock<Option<String>> = OnceLock::new();
    PAGE.get_or_init(|| {
        java(|env, app| {
            let name = env
                .call_method(
                    app,
                    jni::jni_str!("getPackageName"),
                    jni::jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            env.cast_local::<JString>(name)?.try_to_string(env)
        })
        .map(|name| format!("package:{name}"))
        .ok()
    })
    .as_deref()
}

/// How a person allows a camera or a microphone Android refused: on the
/// app's page ([`settings_page`]).
pub(crate) fn allow(kind: crate::Kind) -> &'static str {
    match kind {
        crate::Kind::Camera => {
            "Open Settings \u{2192} Permissions \u{2192} Camera, choose Allow only while using the app, then try again."
        }
        crate::Kind::Microphone => {
            "Open Settings \u{2192} Permissions \u{2192} Microphone, choose Allow only while using the app, then try again."
        }
    }
}

// ---------- microphones and speakers ----------

/// A stream here is opened on its device and ends with it: an invalidated
/// one is gone.
pub(crate) const ROUTED_IN_PLACE: bool = false;

/// The microphones (`input`) or the speakers a person picks among those
/// Android lists now ([`recognizable`]), each with the cpal device it is.
fn listed(host: &cpal::Host, input: bool) -> Result<Vec<(AudioDevice, cpal::Device)>, String> {
    let devices = match input {
        true => host.input_devices(),
        false => host.output_devices(),
    }
    .map_err(|error| error.to_string())?;
    let mut found: Vec<cpal::Device> = Vec::new();
    let mut said = Vec::new();
    for device in devices {
        let (Ok(id), Ok(description)) = (device.id(), device.description()) else {
            continue;
        };
        said.push((id.to_string(), description));
        found.push(device);
    }
    Ok(recognizable(&said, input)
        .into_iter()
        .map(|(row, at)| (row, found[at].clone()))
        .collect())
}

/// The microphones and the speakers a person picks among those Android
/// lists now.
pub(crate) fn audio_devices() -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), String> {
    let host = cpal::default_host();
    let rows =
        |input| listed(&host, input).map(|listed| listed.into_iter().map(|(row, _)| row).collect());
    Ok((rows(true)?, rows(false)?))
}

/// The microphone (`input`) or speaker a piece names: `None` Android's
/// default, else only one [`listed`], its id compared whole.
pub(crate) fn find(host: &cpal::Host, id: Option<&str>, input: bool) -> Option<cpal::Device> {
    match id {
        Some(id) => listed(host, input)
            .ok()?
            .into_iter()
            .find(|(row, _)| row.device.id == id)
            .map(|(_, found)| found),
        None if input => host.default_input_device(),
        None => host.default_output_device(),
    }
}

/// The name a person knows the opened microphone `input` by: its row's,
/// or the default row's for Android's default (cpal's "Default Device",
/// which Android lists as no device of its own).
pub(crate) fn input_name(input: &cpal::Device) -> String {
    let id = input.id().map(|id| id.to_string()).ok();
    let listed = listed(&cpal::default_host(), true).unwrap_or_default();
    let named = |row: &&(AudioDevice, cpal::Device)| Some(&row.0.device.id) == id.as_ref();
    listed
        .iter()
        .find(named)
        .or_else(|| listed.iter().find(|(row, _)| row.device.default))
        .map(|(row, _)| row.device.name.clone())
        .unwrap_or_default()
}

/// The microphone Android records from by default now, as listed.
pub(crate) fn heard() -> Option<Device> {
    listed(&cpal::default_host(), true)
        .ok()?
        .into_iter()
        .map(|(row, _)| row.device)
        .find(|device| device.default)
}

/// An open microphone's watcher ([`activate`]), stopped and joined when
/// this drops; nothing for the chime.
pub(crate) struct Active(Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>);

/// With a microphone's feed, its watcher until the returned hold drops:
/// Android holds no session to activate.
pub(crate) fn activate(
    microphone: Option<(&Arc<MicFeed>, Option<&str>)>,
) -> Result<Active, Status> {
    Ok(Active(microphone.map(|(feed, _)| watch(feed.clone()))))
}

impl Drop for Active {
    fn drop(&mut self) {
        if let Some((stop, watcher)) = self.0.take() {
            drop(stop);
            let _ = watcher.join();
        }
    }
}

/// How often an open microphone's watcher looks.
const LOOK: Duration = Duration::from_millis(500);

/// Looks every [`LOOK`] at what Android tells only when asked: a
/// recording it silenced ends the microphone (its streams closed before
/// it is told so: [`MicFeed::end`]), and a monitor whose sound fell onto
/// the loudspeaker from elsewhere (a headset pulled out, the loudspeaker
/// picked) is turned off ([`MicFeed::silence`]). One turned on there is
/// its view's call.
fn watch(feed: Arc<MicFeed>) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (stop, stopped) = mpsc::channel::<()>();
    let watcher = std::thread::spawn(move || {
        // whether the monitor's sound was on the loudspeaker when last
        // looked (`None`: the monitor off)
        let mut loud = None;
        while stopped.recv_timeout(LOOK) == Err(mpsc::RecvTimeoutError::Timeout) {
            if silenced() {
                return feed.end(Status::Gone(crate::MICROPHONE_TAKEN.into()));
            }
            if !feed.monitoring() {
                loud = None;
                continue;
            }
            // ponytail: the speakers are listed again each look while the
            // monitor plays (a JNI call); Android's AudioDeviceCallback is
            // the upgrade if that ever shows
            let output = feed.output.lock().expect("feed output").clone();
            let now = listed(&cpal::default_host(), false).is_ok_and(|listed| {
                let outputs: Vec<_> = listed.into_iter().map(|(row, _)| row).collect();
                plays_loud(output.as_deref(), &outputs)
            });
            if now && loud == Some(false) {
                feed.silence();
            }
            loud = Some(now);
        }
    });
    (stop, watcher)
}

/// Whether Android silenced this app's recording (Android 10 on): a call
/// took the microphone, or another app it puts first, or this app left the
/// screen. Android tells an app only of its own recordings.
fn silenced() -> bool {
    if sdk().is_none_or(|sdk| sdk < 29) {
        return false;
    }
    java(|env, app| {
        let audio = audio_manager(env, app)?;
        let active = env
            .call_method(
                &audio,
                jni::jni_str!("getActiveRecordingConfigurations"),
                jni::jni_sig!("()Ljava/util/List;"),
                &[],
            )?
            .l()?;
        let count = env
            .call_method(&active, jni::jni_str!("size"), jni::jni_sig!("()I"), &[])?
            .i()?;
        for at in 0..count {
            let recording = env
                .call_method(
                    &active,
                    jni::jni_str!("get"),
                    jni::jni_sig!("(I)Ljava/lang/Object;"),
                    &[JValue::Int(at)],
                )?
                .l()?;
            let silenced = env
                .call_method(
                    &recording,
                    jni::jni_str!("isClientSilenced"),
                    jni::jni_sig!("()Z"),
                    &[],
                )?
                .z()?;
            if silenced {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .unwrap_or(false)
}

/// `Build.VERSION.SDK_INT`, kept once read; `None` while it cannot be.
fn sdk() -> Option<i32> {
    static SDK: OnceLock<i32> = OnceLock::new();
    if let Some(sdk) = SDK.get() {
        return Some(*sdk);
    }
    let read = java(|env, _| {
        env.get_static_field(
            jni::jni_str!("android/os/Build$VERSION"),
            jni::jni_str!("SDK_INT"),
            jni::jni_sig!("I"),
        )?
        .i()
    })
    .ok()?;
    Some(*SDK.get_or_init(|| read))
}

// ---------- the cameras ----------

/// A Camera2 status, as an error a person reads.
fn camera_error(status: ffi::camera_status_t) -> Status {
    match status {
        ffi::camera_status_t::ACAMERA_ERROR_PERMISSION_DENIED => Status::Denied,
        ffi::camera_status_t::ACAMERA_ERROR_CAMERA_IN_USE
        | ffi::camera_status_t::ACAMERA_ERROR_MAX_CAMERA_IN_USE => Status::Busy,
        ffi::camera_status_t::ACAMERA_ERROR_CAMERA_DISCONNECTED => Status::NoDevice,
        ffi::camera_status_t::ACAMERA_ERROR_CAMERA_DISABLED => {
            Status::Failed("A device policy turned the camera off.".into())
        }
        other => {
            tracing::warn!(target: "ducktape::av", status = other.0, "the camera refused to start");
            Status::Failed("The camera refused to start.".into())
        }
    }
}

fn checked(status: ffi::camera_status_t) -> Result<(), Status> {
    match status {
        ffi::camera_status_t::ACAMERA_OK => Ok(()),
        other => Err(camera_error(other)),
    }
}

/// The system's camera manager, deleted when dropped.
struct Manager(*mut ffi::ACameraManager);

impl Manager {
    fn new() -> Result<Self, String> {
        // SAFETY: the NDK's constructor; null only when it has no camera service
        let manager = unsafe { ffi::ACameraManager_create() };
        match manager.is_null() {
            true => Err("Android has no camera service".into()),
            false => Ok(Self(manager)),
        }
    }

    /// The cameras' ids, as the system lists them.
    fn ids(&self) -> Result<Vec<String>, String> {
        let mut list = ptr::null_mut();
        // SAFETY: a live manager; the list is ours until deleted below
        let status = unsafe { ffi::ACameraManager_getCameraIdList(self.0, &mut list) };
        checked(status).map_err(|_| format!("the cameras did not list (Camera2 {})", status.0))?;
        // SAFETY: the list the call above made, `numCameras` C strings long
        let ids = unsafe {
            let count = usize::try_from((*list).numCameras).unwrap_or(0);
            let ids = (0..count)
                .map(|at| {
                    CStr::from_ptr(*(*list).cameraIds.add(at))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            ffi::ACameraManager_deleteCameraIdList(list);
            ids
        };
        Ok(ids)
    }

    /// What the system says of the camera `id`.
    fn characteristics(&self, id: &CStr) -> Option<Characteristics> {
        let mut metadata = ptr::null_mut();
        // SAFETY: a live manager and a C string id; the metadata is ours
        // until freed below
        let status = unsafe {
            ffi::ACameraManager_getCameraCharacteristics(self.0, id.as_ptr(), &mut metadata)
        };
        checked(status).ok()?;
        let entry = |tag: ffi::acamera_metadata_tag| {
            // SAFETY: a zeroed entry is a valid out parameter
            let mut entry: ffi::ACameraMetadata_const_entry = unsafe { std::mem::zeroed() };
            // SAFETY: live metadata; the entry points into it until freed
            let found = unsafe { ffi::ACameraMetadata_getConstEntry(metadata, tag.0, &mut entry) };
            (found == ffi::camera_status_t::ACAMERA_OK).then_some(entry)
        };
        // SAFETY: each tag's own type (u8 facing, i32 orientation and
        // stream configurations), `count` long, read before the free
        let characteristics = unsafe {
            let facing = entry(ffi::acamera_metadata_tag::ACAMERA_LENS_FACING)
                .filter(|entry| entry.count > 0)
                .map(|entry| *entry.data.u8_);
            let sensor = entry(ffi::acamera_metadata_tag::ACAMERA_SENSOR_ORIENTATION)
                .filter(|entry| entry.count > 0)
                .map_or(0, |entry| *entry.data.i32_);
            let configurations =
                entry(ffi::acamera_metadata_tag::ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS)
                    .map(|entry| {
                        std::slice::from_raw_parts(entry.data.i32_, entry.count as usize).to_vec()
                    })
                    .unwrap_or_default();
            ffi::ACameraMetadata_free(metadata);
            Characteristics {
                facing: facing_of(facing),
                sensor: sensor.rem_euclid(360) as u32,
                size: size(&configurations),
            }
        };
        Some(characteristics)
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // SAFETY: made in `new`, deleted once
        unsafe { ffi::ACameraManager_delete(self.0) };
    }
}

/// What opening a camera needs to know of it.
struct Characteristics {
    facing: Facing,
    /// The clockwise turn that stands its pictures upright on the phone
    /// held as it is made to be held.
    sensor: u32,
    /// The picture size asked ([`size`]).
    size: Option<(i32, i32)>,
}

/// The cameras the system lists now, the front one first (a call's
/// camera), then the back ones, then any plugged in; the first is the
/// default. Listing asks for no permission.
pub(crate) fn cameras() -> Result<Vec<CameraDevice>, String> {
    let manager = Manager::new()?;
    let mut found: Vec<(String, Facing)> = manager
        .ids()?
        .into_iter()
        .filter_map(|id| {
            let facing = manager
                .characteristics(&CString::new(id.clone()).ok()?)?
                .facing;
            Some((id, facing))
        })
        .collect();
    let order = |facing: Facing| match facing {
        Facing::Front => 0,
        Facing::Back => 1,
        Facing::External => 2,
    };
    found.sort_by_key(|(_, facing)| order(*facing));
    let names = names(&found.iter().map(|(_, facing)| *facing).collect::<Vec<_>>());
    Ok(found
        .into_iter()
        .zip(names)
        .enumerate()
        .map(|(at, ((id, facing), name))| CameraDevice {
            device: Device {
                id: id.clone(),
                name,
                default: at == 0,
            },
            facing,
            handle: id,
        })
        .collect())
}

/// What the camera's callbacks and its reader's listener tell the stream
/// loop: a frame came, or the camera went.
type Events = mpsc::Sender<Result<(), Status>>;

unsafe extern "C" fn disconnected(context: *mut c_void, _: *mut ffi::ACameraDevice) {
    // SAFETY: the `Events` box `open` handed in, alive until the device closed
    let tell = unsafe { &*context.cast::<Events>() };
    let _ = tell.send(Err(Status::Gone(
        "Another app or the system took the camera.".into(),
    )));
}

unsafe extern "C" fn failed(context: *mut c_void, _: *mut ffi::ACameraDevice, error: c_int) {
    // SAFETY: as in `disconnected`
    let tell = unsafe { &*context.cast::<Events>() };
    let status = match u32::try_from(error).unwrap_or(0) {
        ffi::ERROR_CAMERA_IN_USE | ffi::ERROR_MAX_CAMERAS_IN_USE => Status::Busy,
        // open already: the system turned it off under the piece (a policy
        // refuses it only at open, `camera_error`)
        ffi::ERROR_CAMERA_DISABLED => Status::Gone("The system took the camera.".into()),
        _ => {
            tracing::warn!(target: "ducktape::av", error, "the camera stopped");
            Status::Gone("The camera stopped.".into())
        }
    };
    let _ = tell.send(Err(status));
}

unsafe extern "C" fn session_state(_: *mut c_void, _: *mut ffi::ACameraCaptureSession) {}

/// One camera running: the device, its session repeating a preview into
/// the reader, and everything that feeds the stream loop. Dropped, the
/// session and the device close (the light off), then the rest is freed.
pub(crate) struct Camera {
    window: ndk::native_window::NativeWindow,
    reader: ImageReader,
    device: *mut ffi::ACameraDevice,
    container: *mut ffi::ACaptureSessionOutputContainer,
    output: *mut ffi::ACaptureSessionOutput,
    session: *mut ffi::ACameraCaptureSession,
    target: *mut ffi::ACameraOutputTarget,
    request: *mut ffi::ACaptureRequest,
    /// The device callbacks' context, freed once the device closed.
    tell: *mut Events,
    events: mpsc::Receiver<Result<(), Status>>,
    front: bool,
    sensor: u32,
    /// The display's turn, and when it was read: read again each second,
    /// so a phone turned on its side gets its pictures turned too.
    display: (u32, Instant),
    _manager: Manager,
}

/// Opens the listed camera whose id is `id` (its `CameraDevice::handle`)
/// and starts its preview. The permission is asked before
/// ([`camera_allowed`]).
pub(crate) fn open(id: &str) -> Result<Camera, Status> {
    let manager = Manager::new().map_err(Status::Failed)?;
    let id = CString::new(id).map_err(|_| Status::NoDevice)?;
    let characteristics = manager.characteristics(&id).ok_or(Status::NoDevice)?;
    let (width, height) = characteristics
        .size
        .ok_or_else(|| Status::Failed("The camera offers no YUV pictures.".into()))?;
    let (tell, events) = mpsc::channel();
    let fail = |error: ndk::media_error::MediaError| Status::Failed(error.to_string());
    let mut reader = ImageReader::new(width, height, ImageFormat::YUV_420_888, 4).map_err(fail)?;
    let came = tell.clone();
    reader
        .set_image_listener(Box::new(move |_| {
            let _ = came.send(Ok(()));
        }))
        .map_err(fail)?;
    let window = reader.window().map_err(fail)?;
    let mut camera = Camera {
        window,
        reader,
        device: ptr::null_mut(),
        container: ptr::null_mut(),
        output: ptr::null_mut(),
        session: ptr::null_mut(),
        target: ptr::null_mut(),
        request: ptr::null_mut(),
        tell: Box::into_raw(Box::new(tell)),
        events,
        front: characteristics.facing == Facing::Front,
        sensor: characteristics.sensor,
        display: (display_turn(), Instant::now()),
        _manager: manager,
    };
    // each step's result lands in `camera`, whose drop undoes what was done
    let window = camera
        .window
        .ptr()
        .as_ptr()
        .cast::<ffi::ACameraWindowType>();
    let mut device_callbacks = ffi::ACameraDevice_StateCallbacks {
        context: camera.tell.cast(),
        onDisconnected: Some(disconnected),
        onError: Some(failed),
    };
    let session_callbacks = ffi::ACameraCaptureSession_stateCallbacks {
        context: ptr::null_mut(),
        onClosed: Some(session_state),
        onReady: Some(session_state),
        onActive: Some(session_state),
    };
    // SAFETY: Camera2's documented sequence on live objects, each made
    // into a field of `camera` before the next uses it; the callbacks'
    // context outlives the device (freed in drop, after it closed)
    unsafe {
        checked(ffi::ACameraManager_openCamera(
            camera._manager.0,
            id.as_ptr(),
            &mut device_callbacks,
            &mut camera.device,
        ))?;
        checked(ffi::ACaptureSessionOutputContainer_create(
            &mut camera.container,
        ))?;
        checked(ffi::ACaptureSessionOutput_create(
            window,
            &mut camera.output,
        ))?;
        checked(ffi::ACaptureSessionOutputContainer_add(
            camera.container,
            camera.output,
        ))?;
        checked(ffi::ACameraDevice_createCaptureSession(
            camera.device,
            camera.container,
            &session_callbacks,
            &mut camera.session,
        ))?;
        checked(ffi::ACameraDevice_createCaptureRequest(
            camera.device,
            ffi::ACameraDevice_request_template::TEMPLATE_PREVIEW,
            &mut camera.request,
        ))?;
        checked(ffi::ACameraOutputTarget_create(window, &mut camera.target))?;
        checked(ffi::ACaptureRequest_addTarget(
            camera.request,
            camera.target,
        ))?;
        checked(ffi::ACameraCaptureSession_setRepeatingRequest(
            camera.session,
            ptr::null_mut(),
            1,
            &mut camera.request,
            ptr::null_mut(),
        ))?;
    }
    Ok(camera)
}

impl Source for Camera {
    fn next(&mut self, wait: Duration, wanted: bool, mirror: bool) -> Result<Got, Status> {
        match self.events.recv_timeout(wait) {
            Ok(event) => event?,
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(Got::Nothing),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Status::Gone(String::new())),
        }
        // the frames that came meanwhile: the newest is taken below; the
        // camera's going among them ends the stream
        while let Ok(event) = self.events.try_recv() {
            event?;
        }
        let image = match self.reader.acquire_latest_image() {
            Ok(AcquireResult::Image(image)) => image,
            Ok(_) => return Ok(Got::Nothing),
            Err(error) => return Err(Status::Failed(error.to_string())),
        };
        if !wanted {
            return Ok(Got::Frame);
        }
        if self.display.1.elapsed() > Duration::from_secs(1) {
            self.display = (display_turn(), Instant::now());
        }
        let turn = upright(self.sensor, self.display.0, self.front);
        Ok(picture(&image, turn, mirror).map_or(Got::Frame, Got::Picture))
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: the calls that undo `open`'s, in reverse; each pointer is
        // null or made there, freed once. The device closes synchronously:
        // no callback comes after, so its context is freed then.
        unsafe {
            if !self.session.is_null() {
                ffi::ACameraCaptureSession_stopRepeating(self.session);
                ffi::ACameraCaptureSession_close(self.session);
            }
            if !self.device.is_null() {
                ffi::ACameraDevice_close(self.device);
            }
            if !self.request.is_null() {
                ffi::ACaptureRequest_free(self.request);
            }
            if !self.target.is_null() {
                ffi::ACameraOutputTarget_free(self.target);
            }
            if !self.container.is_null() {
                ffi::ACaptureSessionOutputContainer_free(self.container);
            }
            if !self.output.is_null() {
                ffi::ACaptureSessionOutput_free(self.output);
            }
            drop(Box::from_raw(self.tell));
        }
        // then the fields: the window, the reader (its listener), the manager
    }
}

/// The display's turn from the phone's natural way up, in degrees
/// (`Display.getRotation()`); 0 where it cannot be read.
fn display_turn() -> u32 {
    java(|env, app| {
        let name = env.new_string("display")?;
        let displays = env
            .call_method(
                app,
                jni::jni_str!("getSystemService"),
                jni::jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                &[JValue::Object(&name)],
            )?
            .l()?;
        let display = env
            .call_method(
                &displays,
                jni::jni_str!("getDisplay"),
                jni::jni_sig!("(I)Landroid/view/Display;"),
                &[JValue::Int(0)],
            )?
            .l()?;
        env.call_method(
            &display,
            jni::jni_str!("getRotation"),
            jni::jni_sig!("()I"),
            &[],
        )?
        .i()
    })
    .map_or(0, |rotation| (rotation.rem_euclid(4) * 90) as u32)
}

/// A camera image's picture, turned `turn` degrees clockwise and then
/// mirrored if asked; none for an image whose planes do not read.
fn picture(image: &Image, turn: u32, mirror: bool) -> Option<Picture> {
    let plane = |at| -> Option<Plane<'_>> {
        Some(Plane {
            data: image.plane_data(at).ok()?,
            row: usize::try_from(image.plane_row_stride(at).ok()?).ok()?,
            pixel: usize::try_from(image.plane_pixel_stride(at).ok()?).ok()?,
        })
    };
    let width = usize::try_from(image.width().ok()?).ok()?;
    let height = usize::try_from(image.height().ok()?).ok()?;
    yuv420_to_bgra(
        [plane(0)?, plane(1)?, plane(2)?],
        width,
        height,
        turn,
        mirror,
    )
}
