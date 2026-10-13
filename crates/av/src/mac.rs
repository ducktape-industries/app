//! macOS: the system's camera and microphone permission (TCC), asked
//! through AVFoundation as nokhwa asks for the camera. The prompt needs a
//! usage string in the binary's Info.plist; an unbundled build carries one
//! in its `__info_plist` section (the app's `build.rs`). Launched from a
//! terminal, the grant is the terminal's.
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2::{class, msg_send};
use objc2_foundation::NSString;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

/// `AVMediaTypeVideo` and `AVMediaTypeAudio`, by value.
pub(crate) const VIDEO: &str = "vide";
pub(crate) const AUDIO: &str = "soun";

/// Whether this app may use `media`. Never asked yet: the system's prompt
/// is raised and its answer waited for (on this thread, never the main
/// one; the answer comes on a queue of the system's).
pub(crate) fn allowed(media: &str) -> bool {
    let kind = NSString::from_str(media);
    // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized
    let status: isize =
        unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*kind] };
    match status {
        3 => true,
        0 => {
            let (tell, told) = std::sync::mpsc::channel();
            let answer = RcBlock::new(move |granted: Bool| {
                let _ = tell.send(granted.as_bool());
            });
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
