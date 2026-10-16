//! AVFoundation, as macOS and iOS share it: the system's camera and
//! microphone permission, and a camera through our own capture session.
//! What differs (which cameras are listed, which way each looks, what a
//! person is told) is each system's own module (`mac.rs`, `ios.rs`).
//!
//! The permission is asked through AVFoundation. The prompt needs a usage
//! string in the app's Info.plist (macOS: an unbundled build carries one
//! in its `__info_plist` section, the app's `build.rs`; launched from a
//! terminal, the grant is the terminal's. iOS: the phone's own).
//!
//! One opened camera is a capture session: the device's input, a video
//! output that asks the system for BGRA (the system converts), and a
//! delegate of ours that gets each frame on a dispatch queue of ours and
//! sends it, as a picture while one is wanted, to the stream loop
//! ([`crate::backend::stream`]), which waits for it with a time limit.
//! The device's disconnect and the session's runtime error are sent the
//! same way, as Gone, and on iOS its interruption, with why
//! (`ios::interrupted`); a phone's pictures are turned upright
//! (`ios::upright`). Dropping a [`Camera`] stops the session (the light
//! off) and takes it apart.
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, class, define_class, msg_send};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput,
    AVCaptureDeviceWasDisconnectedNotification, AVCaptureOutput, AVCaptureSession,
    AVCaptureSessionPreset1280x720, AVCaptureSessionRuntimeErrorNotification,
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVError,
    AVMediaTypeVideo,
};
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{
    NSDictionary, NSError, NSNotification, NSNotificationCenter, NSNumber, NSObject, NSString,
};

use crate::backend::{Got, Picture, Source, Status};

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

/// `AVMediaTypeVideo` and `AVMediaTypeAudio`, by value.
const VIDEO: &str = "vide";
const AUDIO: &str = "soun";

/// Whether this app may use `media`. Never asked yet: the system's prompt
/// is raised and its answer waited for (on this thread, never the main
/// one; the answer comes on a queue of the system's).
fn allowed(media: &str) -> bool {
    let kind = NSString::from_str(media);
    // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized
    // SAFETY: AVCaptureDevice's class method, an NSString media type in,
    // an NSInteger out, callable on any thread
    let status: isize =
        unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*kind] };
    match status {
        3 => true,
        0 => {
            let (tell, told) = std::sync::mpsc::channel();
            let answer = RcBlock::new(move |granted: Bool| {
                let _ = tell.send(granted.as_bool());
            });
            // SAFETY: AVCaptureDevice's class method, the media type and a
            // `void (^)(BOOL)` block in; the system copies the block and
            // calls it once, on a queue of its own, after this returns
            unsafe {
                let _: () = msg_send![
                    class!(AVCaptureDevice),
                    requestAccessForMediaType: &*kind,
                    completionHandler: &*answer
                ];
            }
            told.recv().unwrap_or(false)
        }
        _ => false,
    }
}

/// The system's microphone permission, asked the first time.
pub(crate) fn microphone_allowed() -> bool {
    allowed(AUDIO)
}

/// The system's camera permission, asked the first time.
pub(crate) fn camera_allowed() -> bool {
    allowed(VIDEO)
}

// ---------- the cameras ----------

/// AVFoundation's video media type.
pub(crate) fn video() -> Result<&'static NSString, String> {
    // SAFETY: an AVFoundation constant, there since macOS 10.7 and iOS 4
    unsafe { AVMediaTypeVideo }.ok_or_else(|| "AVFoundation has no video media type".into())
}

/// What the stream loop wants of the next frames: set before each wait,
/// read on the system's queue.
#[derive(Default)]
struct Want {
    picture: AtomicBool,
    mirror: AtomicBool,
}

