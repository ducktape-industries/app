//! `ducktape-app --web-probe <url>...` (feature `cef`): Chromium pages inside
//! one gpui window through CEF off-screen rendering. A probe, not product
//! code (wt/briefs/cef-probe): it answers whether CEF and gpui share this
//! process, and what a page costs.
//!
//! Shape:
//! - subprocesses are this same binary (`child`, first thing in `main`): one
//!   binary to build, size and ship, no helper to bundle.
//! - the message loop is CEF's external pump: `on_schedule_message_pump_work`
//!   (any CEF thread) sends its delay down a channel; one gpui foreground task
//!   waits for it (or 33 ms) and calls `do_message_loop_work` on the main
//!   thread. So every CEF callback (`on_paint` included) runs on gpui's
//!   thread, between gpui's own work, and it is the loop macOS needs too
//!   (macOS has no multi-threaded loop).
//! - CEF callbacks never touch gpui: they write into the view's [`Shared`]
//!   and mark it dirty; the pump notifies the view after the work returns.
//! - a frame: `on_paint`'s BGRA buffer is copied into a gpui `RenderImage`
//!   (also BGRA), painted with `paint_image`, and the frame it replaces is
//!   dropped from the sprite atlas.
//!
//! Env knobs: `DUCKTAPE_WEB_PROBE_NO_SANDBOX=1`, `DUCKTAPE_WEB_PROBE_QUIT_SECS=n`,
//! `DUCKTAPE_WEB_PROBE_SCRIPT=korean|scroll|focus`.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use cef::{ImplBrowser as _, ImplBrowserHost as _, ImplFrame as _};
use futures::channel::mpsc;
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

gpui_kit::actions!(web_probe, [Shortcut, NextStop, Quit]);

/// The pump's inbox: `on_schedule_message_pump_work` delays, from any thread.
static PUMP: OnceLock<mpsc::UnboundedSender<i64>> = OnceLock::new();
/// `on_context_initialized` has run: browsers may be created.
static READY: AtomicBool = AtomicBool::new(false);
/// Longest wait between two `do_message_loop_work` calls (cefclient's 30 fps).
const MAX_PUMP_DELAY: Duration = Duration::from_millis(33);

/// A CEF child process: run it and return its exit code. `None` in the
/// browser process (no `--type=`), which goes on to start the app.
pub(crate) fn child() -> Option<i32> {
    if !std::env::args().any(|arg| arg.starts_with("--type=")) {
        return None;
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    Some(cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut side::app()),
        std::ptr::null_mut(),
    ))
}

/// The probe: CEF up, one window with a web view per url, CEF down.
pub(crate) fn probe(urls: Vec<String>) -> i32 {
    if urls.is_empty() {
        eprintln!("usage: ducktape-app --web-probe <url>...");
        return 2;
    }
    let (sender, inbox) = mpsc::unbounded();
    let _ = PUMP.set(sender);
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let sandbox = std::env::var_os("DUCKTAPE_WEB_PROBE_NO_SANDBOX").is_none();
    if !side::initialize(sandbox) {
        eprintln!(
            "[web-probe] cef initialize failed: exit code {}",
            cef::get_exit_code()
        );
        return 1;
    }
    eprintln!(
        "[web-probe] cef initialized (sandbox {})",
        if sandbox { "on" } else { "off" }
    );
    let views: Views = Rc::default();
    let registry = views.clone();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("tab", NextStop, Some("ProbeChrome")),
            KeyBinding::new("ctrl-k", Shortcut, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.spawn(async move |cx: &mut AsyncApp| pump(inbox, urls, registry, cx).await)
            .detach();
    });
    eprintln!("[web-probe] gpui returned; closing browsers");
    side::close(&views.borrow());
    let started = Instant::now();
    while views
        .borrow()
        .iter()
        .any(|(_, shared)| !shared.closed.get())
        && started.elapsed() < Duration::from_secs(3)
    {
        cef::do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    let open = views
        .borrow()
        .iter()
        .filter(|(_, s)| !s.closed.get())
        .count();
    eprintln!(
        "[web-probe] browsers closed in {:?} ({open} still open)",
        started.elapsed()
    );
    views.borrow_mut().clear();
    cef::shutdown();
    eprintln!("[web-probe] cef shut down");
    0
}

type Views = Rc<RefCell<Vec<(WeakEntity<WebView>, Rc<Shared>)>>>;

