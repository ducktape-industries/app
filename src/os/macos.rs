//! macOS raises banners through the user-notification centre, which
//! TERMINATES a process that has no bundle identifier — not an error a
//! caller can catch, the process dies. `cargo test`, `cargo run` and any
//! bare binary are exactly that process, so the identifier is read first.

use ducktape_host::runtime::notify::{Click, Notice, NoticeKind, SOUND};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSError, NSSet, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
    UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

/// The userInfo key a banner carries what a click opens under (its centre
/// row's id, or [`UPDATES`]): macOS does not hand `targetContentIdentifier`
/// back in a click's response.
const ROW: &str = "row";
/// The userInfo key a banner carries its chain under: the row is that
/// chain's, and a decline goes to that chain's centre.
const CHAIN: &str = "chain";
/// The `ROW` of the host's update notice.
const UPDATES: &str = "updates";
/// A call's category, and its two actions: Answer opens the row as a
/// click does; Decline takes the banner down, on this device alone (SPEC
/// D11).
const CALL: &str = "call";
const ANSWER: &str = "answer";
const DECLINE: &str = "decline";

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
            let (action, row, chain, tag) = unsafe {
                let content = response.notification().request().content();
                let info = content.userInfo();
                let key = NSString::from_str(ROW);
                // the row and its chain are only ever written as strings (`post`)
                let row = info
                    .objectForKey(AsRef::<AnyObject>::as_ref(&*key))
                    .map(|row| Retained::cast::<NSString>(row).to_string());
                let key = NSString::from_str(CHAIN);
                let chain = info
                    .objectForKey(AsRef::<AnyObject>::as_ref(&*key))
                    .map(|chain| Retained::cast::<NSString>(chain).to_string());
                (
                    response.actionIdentifier(),
                    row,
                    chain.unwrap_or_default(),
                    content.threadIdentifier().to_string(),
                )
            };
            // SAFETY: a framework constant.
            let default = &*action == unsafe { UNNotificationDefaultActionIdentifier };
            let click = row.and_then(|row| match row.as_str() {
                UPDATES => Some(Click::Updates),
                row => row.parse::<u64>().ok().map(|row| Click::Row {
                    chain: chain.clone(),
                    row,
                }),
            });
            let action = action.to_string();
            if action == DECLINE {
                // the centre holding its chain's log hears it: the one in
                // hand, or that network's followed set
                withdraw(&chain, &tag);
                ducktape_host::runtime::notify::declined(&chain, &tag);
            } else if let Some(click) = click.filter(|_| default || action == ANSWER) {
                super::clicked(click);
            }
            done.call(());
        }
    }
);

/// The centre holds its delegate weakly: this one is kept for the life
/// of the process. A call's category goes in with it.
fn listen(centre: &UNUserNotificationCenter) {
    static LISTENING: std::sync::Once = std::sync::Once::new();
    LISTENING.call_once(|| {
        let clicks: Retained<Clicks> =
            unsafe { msg_send_id![super(Clicks::alloc().set_ivars(())), init] };
        // SAFETY: the delegate outlives the centre's use of it (leaked);
        // the category and its actions are copied by the centre.
        unsafe {
            centre.setDelegate(Some(ProtocolObject::from_ref(&*clicks)));
            let answer = UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(ANSWER),
                &NSString::from_str("Answer"),
                UNNotificationActionOptions::UNNotificationActionOptionForeground,
            );
            let decline = UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(DECLINE),
                &NSString::from_str("Decline"),
                UNNotificationActionOptions::UNNotificationActionOptionDestructive,
            );
            let call =
                UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                    &NSString::from_str(CALL),
                    &NSArray::from_vec(vec![answer, decline]),
                    &NSArray::from_vec(Vec::<Retained<NSString>>::new()),
                    UNNotificationCategoryOptions(0),
                );
            // `NSSet::from_vec` takes only classes whose hash cannot change
            let calls: Retained<NSSet<UNNotificationCategory>> =
                msg_send_id![NSSet::<UNNotificationCategory>::class(), setWithObject: &*call];
            centre.setNotificationCategories(&calls);
        }
        std::mem::forget(clicks);
    });
}

