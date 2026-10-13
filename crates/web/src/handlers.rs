//! Everything that names CEF's own handler types (they clash with gpui's
//! names): the app and process handlers, one client per browser, and the
//! IME and typing calls.
use crate::page::Shared;
use cef::*;
use std::os::raw::c_int;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// CEF's "no range": `CefRange::InvalidRange()`.
const NO_RANGE: Range = Range {
    from: u32::MAX,
    to: u32::MAX,
};

/// The cursor `on_cursor_change` hands over: the cef crate names one on
/// Linux and Windows, and on macOS passes the `NSCursor` as a bare pointer.
#[cfg(target_os = "macos")]
type CursorHandle = *mut u8;

pub(crate) fn app() -> App {
    WebApp::new()
}

/// CEF up in this (the app's) process, its caches under `root`.
pub(crate) fn initialize(sandbox: bool, root: &Path) -> bool {
    let args = args::Args::new();
    let settings = Settings {
        no_sandbox: !sandbox as c_int,
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        root_cache_path: root.to_string_lossy().as_ref().into(),
        log_severity: LogSeverity::WARNING,
        // a SIGTERM is the app's: CEF's own handler exits before teardown
        disable_signal_handlers: 1,
        ..Default::default()
    };
    cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app()),
        std::ptr::null_mut(),
    ) == 1
}

/// A request context with its own cookies and cache in `profile`, a
/// directory under the root cache.
pub(crate) fn context(profile: &Path) -> Option<RequestContext> {
    let _ = std::fs::create_dir_all(profile);
    let settings = RequestContextSettings {
        cache_path: profile.to_string_lossy().as_ref().into(),
        ..Default::default()
    };
    request_context_create_context(Some(&settings), None)
}

/// Asks CEF for the page's browser, in `context` (its profile); the
/// browser arrives in `on_after_created`. A fresh request context is not
/// initialized yet, which the synchronous call refuses.
pub(crate) fn create(shared: &Rc<Shared>, url: &str, context: Option<RequestContext>) -> bool {
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        ..Default::default()
    };
    let settings = BrowserSettings {
        windowless_frame_rate: 60,
        ..Default::default()
    };
    let mut client = PageClient::new(
        PageRender::new(shared.clone()),
        PageDisplay::new(shared.clone()),
        PageLife::new(shared.clone()),
        PageFocus::new(shared.clone()),
        PageLoad::new(shared.clone()),
        PageRequest::new(shared.clone()),
        crate::menu::PageMenu::new(shared.clone()),
    );
    let mut context = context;
    browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url.into()),
        Some(&settings),
        None,
        context.as_mut(),
    ) == 1
}

pub(crate) fn compose(host: &BrowserHost, text: &str, selected: std::ops::Range<usize>) {
    let length = text.encode_utf16().count() as u32;
    let underline = CompositionUnderline {
        range: Range {
            from: 0,
            to: length,
        },
        color: 0xFF00_0000,
        ..Default::default()
    };
    host.ime_set_composition(
        Some(&text.into()),
        Some(&[underline]),
        Some(&NO_RANGE),
        Some(&Range {
            from: selected.start as u32,
            to: selected.end as u32,
        }),
    );
}

pub(crate) fn commit(host: &BrowserHost, text: &str) {
    host.ime_commit_text(Some(&text.into()), Some(&NO_RANGE), 0);
}

pub(crate) fn type_text(host: &BrowserHost, text: &str) {
    for unit in text.encode_utf16() {
        let event = KeyEvent {
            type_: KeyEventType::CHAR,
            windows_key_code: unit as c_int,
            character: unit,
            unmodified_character: unit,
            ..Default::default()
        };
        host.send_key_event(Some(&event));
    }
}

/// The address `request` asks for.
fn address(request: Option<&mut Request>) -> String {
    request
        .map(|request| CefString::from(&request.url()).to_string())
        .unwrap_or_default()
}

/// A popup or new tab's address, opened in the page itself when the rule
/// allows it.
fn redirect(shared: &Shared, url: Option<&CefString>) {
    let url = url.map(CefString::to_string).unwrap_or_default();
    if crate::loadable(&url) {
        *shared.redirect.borrow_mut() = Some(url);
        shared.dirty.set(true);
    }
}

wrap_app! {
    struct WebApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(line) = command_line else { return };
            // off-screen pages paint in software: no GPU process
            for switch in ["disable-gpu", "disable-gpu-compositing", "noerrdialogs"] {
                line.append_switch(Some(&switch.into()));
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(WebProcess::new())
        }
    }
}

wrap_browser_process_handler! {
    struct WebProcess;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            crate::READY.store(true, Ordering::SeqCst);
        }

        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            crate::schedule(delay_ms);
        }
    }
}

