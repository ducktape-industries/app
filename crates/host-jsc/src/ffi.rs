//! WebKit and UIKit through objc2: the one hidden web view, its page's
//! delegate, and the call into the page. Everything here runs on the main
//! thread: each type is `MainThreadOnly`, and every entry takes a
//! `MainThreadMarker` or is a method WebKit calls there.
//!
//! What crosses the boundary:
//! - `WKWebView` (declared here: objc2-web-kit declares it for macOS only,
//!   where it is an `NSView`; on iOS it is a `UIView`): made once
//!   (`initWithFrame:configuration:`), loaded (`loadRequest:`), told its
//!   delegate (`setNavigationDelegate:`), added to a window (`addSubview:`),
//!   and called (`callAsyncJavaScript:arguments:inFrame:inContentWorld:
//!   completionHandler:`, whose block WebKit calls once, on the main
//!   thread, with a result object and an error it owns for the call).
//! - [`PageDelegate`], our `NSObject` subclass: WebKit's navigation
//!   delegate (the load finished or failed; `webViewWebContentProcessDid
//!   Terminate:`) and the `dt` scheme's handler (`webView:startURLScheme
//!   Task:` answers from the runner's bytes; `stopURLSchemeTask:` needs
//!   nothing). WebKit holds it weakly: the page keeps it.
//! - With the `gate` feature, `_webProcessIdentifier` (a private key, read
//!   by key-value coding) for the simulator gate's killer.
//!
//! Every object is owned by a `Retained` (objc2's strong reference); the
//! block's arguments are borrowed for the call. No `catch_unwind`: the
//! phone builds with `panic = "abort"`, and nothing here panics.
#![allow(
    unsafe_code,
    reason = "WebKit and UIKit calls; objc2-web-kit has no iOS WKWebView"
)]

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, extern_class, extern_methods,
    msg_send,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSString, NSURL, NSURLRequest,
};
use objc2_ui_kit::{UIApplication, UIResponder, UIView, UIWindowScene};
use objc2_web_kit::{
    WKContentWorld, WKFrameInfo, WKNavigationDelegate, WKURLSchemeHandler, WKURLSchemeTask,
    WKWebViewConfiguration,
};
use std::cell::Cell;

extern_class!(
    /// WebKit's web view, as iOS has it.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WKWebView"]
    pub(crate) struct WebView;
);

impl WebView {
    extern_methods!(
        #[unsafe(method(initWithFrame:configuration:))]
        #[unsafe(method_family = init)]
        fn init_with_frame_configuration(
            this: Allocated<Self>,
            frame: CGRect,
            configuration: &WKWebViewConfiguration,
        ) -> Retained<Self>;

        #[unsafe(method(loadRequest:))]
        #[unsafe(method_family = none)]
        fn load_request(&self, request: &NSURLRequest) -> Option<Retained<AnyObject>>;

        #[unsafe(method(setNavigationDelegate:))]
        #[unsafe(method_family = none)]
        fn set_navigation_delegate(
            &self,
            delegate: Option<&ProtocolObject<dyn WKNavigationDelegate>>,
        );

        #[unsafe(method(callAsyncJavaScript:arguments:inFrame:inContentWorld:completionHandler:))]
        #[unsafe(method_family = none)]
        fn call_async_javascript(
            &self,
            body: &NSString,
            arguments: Option<&NSDictionary<NSString, AnyObject>>,
            frame: Option<&WKFrameInfo>,
            world: &WKContentWorld,
            completion: Option<&block2::DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>>,
        );
    );
}