/// The delegate and the call category in place at the host's start: the
/// centre takes categories in on its own time, so a call's first banner
/// raised right after them could show no Answer and Decline.
pub(super) fn start() {
    if bundled() {
        // SAFETY: as in `post`.
        unsafe { listen(&UNUserNotificationCenter::currentNotificationCenter()) };
    }
}

pub(super) fn post(notice: &Notice, click: Option<Click>) -> bool {
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
        // what a click opens, and on which chain
        if let Some(click) = &click {
            let (opens, chain) = match click {
                Click::Row { chain, row } => (row.to_string(), chain.as_str()),
                Click::Updates => (UPDATES.to_string(), ""),
            };
            let row = NSDictionary::<NSString, NSString>::from_vec(
                &[&*NSString::from_str(ROW), &*NSString::from_str(CHAIN)],
                vec![NSString::from_str(&opens), NSString::from_str(chain)],
            );
            // a dictionary of strings is a property list, as userInfo holds
            content.setUserInfo(&Retained::cast::<NSDictionary>(row));
        }
        // a notice quacks (the bundle's Resources, `bundle-app-macos.sh`;
        // a name macOS does not find plays its default), a call keeps the
        // default
        let sound = match notice.kind {
            NoticeKind::Call => UNNotificationSound::defaultSound(),
            NoticeKind::Notice => UNNotificationSound::soundNamed(&NSString::from_str(SOUND)),
        };
        content.setSound(Some(&sound));
        if notice.kind == NoticeKind::Call {
            content.setCategoryIdentifier(&NSString::from_str(CALL));
        }
        // A tag REPLACES: the banner standing under it goes, and this one
        // takes its place. Not by reusing its identifier: two requests
        // under one identifier added a moment apart cancel each other in
        // the centre (a burst's second "N more" took both down).
        // The host's notice (the one a click opens Updates from) stands
        // under one identifier for the install instead: any process takes
        // it down or puts the next one in its place.
        // ponytail: two host notices added a moment apart would cancel
        // each other as above; one per staged release never comes close.
        let host = matches!(click, Some(Click::Updates));
        let identifier = match host {
            true => HOST_NOTICE.to_string(),
            false => ducktape_host::runtime::notify::request_id(),
        };
        if host {
            let standing = NSArray::from_vec(vec![NSString::from_str(&identifier)]);
            centre.removePendingNotificationRequestsWithIdentifiers(&standing);
            centre.removeDeliveredNotificationsWithIdentifiers(&standing);
        } else if !notice.tag.is_empty() {
            let standing = standing()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(
                    (notice.chain.clone(), notice.tag.clone()),
                    identifier.clone(),
                );
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

/// The banner standing under `tag` on `chain`, taken down.
pub(super) fn withdraw(chain: &str, tag: &str) {
    let standing = standing()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&(chain.to_owned(), tag.to_owned()));
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

/// The host notice's request identifier: one for the install.
const HOST_NOTICE: &str = "ducktape-update";

/// The host's notice down, whichever process raised it.
pub(super) fn withdraw_host() {
    if !bundled() {
        return;
    }
    // SAFETY: as in `post`.
    unsafe {
        let centre = UNUserNotificationCenter::currentNotificationCenter();
        let standing = NSArray::from_vec(vec![NSString::from_str(HOST_NOTICE)]);
        centre.removePendingNotificationRequestsWithIdentifiers(&standing);
        centre.removeDeliveredNotificationsWithIdentifiers(&standing);
    }
}

/// The banner each chain's tag is standing under, by its request
/// identifier: two networks' programs may share a tag, and each stands.
type Standing = std::collections::HashMap<(String, String), String>;

fn standing() -> &'static std::sync::Mutex<Standing> {
    static STANDING: std::sync::OnceLock<std::sync::Mutex<Standing>> = std::sync::OnceLock::new();
    STANDING.get_or_init(Default::default)
}