wrap_render_handler! {
    struct PageRender {
        shared: Rc<Shared>,
    }

    impl RenderHandler {
        fn accessibility_handler(&self) -> Option<AccessibilityHandler> {
            Some(crate::a11y::handler(self.shared.clone()))
        }

        fn on_scroll_offset_changed(&self, _browser: Option<&mut Browser>, x: f64, y: f64) {
            self.shared.a11y.tree.borrow_mut().scrolled(x, y);
        }

        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (width, height) = self.shared.size.get();
                rect.width = width.max(1);
                rect.height = height.max(1);
            }
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> c_int {
            let Some(info) = info else { return 0 };
            info.device_scale_factor = self.shared.scale.get();
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: c_int) {
            if show == 0 {
                self.shared.popup_rect.set(None);
            }
            self.shared.dirty.set(true);
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            self.shared
                .popup_rect
                .set(rect.map(|r| (r.x, r.y, r.width, r.height)));
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: c_int,
            height: c_int,
        ) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let length = width as usize * height as usize * 4;
            // SAFETY: CEF passes a width x height BGRA buffer, valid for this call
            let bytes = unsafe { std::slice::from_raw_parts(buffer, length) }.to_vec();
            let Some(pixels) = image::ImageBuffer::from_raw(width as u32, height as u32, bytes) else {
                return;
            };
            let image = Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(pixels)]));
            if type_ == PaintElementType::POPUP {
                *self.shared.popup.borrow_mut() = Some(image);
            } else {
                *self.shared.frame.borrow_mut() = Some(image);
            }
            self.shared.dirty.set(true);
        }
    }
}

wrap_display_handler! {
    struct PageDisplay {
        shared: Rc<Shared>,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|frame| frame.is_main() == 1) {
                self.shared.state.borrow_mut().url = url.map(CefString::to_string).unwrap_or_default();
                self.shared.refused.borrow_mut().take();
                self.shared.dirty.set(true);
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            self.shared.state.borrow_mut().title = title.map(CefString::to_string).unwrap_or_default();
            self.shared.dirty.set(true);
        }

        // off-screen, the page's box shows it: CEF has no window to set
        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: CursorHandle,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> c_int {
            self.shared.cursor.set(crate::cursor::style(type_));
            self.shared.dirty.set(true);
            1
        }
    }
}

wrap_life_span_handler! {
    struct PageLife {
        shared: Rc<Shared>,
    }

    impl LifeSpanHandler {
        // no popup windows: the address opens in the page itself
        #[allow(clippy::too_many_arguments)]
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut c_int>,
        ) -> c_int {
            redirect(&self.shared, target_url);
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            *self.shared.browser.borrow_mut() = browser.map(|browser| browser.clone());
            self.shared.dirty.set(true);
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.shared.closed.set(true);
        }
    }
}

wrap_focus_handler! {
    struct PageFocus {
        shared: Rc<Shared>,
    }

    impl FocusHandler {
        fn on_take_focus(&self, _browser: Option<&mut Browser>, next: c_int) {
            self.shared.take_focus.set(Some(next != 0));
            self.shared.dirty.set(true);
        }
    }
}

wrap_load_handler! {
    struct PageLoad {
        shared: Rc<Shared>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: c_int,
            can_go_back: c_int,
            can_go_forward: c_int,
        ) {
            let mut state = self.shared.state.borrow_mut();
            state.loading = is_loading != 0;
            state.can_go_back = can_go_back != 0;
            state.can_go_forward = can_go_forward != 0;
            self.shared.dirty.set(true);
        }
    }
}

wrap_request_handler! {
    struct PageRequest {
        shared: Rc<Shared>,
    }

    impl RequestHandler {
        // every navigation of the page itself (a link, a script, a
        // redirect) is held to the rule; frames inside it are the page's
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: c_int,
            _is_redirect: c_int,
        ) -> c_int {
            if frame.is_none_or(|frame| frame.is_main() != 1) {
                return 0;
            }
            let url = address(request);
            if crate::loadable(&url) {
                return 0;
            }
            *self.shared.refused.borrow_mut() = Some(url);
            self.shared.dirty.set(true);
            1
        }

        // a middle click or a "new tab" opens in the page itself
        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: c_int,
        ) -> c_int {
            redirect(&self.shared, target_url);
            1
        }
    }
}

wrap_download_handler! {
    struct PageDownload;

    impl DownloadHandler {
        // no downloads
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> c_int {
            0
        }
    }
}

wrap_permission_handler! {
    struct PagePermission;

    impl PermissionHandler {
        // no camera, microphone or screen
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _requesting_origin: Option<&CefString>,
            _requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> c_int {
            if let Some(callback) = callback {
                callback.cancel();
            }
            1
        }

        // no other permission either (location, notifications, ...)
        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            _requesting_origin: Option<&CefString>,
            _requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> c_int {
            if let Some(callback) = callback {
                callback.cont(PermissionRequestResult::DENY);
            }
            1
        }
    }
}

wrap_client! {
    struct PageClient {
        render: RenderHandler,
        display: DisplayHandler,
        life: LifeSpanHandler,
        focus: FocusHandler,
        load: LoadHandler,
        request: RequestHandler,
        menu: ContextMenuHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn focus_handler(&self) -> Option<FocusHandler> {
            Some(self.focus.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.menu.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(PageDownload::new())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(PagePermission::new())
        }
    }
}