/// The external pump: `do_message_loop_work` when CEF asks (after its delay,
/// capped) or every [`MAX_PUMP_DELAY`]; then the views CEF painted are
/// notified. Opens the window once CEF's context is up.
async fn pump(
    mut inbox: mpsc::UnboundedReceiver<i64>,
    urls: Vec<String>,
    views: Views,
    cx: &mut AsyncApp,
) {
    let mut urls = Some(urls);
    let mut due = Instant::now();
    loop {
        let wait = due.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            let timer = cx.background_executor().timer(wait).fuse();
            futures::pin_mut!(timer);
            futures::select_biased! {
                delay = inbox.next() => {
                    let Some(delay) = delay else { return };
                    let delay = Duration::from_millis(delay.max(0) as u64).min(MAX_PUMP_DELAY);
                    due = due.min(Instant::now() + delay);
                    continue;
                }
                _ = timer => {}
            }
        }
        cef::do_message_loop_work();
        due = Instant::now() + MAX_PUMP_DELAY;
        if READY.load(Ordering::SeqCst)
            && let Some(urls) = urls.take()
        {
            let opened = cx.update(|cx| open(urls, &views, cx));
            if let Err(error) = opened {
                eprintln!("[web-probe] window did not open: {error:?}");
                return;
            }
        }
        for (view, shared) in views.borrow().iter() {
            if shared.dirty.replace(false) {
                let _ = view.update(cx, |_, cx| cx.notify());
            }
        }
        stats::tick();
    }
}

fn open(urls: Vec<String>, views: &Views, cx: &mut App) -> Result<(), String> {
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1280.), px(800.)),
                ))),
                titlebar: None,
                app_id: Some("dev.ducktape.web-probe".into()),
                ..Default::default()
            },
            |window, cx| {
                let pages: Vec<Entity<WebView>> = urls
                    .iter()
                    .map(|url| {
                        let view = cx.new(|cx| WebView::new(url, window, cx));
                        let shared = view.read(cx).shared.clone();
                        views.borrow_mut().push((view.downgrade(), shared));
                        view
                    })
                    .collect();
                cx.new(|cx| Probe {
                    pages,
                    popover: true,
                    button: cx.focus_handle().tab_stop(true).tab_index(0),
                })
            },
        )
        .map_err(|error| format!("{error:?}"))?;
    cx.activate(true);
    if let Ok(script) = std::env::var("DUCKTAPE_WEB_PROBE_SCRIPT") {
        let probe = window;
        cx.spawn(async move |cx: &mut AsyncApp| script::run(&script, probe, cx).await)
            .detach();
    }
    if let Some(secs) = std::env::var("DUCKTAPE_WEB_PROBE_QUIT_SECS")
        .ok()
        .and_then(|secs| secs.parse::<u64>().ok())
    {
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_secs(secs))
                .await;
            eprintln!("[web-probe] quit after {secs}s");
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    }
    Ok(())
}

/// What CEF's callbacks (main thread, inside `do_message_loop_work`) leave
/// for the gpui side, and what the gpui side tells them back.
#[derive(Default)]
pub(crate) struct Shared {
    browser: RefCell<Option<cef::Browser>>,
    /// The view's size in logical pixels, for `view_rect`.
    size: Cell<(i32, i32)>,
    scale: Cell<f32>,
    /// The newest frame CEF painted, not yet shown.
    frame: RefCell<Option<Painted>>,
    popup: RefCell<Option<Painted>>,
    /// The `<select>` popup's rect in view coordinates, while it is open.
    popup_rect: Cell<Option<(i32, i32, i32, i32)>>,
    dirty: Cell<bool>,
    loaded: Cell<bool>,
    closed: Cell<bool>,
    /// `on_take_focus`: Tab ran off the page's end (`true`) or start.
    take_focus: Cell<Option<bool>>,
}

impl Shared {
    fn host(&self) -> Option<cef::BrowserHost> {
        self.browser.borrow().as_ref()?.host()
    }

    fn run_js(&self, code: &str) {
        if let Some(frame) = self.browser.borrow().as_ref().and_then(|b| b.main_frame()) {
            frame.execute_java_script(Some(&code.into()), Some(&"probe://script".into()), 0);
        }
    }
}

struct Painted {
    image: Arc<RenderImage>,
    at: Instant,
}