define_class!(
    /// The page's navigation delegate and its `dt` scheme's handler: each
    /// callback goes to the runner (`runner.rs`), on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DucktapeViewPage"]
    pub(crate) struct PageDelegate;

    unsafe impl NSObjectProtocol for PageDelegate {}

    unsafe impl WKNavigationDelegate for PageDelegate {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, _web: &AnyObject, _navigation: Option<&AnyObject>) {
            crate::runner::page_loaded(self.mtm());
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web: &AnyObject, _navigation: Option<&AnyObject>, error: &NSError) {
            crate::runner::page_failed(error.localizedDescription().to_string());
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            _web: &AnyObject,
            _navigation: Option<&AnyObject>,
            error: &NSError,
        ) {
            crate::runner::page_failed(error.localizedDescription().to_string());
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn terminated(&self, _web: &AnyObject) {
            crate::runner::page_died(self.mtm());
        }
    }

    unsafe impl WKURLSchemeHandler for PageDelegate {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web: &AnyObject, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            serve(task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web: &AnyObject, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
    }
);

/// The page's web view and the delegate it reports to.
pub(crate) struct Web {
    view: Retained<WebView>,
    _delegate: Retained<PageDelegate>,
}

/// Where the page is loaded from: the `dt` scheme's own handler.
const PAGE: &str = "dt://app/index.html";

impl Web {
    /// The hidden 1x1 web view, its `dt` scheme served by [`PageDelegate`],
    /// loading the page.
    pub(crate) fn new(mtm: MainThreadMarker) -> Self {
        let delegate: Retained<PageDelegate> =
            // SAFETY: `NSObject`'s `init` on a class of ours with no ivars
            unsafe { msg_send![PageDelegate::alloc(mtm), init] };
        // SAFETY: a plain configuration, made on the main thread
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        // SAFETY: `dt` is no scheme WebKit handles itself; the handler is
        // kept alive by `Web` for as long as the view
        unsafe {
            configuration.setURLSchemeHandler_forURLScheme(
                Some(ProtocolObject::from_ref(&*delegate)),
                &NSString::from_str("dt"),
            );
        }
        let frame = CGRect::new(CGPoint::new(0., 0.), CGSize::new(1., 1.));
        let view =
            WebView::init_with_frame_configuration(WebView::alloc(mtm), frame, &configuration);
        view.set_navigation_delegate(Some(ProtocolObject::from_ref(&*delegate)));
        let web = Self {
            view,
            _delegate: delegate,
        };
        web.load();
        web
    }

    /// Loads the page again: at start, and on a new content process after
    /// the last one died.
    pub(crate) fn load(&self) {
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(PAGE)) {
            let _navigation = self.view.load_request(&NSURLRequest::requestWithURL(&url));
        }
    }

    /// Puts the view in the app's window, once it has one (the page runs
    /// detached too, but starts slower: SPEC §6.1).
    pub(crate) fn attach(&self, mtm: MainThreadMarker) {
        if self.view.window().is_some() {
            return;
        }
        let scenes = UIApplication::sharedApplication(mtm).connectedScenes();
        let window = scenes.iter().find_map(|scene| {
            let scene = scene.downcast::<UIWindowScene>().ok()?;
            scene.keyWindow().or_else(|| scene.windows().firstObject())
        });
        if let Some(window) = window {
            window.addSubview(&self.view);
        }
    }

    /// `dt.call(a)` in the page, `done` given its answer: the string the
    /// page returned, or WebKit's words for why there is none.
    pub(crate) fn call(&self, a: &str, done: impl FnOnce(Result<String, String>) + 'static) {
        let done = Cell::new(Some(done));
        let block = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            let Some(done) = done.take() else {
                return;
            };
            // SAFETY: WebKit passes an error it owns for the call, or null
            let answered = match unsafe { error.as_ref() } {
                Some(error) => Err(error.localizedDescription().to_string()),
                // SAFETY: the result object is WebKit's for the call, or null
                None => match unsafe { result.as_ref() }
                    .and_then(|result| result.downcast_ref::<NSString>())
                {
                    Some(text) => Ok(text.to_string()),
                    None => Err("the page answered no string".into()),
                },
            };
            done(answered);
        });
        let key = NSString::from_str("a");
        let value = NSString::from_str(a);
        let arguments =
            NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[value.as_ref()]);
        // SAFETY: the page world, on the main thread
        let world = unsafe { WKContentWorld::pageWorld(self.view.mtm()) };
        self.view.call_async_javascript(
            &NSString::from_str("return await dt.call(a)"),
            Some(&arguments),
            None,
            &world,
            Some(&block),
        );
    }

    /// The content process's pid, for the gate's killer.
    #[cfg(feature = "gate")]
    pub(crate) fn content_process(&self) -> Option<i32> {
        // SAFETY: key-value coding of a key WebKit's view answers with an
        // `NSNumber` (or nil before the process starts)
        let pid: Option<Retained<AnyObject>> = unsafe {
            msg_send![&*self.view, valueForKey: &*NSString::from_str("_webProcessIdentifier")]
        };
        // SAFETY: `intValue` on the `NSNumber` it answered
        pid.map(|pid| unsafe { msg_send![&*pid, intValue] })
    }
}

/// Answers one `dt://app/<name>` request from the runner's bytes, or fails
/// it (the page's `fetch` then rejects).
fn serve(task: &ProtocolObject<dyn WKURLSchemeTask>) {
    // SAFETY: the task's request, while WebKit runs the task
    let request = unsafe { task.request() };
    let Some(url) = request.URL() else {
        return;
    };
    let name = url.path().map(|path| path.to_string()).unwrap_or_default();
    let Some((bytes, mime)) = crate::runner::served(name.trim_start_matches('/')) else {
        let error = NSError::new(404, &NSString::from_str("industries.ducktape.views"));
        // SAFETY: the task has not been answered
        unsafe { task.didFailWithError(&error) };
        return;
    };
    // an HTTP response: a plain `NSURLResponse` reaches the page's `fetch`
    // as status 0, which is no `ok`
    let headers = NSDictionary::<NSString, NSString>::from_slices(
        &[&*NSString::from_str("Content-Type")],
        &[&*NSString::from_str(mime)],
    );
    let Some(response) = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
        NSHTTPURLResponse::alloc(),
        &url,
        200,
        Some(&NSString::from_str("HTTP/1.1")),
        Some(&headers),
    ) else {
        let error = NSError::new(500, &NSString::from_str("industries.ducktape.views"));
        // SAFETY: the task has not been answered
        unsafe { task.didFailWithError(&error) };
        return;
    };
    // SAFETY: the task's one response, its data and its end, in order
    unsafe {
        task.didReceiveResponse(&response);
        task.didReceiveData(&NSData::with_bytes(&bytes));
        task.didFinish();
    }
}
