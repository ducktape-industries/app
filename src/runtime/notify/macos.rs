//! macOS raises banners through the user-notification centre, which
//! TERMINATES a process that has no bundle identifier — not an error a
//! caller can catch, the process dies. `cargo test`, `cargo run` and any
//! bare binary are exactly that process, so the identifier is read first.

use super::Notice;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

/// The userInfo key a banner carries its centre row under: macOS does
/// not hand `targetContentIdentifier` back in a click's response.
const ROW: &str = "row";

fn bundled() -> bool {
    // SAFETY: reading the main bundle's identifier is valid on any thread.
    unsafe { NSBundle::mainBundle().bundleIdentifier().is_some() }
}

declare_class!(
    /// The notification centre's delegate: a banner the host raised
    /// shows even while this app is in front (the host already decided
    /// it should), and a click on one opens the row it names.
    struct Clicks;

    unsafe impl ClassType for Clicks {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "DucktapeNoticeClicks";
    }

    impl DeclaredClass for Clicks {}

    unsafe impl NSObjectProtocol for Clicks {}

    unsafe impl UNUserNotificationCenterDelegate for Clicks {
        #[method(userNotificationCenter:willPresentNotification:withCompletionHandler:)]
        fn will_present(
            &self,
            _centre: &UNUserNotificationCenter,
            _notification: &UNNotification,
            shown: &block2::Block<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            shown.call((UNNotificationPresentationOptions::UNNotificationPresentationOptionBanner
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionList
                | UNNotificationPresentationOptions::UNNotificationPresentationOptionSound,));
        }

        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        fn did_receive(
            &self,
            _centre: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &block2::Block<dyn Fn()>,
        ) {
            // SAFETY: reads off the response the framework hands in.
            let (action, row) = unsafe {
                let info = response.notification().request().content().userInfo();
                let key = NSString::from_str(ROW);
                // the row is only ever written as a string (`post`)
                let row = info
                    .objectForKey(AsRef::<AnyObject>::as_ref(&*key))
                    .map(|row| Retained::cast::<NSString>(row).to_string());
                (response.actionIdentifier(), row)
            };
            // SAFETY: a framework constant.
            let default = &*action == unsafe { UNNotificationDefaultActionIdentifier };
            let entry = row.and_then(|row| row.parse::<u64>().ok());
            if let Some(entry) = entry.filter(|_| default) {
                super::clicked(entry);
            }
            done.call(());
        }
    }
);

/// The centre holds its delegate weakly: this one is kept for the life
/// of the process.
fn listen(centre: &UNUserNotificationCenter) {
    static LISTENING: std::sync::Once = std::sync::Once::new();
    LISTENING.call_once(|| {
        let clicks: Retained<Clicks> =
            unsafe { msg_send_id![super(Clicks::alloc().set_ivars(())), init] };
        // SAFETY: the delegate outlives the centre's use of it (leaked).
        unsafe { centre.setDelegate(Some(ProtocolObject::from_ref(&*clicks))) };
        std::mem::forget(clicks);
    });
}

pub(super) fn post(notice: &Notice, entry: Option<u64>) -> bool {
    if !bundled() {
        tracing::debug!(
            target: "ducktape::app",
            reason = "no_bundle_identifier",
            "skipped a desktop notice: this process is not an app bundle"
        );
        return false;
    }
    // SAFETY: plain framework work on objects this function owns; the
    // centre is thread-safe by contract and copies what it is handed.
    let (centre, request) = unsafe {
        let centre = UNUserNotificationCenter::currentNotificationCenter();
        listen(&centre);
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notice.title));
        content.setBody(&NSString::from_str(&notice.body));
        if !notice.tag.is_empty() {
            content.setThreadIdentifier(&NSString::from_str(&notice.tag));
        }
        // the row a click opens
        if let Some(entry) = entry {
            let row = NSDictionary::<NSString, NSString>::from_vec(
                &[&*NSString::from_str(ROW)],
                vec![NSString::from_str(&entry.to_string())],
            );
            // a dictionary of strings is a property list, as userInfo holds
            content.setUserInfo(&Retained::cast::<NSDictionary>(row));
        }
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        // A tag REPLACES: the banner standing under it goes, and this one
        // takes its place. Not by reusing its identifier: two requests
        // under one identifier added a moment apart cancel each other in
        // the centre (a burst's second "N more" took both down).
        let identifier = format!("ducktape-{}", fresh());
        if !notice.tag.is_empty() {
            let standing = standing()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(notice.tag.clone(), identifier.clone());
            if let Some(standing) = standing {
                let standing = NSArray::from_vec(vec![NSString::from_str(&standing)]);
                centre.removePendingNotificationRequestsWithIdentifiers(&standing);
                centre.removeDeliveredNotificationsWithIdentifiers(&standing);
            }
        }
        let request: Retained<UNNotificationRequest> =
            UNNotificationRequest::requestWithIdentifier_content_trigger(
                &NSString::from_str(&identifier),
                &content,
                None,
            );
        (centre, request)
    };
    // The person's word comes first: macOS asks them on the first
    // banner, and answers every later ask from what they said. Unasked,
    // the centre refuses every request.
    let (said, heard) = std::sync::mpsc::channel();
    let asked = centre.clone();
    let answered = block2::RcBlock::new(move |granted: Bool, _: *mut NSError| {
        if !granted.as_bool() {
            let _ = said.send(false);
            return;
        }
        // the answer is whether the centre took it
        let said = said.clone();
        let added = block2::RcBlock::new(move |error: *mut NSError| {
            let _ = said.send(error.is_null());
        });
        // SAFETY: as above.
        unsafe { asked.addNotificationRequest_withCompletionHandler(&request, Some(&added)) };
    });
    // SAFETY: as above; the block is copied by the framework.
    unsafe {
        centre.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::UNAuthorizationOptionAlert
                | UNAuthorizationOptions::UNAuthorizationOptionSound,
            &answered,
        );
    }
    // still asking the person: the banner goes up when they allow it
    heard
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap_or(true)
}

/// The banner standing under `tag`, taken down.
pub(super) fn withdraw(tag: &str) {
    let standing = standing()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(tag);
    let Some(standing) = standing.filter(|_| bundled()) else {
        return;
    };
    // SAFETY: as in `post`.
    unsafe {
        let centre = UNUserNotificationCenter::currentNotificationCenter();
        let standing = NSArray::from_vec(vec![NSString::from_str(&standing)]);
        centre.removePendingNotificationRequestsWithIdentifiers(&standing);
        centre.removeDeliveredNotificationsWithIdentifiers(&standing);
    }
}

/// The banner each tag is standing under, by its request identifier.
fn standing() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static STANDING: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, String>>,
    > = std::sync::OnceLock::new();
    STANDING.get_or_init(Default::default)
}

fn fresh() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