/// One CEF browser drawn into gpui.
pub(crate) struct WebView {
    shared: Rc<Shared>,
    focus: FocusHandle,
    shown: Option<Arc<RenderImage>>,
    popup_shown: Option<Arc<RenderImage>>,
    /// The canvas origin in window coordinates, for mouse positions.
    origin: Rc<Cell<Point<Pixels>>>,
    /// The composition CEF holds, in UTF-16 units of the marked text.
    marked: Option<Range<usize>>,
    _focus_events: [Subscription; 2],
}

impl WebView {
    fn new(url: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = Rc::new(Shared {
            size: Cell::new((1280, 760)),
            scale: Cell::new(window.scale_factor()),
            ..Default::default()
        });
        let browser = side::create(&shared, url);
        eprintln!("[web-probe] browser for {url}: {}", browser.is_some());
        *shared.browser.borrow_mut() = browser;
        let focus = cx.focus_handle().tab_stop(true).tab_index(1);
        let _focus_events = [
            cx.on_focus(&focus, window, |this: &mut Self, _, _| {
                eprintln!("[web-probe] gpui focus -> page");
                if let Some(host) = this.shared.host() {
                    host.set_focus(1);
                }
            }),
            cx.on_blur(&focus, window, |this: &mut Self, _, _| {
                eprintln!("[web-probe] gpui focus left page");
                if let Some(host) = this.shared.host() {
                    host.set_focus(0);
                }
            }),
        ];
        Self {
            shared,
            focus,
            shown: None,
            popup_shown: None,
            origin: Rc::default(),
            marked: None,
            _focus_events,
        }
    }

    fn mouse(&self, position: Point<Pixels>, modifiers: u32) -> cef::MouseEvent {
        let at = position - self.origin.get();
        cef::MouseEvent {
            x: at.x.as_f32() as i32,
            y: at.y.as_f32() as i32,
            modifiers,
        }
    }

    fn click(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.button(
            event.position,
            event.button,
            &event.modifiers,
            false,
            event.click_count,
        );
    }

    fn button(
        &self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: &Modifiers,
        up: bool,
        clicks: usize,
    ) {
        let kind = match button {
            MouseButton::Left => cef::MouseButtonType::LEFT,
            MouseButton::Right => cef::MouseButtonType::RIGHT,
            MouseButton::Middle => cef::MouseButtonType::MIDDLE,
            MouseButton::Navigate(_) => return,
        };
        if let Some(host) = self.shared.host() {
            let event = self.mouse(position, flags(modifiers, Some(button)));
            host.send_mouse_click_event(Some(&event), kind, up as i32, clicks as i32);
        }
    }

    fn key(&self, keystroke: &Keystroke, down: bool) {
        let Some(code) = windows_key_code(&keystroke.key) else {
            return;
        };
        let Some(host) = self.shared.host() else {
            return;
        };
        let event = cef::KeyEvent {
            type_: if down {
                cef::KeyEventType::RAWKEYDOWN
            } else {
                cef::KeyEventType::KEYUP
            },
            modifiers: flags(&keystroke.modifiers, None),
            windows_key_code: code,
            ..Default::default()
        };
        host.send_key_event(Some(&event));
    }
}

/// CEF `EVENTFLAG_*` bits.
fn flags(modifiers: &Modifiers, button: Option<MouseButton>) -> u32 {
    let mut bits = 0;
    for (on, bit) in [
        (modifiers.shift, 1 << 1),
        (modifiers.control, 1 << 2),
        (modifiers.alt, 1 << 3),
        (button == Some(MouseButton::Left), 1 << 4),
        (button == Some(MouseButton::Middle), 1 << 5),
        (button == Some(MouseButton::Right), 1 << 6),
        (modifiers.platform, 1 << 7),
    ] {
        if on {
            bits |= bit;
        }
    }
    bits
}

/// gpui key name -> Windows virtual key code, the code CEF's key events
/// carry. Printable text does not come through here: it arrives as gpui
/// text input (`replace_text_in_range`), so a key is never typed twice.
fn windows_key_code(key: &str) -> Option<i32> {
    Some(match key {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "delete" => 0x2E,
        _ => match key.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => byte.to_ascii_uppercase() as i32,
            _ => return None,
        },
    })
}