/// What the delegate holds: where frames go and what is wanted of them.
pub(crate) struct Frames {
    tell: mpsc::Sender<Result<Got, Status>>,
    want: Arc<Want>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Delegate`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "DucktapeCameraFrames"]
    #[ivars = Frames]
    pub(crate) struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for Delegate {
        /// One frame, on our dispatch queue: a picture while one is
        /// wanted, else only word that a frame came.
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn frame(&self, _: &AVCaptureOutput, buffer: &CMSampleBuffer, _: &AVCaptureConnection) {
            let Frames { tell, want } = self.ivars();
            let picture = match want.picture.load(Ordering::Relaxed) {
                // SAFETY: the buffer the system lends for this call
                true => unsafe { buffer.image_buffer() }
                    .and_then(|image| picture(&image, want.mirror.load(Ordering::Relaxed))),
                false => None,
            };
            let _ = tell.send(Ok(picture.map_or(Got::Frame, Got::Picture)));
        }
    }
);

impl Delegate {
    fn new(frames: Frames) -> Retained<Self> {
        let this = Self::alloc().set_ivars(frames);
        // SAFETY: NSObject's init
        unsafe { msg_send![super(this), init] }
    }
}

/// A BGRA pixel buffer's picture, mirrored if asked; none for another
/// format or a buffer that does not lock.
pub(crate) fn picture(image: &CVPixelBuffer, mirror: bool) -> Option<Picture> {
    let read = CVPixelBufferLockFlags::ReadOnly;
    // SAFETY: locked for reading around the read below
    if unsafe { CVPixelBufferLockBaseAddress(image, read) } != 0 {
        return None;
    }
    let base = CVPixelBufferGetBaseAddress(image);
    let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
    let stride = CVPixelBufferGetBytesPerRow(image);
    let picture = match !base.is_null()
        && height > 0
        && CVPixelBufferGetPixelFormatType(image) == kCVPixelFormatType_32BGRA
    {
        // SAFETY: a locked buffer's rows, `stride` bytes apart
        true => bgra_rows(
            unsafe { std::slice::from_raw_parts(base as *const u8, stride * height) },
            width,
            height,
            stride,
            mirror,
        ),
        false => None,
    };
    // SAFETY: locked above, with the same flags
    unsafe { CVPixelBufferUnlockBaseAddress(image, read) };
    picture
}

/// BGRA rows `stride` bytes apart, as one picture, mirrored if asked.
pub(crate) fn bgra_rows(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    mirror: bool,
) -> Option<Picture> {
    let row = width * 4;
    if width == 0 || height == 0 || stride < row || src.len() < stride * (height - 1) + row {
        return None;
    }
    let mut bgra = Vec::with_capacity(row * height);
    for line in src.chunks(stride).take(height) {
        match mirror {
            true => line[..row]
                .chunks_exact(4)
                .rev()
                .for_each(|pixel| bgra.extend_from_slice(pixel)),
            false => bgra.extend_from_slice(&line[..row]),
        }
    }
    Some(Picture {
        width: width as u32,
        height: height as u32,
        bgra,
    })
}

/// The video output: BGRA (the system converts from the camera's own
/// format), late frames dropped, each frame to `delegate` on `queue`.
fn output(delegate: &Delegate, queue: &DispatchQueue) -> Retained<AVCaptureVideoDataOutput> {
    // SAFETY: a CFString is an NSString (toll-free bridged)
    let key: &NSString =
        unsafe { &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString) };
    let bgra = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let bgra: &AnyObject = &bgra;
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[bgra]);
    // SAFETY: a new output, set up before any session has it
    unsafe {
        let output = AVCaptureVideoDataOutput::new();
        output.setVideoSettings(Some(&settings));
        output.setAlwaysDiscardsLateVideoFrames(true);
        output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(delegate)), Some(queue));
        output
    }
}

/// What a refused open says, as a status.
fn refused(error: &NSError) -> Status {
    let code = error.code();
    if code == AVError::DeviceInUseByAnotherApplication.0
        || code == AVError::DeviceAlreadyUsedByAnotherSession.0
    {
        Status::Busy
    } else if code == AVError::ApplicationIsNotAuthorizedToUseDevice.0 {
        Status::Denied
    } else if code == AVError::DeviceWasDisconnected.0 || code == AVError::DeviceNotConnected.0 {
        Status::NoDevice
    } else {
        Status::Failed(error.localizedDescription().to_string())
    }
}

/// One camera running: its session and everything that feeds the stream
/// loop. Dropped, the session stops and is taken apart.
pub(crate) struct Camera {
    session: Retained<AVCaptureSession>,
    input: Retained<AVCaptureDeviceInput>,
    output: Retained<AVCaptureVideoDataOutput>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    frames: mpsc::Receiver<Result<Got, Status>>,
    want: Arc<Want>,
    _delegate: Retained<Delegate>,
    _queue: DispatchRetained<DispatchQueue>,
}

/// Opens the listed camera whose unique id is `id` (its
/// `CameraDevice::handle`) and starts its session. The permission is
/// asked before ([`camera_allowed`]).
pub(crate) fn open(id: &str) -> Result<Camera, Status> {
    autoreleasepool(|_| {
        // SAFETY: AVFoundation's documented calls, on this thread of ours
        // (none needs the main one)
        unsafe {
            let device = AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(id))
                .ok_or(Status::NoDevice)?;
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| refused(&e))?;
            let (tell, frames) = mpsc::channel();
            let want = Arc::new(Want::default());
            let delegate = Delegate::new(Frames {
                tell: tell.clone(),
                want: want.clone(),
            });
            let queue = DispatchQueue::new("industries.ducktape.camera", None);
            let output = output(&delegate, &queue);
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                session.setSessionPreset(AVCaptureSessionPreset1280x720);
            }
            let fits = session.canAddInput(&input) && session.canAddOutput(&output);
            if fits {
                session.addInput(&input);
                session.addOutput(&output);
                // a phone's pictures upright for its portrait screen
                #[cfg(target_os = "ios")]
                crate::ios::upright(&output);
            }
            session.commitConfiguration();
            // gone: the device unplugged, the session stopped on an error,
            // or (iOS) interrupted, `why` saying how
            let center = NSNotificationCenter::defaultCenter();
            let observe = |name, object: &AnyObject, why: fn(&NSNotification) -> String| {
                let tell = tell.clone();
                let gone = RcBlock::new(move |notification: NonNull<NSNotification>| {
                    // SAFETY: the notification the center lends for this call
                    let why = why(notification.as_ref());
                    let _ = tell.send(Err(Status::Gone(why)));
                });
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(object),
                    None,
                    &gone,
                )
            };
            let unsaid = |_: &NSNotification| String::new();
            let observers = vec![
                observe(AVCaptureDeviceWasDisconnectedNotification, &device, unsaid),
                observe(AVCaptureSessionRuntimeErrorNotification, &session, unsaid),
                // a call, another app or the background took the camera
                #[cfg(target_os = "ios")]
                observe(
                    objc2_av_foundation::AVCaptureSessionWasInterruptedNotification,
                    &session,
                    crate::ios::interrupted,
                ),
            ];
            let camera = Camera {
                session,
                input,
                output,
                observers,
                frames,
                want,
                _delegate: delegate,
                _queue: queue,
            };
            if !fits {
                return Err(Status::Failed(
                    "the camera does not fit a capture session".into(),
                ));
            }
            // blocks until the session runs (or fails: the error is sent)
            camera.session.startRunning();
            Ok(camera)
        }
    })
}

impl Source for Camera {
    fn next(&mut self, wait: Duration, wanted: bool, mirror: bool) -> Result<Got, Status> {
        self.want.picture.store(wanted, Ordering::Relaxed);
        self.want.mirror.store(mirror, Ordering::Relaxed);
        match self.frames.recv_timeout(wait) {
            Ok(got) => got,
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(Got::Nothing),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Status::Gone(String::new())),
        }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        autoreleasepool(|_| {
            // SAFETY: the calls that undo `open`'s, in reverse
            unsafe {
                let center = NSNotificationCenter::defaultCenter();
                for observer in &self.observers {
                    center.removeObserver((**observer).as_ref());
                }
                // blocks until the flow has stopped: the light off
                self.session.stopRunning();
                self.output.setSampleBufferDelegate_queue(None, None);
                self.session.beginConfiguration();
                self.session.removeOutput(&self.output);
                self.session.removeInput(&self.input);
                self.session.commitConfiguration();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::sel;
    use objc2_core_foundation::CFRetained;
    use objc2_core_video::CVPixelBufferCreate;

    /// Padded BGRA rows come out as one picture, mirrored if asked.
    #[test]
    fn bgra_rows_are_read_by_their_stride() {
        let src = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 9, 9, 9, 9, 8, 8, 8, 8];
        let plain = bgra_rows(&src, 2, 2, 10, false).unwrap();
        assert_eq!(plain.bgra, [1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, 8, 8, 8, 8]);
        let mirrored = bgra_rows(&src, 2, 2, 10, true).unwrap();
        assert_eq!(
            mirrored.bgra,
            [5, 6, 7, 8, 1, 2, 3, 4, 8, 8, 8, 8, 9, 9, 9, 9]
        );
        assert!(bgra_rows(&src[..17], 2, 2, 10, false).is_none());
        assert!(bgra_rows(&src, 2, 2, 7, false).is_none());
    }

    /// A pixel buffer the system made (its rows padded as it likes) gives
    /// its picture; the delegate answers the frame selector; an output
    /// takes our BGRA settings and our delegate. No camera is opened.
    #[test]
    fn a_system_pixel_buffer_becomes_a_picture() {
        let (width, height) = (3, 2);
        let mut raw = std::ptr::null_mut();
        let made = unsafe {
            CVPixelBufferCreate(
                None,
                width,
                height,
                kCVPixelFormatType_32BGRA,
                None,
                NonNull::from(&mut raw),
            )
        };
        assert_eq!(made, 0);
        let image = unsafe { CFRetained::from_raw(NonNull::new(raw).unwrap()) };
        let stride = CVPixelBufferGetBytesPerRow(&image);
        assert!(stride >= width * 4);
        unsafe {
            assert_eq!(
                CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::empty()),
                0
            );
            let base = CVPixelBufferGetBaseAddress(&image) as *mut u8;
            for y in 0..height {
                for x in 0..width {
                    let at = base.add(y * stride + x * 4);
                    std::ptr::copy_nonoverlapping([x as u8, y as u8, 7, 255].as_ptr(), at, 4);
                }
            }
            CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::empty());
        }
        let picture = picture(&image, true).unwrap();
        assert_eq!((picture.width, picture.height), (3, 2));
        assert_eq!(picture.bgra[..4], [2, 0, 7, 255]);
        assert_eq!(picture.bgra[12..16], [2, 1, 7, 255]);

        let (tell, _) = mpsc::channel();
        let delegate = Delegate::new(Frames {
            tell,
            want: Arc::default(),
        });
        assert!(
            delegate.respondsToSelector(sel!(captureOutput:didOutputSampleBuffer:fromConnection:))
        );
        // the output takes our settings (a key it refuses throws); it reads
        // them back only once a camera feeds it, so that is not asked here
        let queue = DispatchQueue::new("industries.ducktape.camera.test", None);
        let output = output(&delegate, &queue);
        unsafe {
            assert!(output.alwaysDiscardsLateVideoFrames());
            assert!(output.sampleBufferDelegate().is_some());
        }
    }
}