impl Render for WebView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // the newest frames in; the ones they replace out of the atlas
        let fresh = self.shared.frame.borrow_mut().take();
        let fresh_at = fresh.as_ref().map(|painted| painted.at);
        if let Some(painted) = fresh
            && let Some(old) = self.shown.replace(painted.image)
        {
            let _ = window.drop_image(old);
        }
        if let Some(painted) = self.shared.popup.borrow_mut().take()
            && let Some(old) = self.popup_shown.replace(painted.image)
        {
            let _ = window.drop_image(old);
        }
        if self.shared.popup_rect.get().is_none()
            && let Some(old) = self.popup_shown.take()
        {
            let _ = window.drop_image(old);
        }
        if let Some(next) = self.shared.take_focus.take() {
            eprintln!("[web-probe] page gave focus back (next={next})");
            cx.defer_in(window, move |_, window, cx| {
                if next {
                    window.focus_next(cx);
                } else {
                    window.focus_prev(cx);
                }
            });
        }

        let entity = cx.entity();
        let focus = self.focus.clone();
        let shared = self.shared.clone();
        let origin = self.origin.clone();
        let image = self.shown.clone();
        let popup = self.popup_shown.clone().zip(self.shared.popup_rect.get());
        div()
            .id("web-view")
            .size_full()
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::click))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::click))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    this.button(
                        event.position,
                        event.button,
                        &event.modifiers,
                        true,
                        event.click_count,
                    )
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    this.button(
                        event.position,
                        event.button,
                        &event.modifiers,
                        true,
                        event.click_count,
                    )
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                if let Some(host) = this.shared.host() {
                    let mouse = this.mouse(
                        event.position,
                        flags(&event.modifiers, event.pressed_button),
                    );
                    host.send_mouse_move_event(Some(&mouse), 0);
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, _| {
                if let Some(host) = this.shared.host() {
                    let delta = event.delta.pixel_delta(px(40.));
                    let mouse = this.mouse(event.position, flags(&event.modifiers, None));
                    host.send_mouse_wheel_event(
                        Some(&mouse),
                        delta.x.as_f32() as i32,
                        delta.y.as_f32() as i32,
                    );
                }
            }))
            // no stop_propagation: an unhandled key with a character goes on
            // to gpui's text input, which is where the page gets its text
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, _| this.key(&event.keystroke, true)),
            )
            .on_key_up(
                cx.listener(|this, event: &KeyUpEvent, _, _| this.key(&event.keystroke, false)),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                        origin.set(bounds.origin);
                        let scale = window.scale_factor();
                        let logical = (
                            bounds.size.width.as_f32().round() as i32,
                            bounds.size.height.as_f32().round() as i32,
                        );
                        let host = shared.host();
                        if shared.scale.get() != scale {
                            shared.scale.set(scale);
                            if let Some(host) = &host {
                                host.notify_screen_info_changed();
                            }
                        }
                        if shared.size.get() != logical {
                            shared.size.set(logical);
                            if let Some(host) = &host {
                                host.was_resized();
                            }
                        }
                        if let Some(image) = image {
                            let pixels = image.size(0);
                            let drawn = Bounds::new(
                                bounds.origin,
                                size(
                                    px(pixels.width.0 as f32 / scale),
                                    px(pixels.height.0 as f32 / scale),
                                ),
                            );
                            let _ = window.paint_image(
                                bounds,
                                drawn,
                                Corners::default(),
                                image,
                                0,
                                false,
                            );
                            if let Some(at) = fresh_at {
                                stats::shown(at.elapsed());
                            }
                        }
                        if let Some((popup, (x, y, width, height))) = popup {
                            let at = Bounds::new(
                                bounds.origin + point(px(x as f32), px(y as f32)),
                                size(px(width as f32), px(height as f32)),
                            );
                            let _ = window.paint_image(at, at, Corners::default(), popup, 0, false);
                        }
                    },
                )
                .size_full(),
            )
    }
}

/// Text input and IME, the way gpui's platform layer drives them: a
/// preedit is `replace_and_mark_text_in_range`, a commit (and plain typed
/// text) is `replace_text_in_range`.
impl EntityInputHandler for WebView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    // ponytail: the page's selection is not mirrored to gpui (CEF reports it in
    // `on_text_selection_changed`); the IME candidate window has no anchor
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.marked.take().is_some()
            && let Some(host) = self.shared.host()
        {
            host.ime_finish_composing_text(0);
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let composing = self.marked.take().is_some();
        let Some(host) = self.shared.host() else {
            return;
        };
        if text.is_empty() {
            if composing {
                host.ime_cancel_composition();
            }
            return;
        }
        if composing {
            side::commit(&host, text);
        } else {
            // plain typing: the character events a keyboard makes, so the
            // text goes where the page's focus is (a commit with no
            // composition lands in the last text field instead)
            side::type_text(&host, text);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Some(host) = self.shared.host() else {
            return;
        };
        if text.is_empty() {
            if self.marked.take().is_some() {
                host.ime_cancel_composition();
            }
            return;
        }
        let length = text.encode_utf16().count();
        side::compose(&host, text, selected.unwrap_or(length..length));
        self.marked = Some(0..length);
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// The window: a strip of gpui chrome (a button with a tooltip that toggles
/// a popover drawn over the page) above the pages.
struct Probe {
    pages: Vec<Entity<WebView>>,
    popover: bool,
    button: FocusHandle,
}

impl Render for Probe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111111))
            .on_action(|_: &Shortcut, _, _| eprintln!("[web-probe] app shortcut ctrl-k fired"))
            .child(
                div()
                    .key_context("ProbeChrome")
                    .on_action(|_: &NextStop, window, cx| window.focus_next(cx))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .bg(rgb(0xe4e7f0))
                    .child(
                        div()
                            .id("overlay-button")
                            .track_focus(&self.button)
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x3355cc))
                            .focus(|style| style.border_2().border_color(rgb(0xff8800)))
                            .text_color(rgb(0xffffff))
                            .child("Overlay")
                            .tooltip(|_, cx| cx.new(|_| Tip).into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.popover = !this.popover;
                                cx.notify();
                            })),
                    )
                    .child("ducktape web probe (gpui chrome)"),
            )
            .children(
                self.pages
                    .iter()
                    .map(|page| div().flex_1().min_h(px(40.)).child(page.clone())),
            )
            .when(self.popover, |root| {
                root.child(deferred(
                    anchored().position(point(px(120.), px(24.))).child(
                        div()
                            .w(px(300.))
                            .h(px(140.))
                            .p_3()
                            .rounded_lg()
                            .border_2()
                            .border_color(rgb(0xcc3300))
                            .bg(rgb(0xfff2b3))
                            .shadow_lg()
                            .child("gpui popover, drawn over the web page"),
                    ),
                ))
            })
    }
}

struct Tip;

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(rgb(0x222222))
            .text_color(rgb(0xffffff))
            .child("gpui tooltip, over the page")
    }
}

/// Frame counters, printed once a second to stderr.
mod stats {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    struct Stats {
        since: Instant,
        cef_frames: u32,
        copy_ms: Vec<f32>,
        latency_ms: Vec<f32>,
    }

    thread_local! {
        static STATS: RefCell<Stats> = RefCell::new(Stats {
            since: Instant::now(),
            cef_frames: 0,
            copy_ms: Vec::new(),
            latency_ms: Vec::new(),
        });
    }

    /// `on_paint` ran; `copy` is its buffer copy into a `RenderImage`.
    pub(super) fn painted(copy: Duration) {
        STATS.with_borrow_mut(|stats| {
            stats.cef_frames += 1;
            stats.copy_ms.push(copy.as_secs_f32() * 1000.);
        });
    }

    /// gpui painted a frame `latency` after `on_paint` delivered it.
    pub(super) fn shown(latency: Duration) {
        STATS.with_borrow_mut(|stats| stats.latency_ms.push(latency.as_secs_f32() * 1000.));
    }

    pub(super) fn tick() {
        STATS.with_borrow_mut(|stats| {
            if stats.since.elapsed() < Duration::from_secs(1) {
                return;
            }
            if stats.cef_frames > 0 || !stats.latency_ms.is_empty() {
                eprintln!(
                    "[web-probe] stats cef_fps={} shown_fps={} copy_ms p50={:.2} max={:.2} paint_to_shown_ms p50={:.2} p95={:.2} max={:.2}",
                    stats.cef_frames,
                    stats.latency_ms.len(),
                    percentile(&mut stats.copy_ms, 0.5),
                    percentile(&mut stats.copy_ms, 1.0),
                    percentile(&mut stats.latency_ms, 0.5),
                    percentile(&mut stats.latency_ms, 0.95),
                    percentile(&mut stats.latency_ms, 1.0),
                );
            }
            stats.since = Instant::now();
            stats.cef_frames = 0;
            stats.copy_ms.clear();
            stats.latency_ms.clear();
        });
    }

    fn percentile(samples: &mut [f32], at: f32) -> f32 {
        if samples.is_empty() {
            return 0.;
        }
        samples.sort_by(f32::total_cmp);
        samples[((samples.len() - 1) as f32 * at).round() as usize]
    }
}

/// Scripted runs: input the probe drives itself, so a run measures the same
/// thing every time.
mod script {
    use super::*;

    pub(super) async fn run(script: &str, window: WindowHandle<Probe>, cx: &mut AsyncApp) {
        let executor = cx.background_executor().clone();
        let sleep = |ms: u64| executor.timer(Duration::from_millis(ms));
        let Ok(page) = window.read_with(cx, |probe, _| probe.pages[0].clone()) else {
            return;
        };
        let shared = page.read_with(cx, |view, _| view.shared.clone());
        while !shared.loaded.get() {
            sleep(50).await;
        }
        sleep(500).await;
        eprintln!("[web-probe] script {script}: page loaded");
        let focus = |cx: &mut AsyncApp| {
            let _ = window.update(cx, |_, window, cx| {
                let handle = page.read(cx).focus.clone();
                window.focus(&handle, cx);
            });
        };
        match script {
            "korean" => {
                focus(cx);
                shared.run_js("document.getElementById('t').focus()");
                sleep(300).await;
                for (commit, text) in [
                    (false, "ㅎ"),
                    (false, "하"),
                    (false, "한"),
                    (true, "한"),
                    (false, "ㄱ"),
                    (false, "그"),
                    (false, "글"),
                    (true, "글"),
                ] {
                    eprintln!(
                        "[web-probe] ime {} {text}",
                        if commit { "commit" } else { "preedit" }
                    );
                    let _ = window.update(cx, |_, window, cx| {
                        page.update(cx, |view, cx| {
                            if commit {
                                view.replace_text_in_range(None, text, window, cx);
                            } else {
                                view.replace_and_mark_text_in_range(None, text, None, window, cx);
                            }
                        })
                    });
                    sleep(150).await;
                }
                sleep(300).await;
                shared.run_js("document.title = 'final=' + document.getElementById('t').value");
                eprintln!("[web-probe] script korean done");
            }
            "scroll" => {
                let (width, height) = shared.size.get();
                for _ in 0..300 {
                    if let Some(host) = shared.host() {
                        let at = cef::MouseEvent {
                            x: width / 2,
                            y: height / 2,
                            modifiers: 0,
                        };
                        host.send_mouse_wheel_event(Some(&at), 0, -60);
                    }
                    sleep(16).await;
                }
                eprintln!("[web-probe] script scroll done");
            }
            "focus" => {
                focus(cx);
                eprintln!("[web-probe] script focus: page focused");
            }
            other => eprintln!("[web-probe] unknown script {other}"),
        }
    }
}

/// Everything that names CEF's own types (they clash with gpui's names).
mod side {
    use super::{READY, Shared, stats};
    use cef::*;
    use std::os::raw::c_int;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    /// CEF's "no range": `CefRange::InvalidRange()`.
    const NO_RANGE: Range = Range {
        from: u32::MAX,
        to: u32::MAX,
    };

    pub(super) fn app() -> App {
        ProbeApp::new()
    }

    pub(super) fn initialize(sandbox: bool) -> bool {
        let args = args::Args::new();
        let cache = std::env::temp_dir().join("ducktape-web-probe");
        let settings = Settings {
            no_sandbox: !sandbox as c_int,
            windowless_rendering_enabled: 1,
            external_message_pump: 1,
            root_cache_path: cache.to_string_lossy().as_ref().into(),
            log_severity: LogSeverity::WARNING,
            ..Default::default()
        };
        cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app()),
            std::ptr::null_mut(),
        ) == 1
    }

    pub(super) fn create(shared: &Rc<Shared>, url: &str) -> Option<Browser> {
        let window_info = WindowInfo {
            windowless_rendering_enabled: 1,
            ..Default::default()
        };
        let settings = BrowserSettings {
            windowless_frame_rate: 60,
            ..Default::default()
        };
        let mut client = ViewClient::new(
            ViewRender::new(shared.clone()),
            ViewDisplay::new(shared.clone()),
            ViewLife::new(shared.clone()),
            ViewFocus::new(shared.clone()),
            ViewLoad::new(shared.clone()),
        );
        browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&url.into()),
            Some(&settings),
            None,
            None,
        )
    }

    /// Close every browser and drop the references the views keep.
    pub(super) fn close(views: &[(gpui_kit::WeakEntity<super::WebView>, Rc<Shared>)]) {
        for (_, shared) in views {
            if let Some(host) = shared.host() {
                host.close_browser(1);
            }
        }
        for (_, shared) in views {
            shared.browser.borrow_mut().take();
            shared.frame.borrow_mut().take();
            shared.popup.borrow_mut().take();
        }
    }

    pub(super) fn compose(host: &BrowserHost, text: &str, selected: std::ops::Range<usize>) {
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

    pub(super) fn commit(host: &BrowserHost, text: &str) {
        host.ime_commit_text(Some(&text.into()), Some(&NO_RANGE), 0);
    }

    pub(super) fn type_text(host: &BrowserHost, text: &str) {
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

    wrap_app! {
        struct ProbeApp;

        impl App {
            fn on_before_command_line_processing(
                &self,
                _process_type: Option<&CefString>,
                command_line: Option<&mut CommandLine>,
            ) {
                let Some(line) = command_line else { return };
                // software GL box: no GPU process; OSR paints in software
                for switch in ["disable-gpu", "disable-gpu-compositing", "noerrdialogs"] {
                    line.append_switch(Some(&switch.into()));
                }
            }

            fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
                Some(ProbeProcess::new())
            }
        }
    }

    wrap_browser_process_handler! {
        struct ProbeProcess;

        impl BrowserProcessHandler {
            fn on_context_initialized(&self) {
                READY.store(true, Ordering::SeqCst);
            }

            fn on_schedule_message_pump_work(&self, delay_ms: i64) {
                if let Some(pump) = super::PUMP.get() {
                    let _ = pump.unbounded_send(delay_ms);
                }
            }
        }
    }

    wrap_render_handler! {
        struct ViewRender {
            shared: Rc<Shared>,
        }

        impl RenderHandler {
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
                let at = Instant::now();
                let length = width as usize * height as usize * 4;
                // SAFETY: CEF passes a width x height BGRA buffer, valid for this call
                let bytes = unsafe { std::slice::from_raw_parts(buffer, length) }.to_vec();
                let Some(pixels) = image::ImageBuffer::from_raw(width as u32, height as u32, bytes) else {
                    return;
                };
                let image = Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(pixels)]));
                stats::painted(at.elapsed());
                let painted = Some(super::Painted { image, at });
                if type_ == PaintElementType::POPUP {
                    *self.shared.popup.borrow_mut() = painted;
                } else {
                    *self.shared.frame.borrow_mut() = painted;
                }
                self.shared.dirty.set(true);
            }
        }
    }

    wrap_display_handler! {
        struct ViewDisplay {
            shared: Rc<Shared>,
        }

        impl DisplayHandler {
            fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
                let title = title.map(|title| title.to_string()).unwrap_or_default();
                eprintln!("[web-probe] title: {title}");
                let _ = &self.shared;
            }

            fn on_console_message(
                &self,
                _browser: Option<&mut Browser>,
                _level: LogSeverity,
                message: Option<&CefString>,
                _source: Option<&CefString>,
                _line: c_int,
            ) -> c_int {
                let message = message.map(|m| m.to_string()).unwrap_or_default();
                eprintln!("[web-probe] console: {message}");
                1
            }
        }
    }

    wrap_life_span_handler! {
        struct ViewLife {
            shared: Rc<Shared>,
        }

        impl LifeSpanHandler {
            fn on_before_close(&self, _browser: Option<&mut Browser>) {
                self.shared.closed.set(true);
            }
        }
    }

    wrap_focus_handler! {
        struct ViewFocus {
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
        struct ViewLoad {
            shared: Rc<Shared>,
        }

        impl LoadHandler {
            fn on_loading_state_change(
                &self,
                _browser: Option<&mut Browser>,
                is_loading: c_int,
                _can_go_back: c_int,
                _can_go_forward: c_int,
            ) {
                if is_loading == 0 {
                    eprintln!("[web-probe] page loaded");
                    self.shared.loaded.set(true);
                }
                self.shared.dirty.set(true);
            }
        }
    }

    wrap_client! {
        struct ViewClient {
            render: RenderHandler,
            display: DisplayHandler,
            life: LifeSpanHandler,
            focus: FocusHandler,
            load: LoadHandler,
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
        }
    }
}
