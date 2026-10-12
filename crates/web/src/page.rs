//! One browser drawn into gpui: a [`Page`] entity, what its CEF callbacks
//! leave it ([`Shared`]), its frames painted as gpui images, and the
//! pointer, keys, text and IME it forwards.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use cef::{ImplBrowser as _, ImplBrowserHost as _, ImplFrame as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Where a page is: what an address bar, a title and the back, forward and
/// stop buttons show. A [`Page`] emits it whenever any of it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageState {
    /// The address the page is at: a link followed or a redirect moves it.
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// What a page's CEF callbacks (main thread, inside `do_message_loop_work`)
/// leave for the gpui side, and what the gpui side tells them back.
pub(crate) struct Shared {
    pub(crate) browser: RefCell<Option<cef::Browser>>,
    /// The page's profile directory: its cookies and cache.
    profile: PathBuf,
    /// The address to open once the engine can make the browser.
    pending: RefCell<Option<String>>,
    /// A browser was made, so one must be closed.
    made: Cell<bool>,
    pub(crate) closed: Cell<bool>,
    /// The page's size in logical pixels, for `view_rect`.
    pub(crate) size: Cell<(i32, i32)>,
    pub(crate) scale: Cell<f32>,
    /// The newest frame CEF painted, not yet shown.
    pub(crate) frame: RefCell<Option<Arc<RenderImage>>>,
    pub(crate) popup: RefCell<Option<Arc<RenderImage>>>,
    /// The `<select>` popup's rect in page coordinates, while it is open.
    pub(crate) popup_rect: Cell<Option<PopupRect>>,
    pub(crate) dirty: Cell<bool>,
    /// `on_take_focus`: Tab ran off the page's end (`true`) or start.
    pub(crate) take_focus: Cell<Option<bool>>,
    /// The pointer the page wants over its box ([`crate::cursor`]).
    pub(crate) cursor: Cell<CursorStyle>,
    /// A right-click's menu, not yet drawn ([`crate::menu`]).
    pub(crate) menu: RefCell<Option<crate::menu::Asked>>,
    pub(crate) state: RefCell<PageState>,
    /// An address the rule refused: the page says so until it moves.
    pub(crate) refused: RefCell<Option<String>>,
    /// A popup's or a new tab's address, opened here instead.
    pub(crate) redirect: RefCell<Option<String>>,
    /// The page's accessibility tree, while the app's accessibility is on.
    pub(crate) a11y: crate::a11y::Accessibility,
}

impl Shared {
    pub(crate) fn host(&self) -> Option<cef::BrowserHost> {
        self.browser.borrow().as_ref()?.host()
    }

    pub(crate) fn started(&self) -> bool {
        self.made.get()
    }

    /// A browser was made and has not closed.
    pub(crate) fn open(&self) -> bool {
        self.made.get() && !self.closed.get()
    }

    pub(crate) fn close(&self) {
        self.pending.borrow_mut().take();
        if let Some(host) = self.host() {
            host.close_browser(1);
        }
    }

    /// Drops the references CEF must not outlive.
    pub(crate) fn forget(&self) {
        self.browser.borrow_mut().take();
        self.frame.borrow_mut().take();
        self.popup.borrow_mut().take();
    }

    /// Asks for the browser of the pending address, in its profile's
    /// request context (`context`), once; it arrives in `on_after_created`.
    /// An address asked for before it arrived goes to it once it has. The
    /// address is held to [`crate::loadable`] here, at the call, as in
    /// [`Page::load`]: CEF runs `LoadURL("javascript:...")` without asking
    /// `on_before_browse`.
    pub(crate) fn create(
        self: &Rc<Self>,
        context: impl FnOnce(&Path) -> Option<cef::RequestContext>,
    ) {
        let pending = || {
            self.pending
                .borrow_mut()
                .take()
                .filter(|url| crate::loadable(url))
        };
        if self.made.get() {
            let frame = self.browser.borrow().as_ref().and_then(|b| b.main_frame());
            if let Some(frame) = frame
                && let Some(url) = pending()
            {
                frame.load_url(Some(&url.as_str().into()));
            }
            return;
        }
        let Some(url) = pending() else {
            return;
        };
        self.made
            .set(crate::handlers::create(self, &url, context(&self.profile)));
    }
}

/// One browser drawn into gpui, under one profile (cookies, cache). Made
/// with the address it opens; moved with [`Page::load`], [`Page::back`],
/// [`Page::forward`], [`Page::reload`] and [`Page::stop`]; emits its
/// [`PageState`] as it changes; closed when dropped.
pub struct Page {
    shared: Rc<Shared>,
    focus: FocusHandle,
    shown: Option<Arc<RenderImage>>,
    popup_shown: Option<Arc<RenderImage>>,
    /// The canvas origin in window coordinates, for mouse positions.
    origin: Rc<Cell<Point<Pixels>>>,
    /// The composition CEF holds, in UTF-16 units of the marked text.
    marked: Option<Range<usize>>,
    /// Why no page can show here (no sandbox helper, CEF did not start).
    unavailable: Option<SharedString>,
    /// The context menu on screen.
    menu: Option<crate::menu::Open>,
    told: PageState,
    _focus_events: [Subscription; 2],
}

impl EventEmitter<PageState> for Page {}

impl Page {
    /// A page opening `url`, its cookies and cache in `profile`. Every
    /// profile directory sits in one parent, CEF's root cache, which the
    /// first page starts CEF with.
    pub fn new(url: &str, profile: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = profile.parent().unwrap_or(&profile).to_path_buf();
        let unavailable = crate::start(&root, cx).err();
        let shared = Rc::new(Shared {
            browser: RefCell::default(),
            profile,
            pending: RefCell::default(),
            made: Cell::default(),
            closed: Cell::default(),
            size: Cell::new((800, 600)),
            scale: Cell::new(window.scale_factor()),
            frame: RefCell::default(),
            popup: RefCell::default(),
            popup_rect: Cell::default(),
            dirty: Cell::default(),
            take_focus: Cell::default(),
            cursor: Cell::default(),
            menu: RefCell::default(),
            state: RefCell::default(),
            refused: RefCell::default(),
            redirect: RefCell::default(),
            a11y: Default::default(),
        });
        if unavailable.is_none() {
            crate::register(cx.weak_entity(), shared.clone());
        }
        let focus = cx.focus_handle().tab_stop(true);
        let _focus_events = [
            cx.on_focus(&focus, window, |this: &mut Self, _, _| {
                if let Some(host) = this.shared.host() {
                    host.set_focus(1);
                }
            }),
            // the page's own menu holds the keys for the page: CEF keeps
            // its focus, else it drops the edit command chosen there
            cx.on_blur(&focus, window, |this: &mut Self, window, cx| {
                if this
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.has_keys(window, cx))
                {
                    return;
                }
                if let Some(host) = this.shared.host() {
                    host.set_focus(0);
                }
            }),
        ];
        let mut page = Self {
            shared,
            focus,
            shown: None,
            popup_shown: None,
            origin: Rc::default(),
            marked: None,
            unavailable,
            menu: None,
            told: PageState::default(),
            _focus_events,
        };
        page.load(url, cx);
        page
    }

    /// Opens `url`, if [`crate::loadable`]; else the page says it was
    /// refused and stays where it is.
    pub fn load(&mut self, url: &str, cx: &mut Context<Self>) {
        if !crate::loadable(url) {
            *self.shared.refused.borrow_mut() = Some(url.to_owned());
            cx.notify();
            return;
        }
        self.shared.refused.borrow_mut().take();
        match self
            .shared
            .browser
            .borrow()
            .as_ref()
            .and_then(|b| b.main_frame())
        {
            Some(frame) => frame.load_url(Some(&url.into())),
            None => *self.shared.pending.borrow_mut() = Some(url.to_owned()),
        }
        cx.notify();
    }

    pub fn back(&self) {
        if let Some(browser) = self.shared.browser.borrow().as_ref() {
            browser.go_back();
        }
    }

    pub fn forward(&self) {
        if let Some(browser) = self.shared.browser.borrow().as_ref() {
            browser.go_forward();
        }
    }

    pub fn reload(&self) {
        if let Some(browser) = self.shared.browser.borrow().as_ref() {
            browser.reload();
        }
    }

    pub fn stop(&self) {
        if let Some(browser) = self.shared.browser.borrow().as_ref() {
            browser.stop_load();
        }
    }

    /// Where the page is, as last emitted.
    pub fn state(&self) -> &PageState {
        &self.told
    }

    /// After a pump: a popup's address opened here, the state emitted if
    /// it moved, a redraw if CEF painted.
    pub(crate) fn pumped(&mut self, cx: &mut Context<Self>) {
        let redirect = self.shared.redirect.borrow_mut().take();
        if let Some(url) = redirect {
            self.load(&url, cx);
        }
        let state = self.shared.state.borrow().clone();
        if state != self.told {
            self.told = state.clone();
            cx.emit(state);
        }
        if self.shared.dirty.replace(false) {
            cx.notify();
        }
    }

    /// The context menu closed; the kit gives the keys back to the page.
    pub(crate) fn menu_closed(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    fn mouse(&self, position: Point<Pixels>, modifiers: u32) -> cef::MouseEvent {
        let at = position - self.origin.get();
        cef::MouseEvent {
            x: at.x.as_f32() as i32,
            y: at.y.as_f32() as i32,
            modifiers,
        }
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.button(
            event.position,
            event.button,
            &event.modifiers,
            false,
            event.click_count,
        );
    }

    fn release(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.button(
            event.position,
            event.button,
            &event.modifiers,
            true,
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
        // Enter's character, `\r`, as a keyboard types it: the page submits a
        // form and breaks a line on it (the keypress), and gpui hands a
        // control key no text to come in as
        if down && code == 0x0D {
            host.send_key_event(Some(&cef::KeyEvent {
                type_: cef::KeyEventType::CHAR,
                character: 0x0D,
                unmodified_character: 0x0D,
                ..event
            }));
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.shared.close();
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

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(reason) = &self.unavailable {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(reason.clone())
                .into_any_element();
        }
        // the newest frames in; the ones they replace out of the atlas
        let fresh = self.shared.frame.borrow_mut().take();
        if let Some(image) = fresh
            && let Some(old) = self.shown.replace(image)
        {
            let _ = window.drop_image(old);
        }
        let popup = self.shared.popup.borrow_mut().take();
        if let Some(image) = popup
            && let Some(old) = self.popup_shown.replace(image)
        {
            let _ = window.drop_image(old);
        }
        if self.shared.popup_rect.get().is_none()
            && let Some(old) = self.popup_shown.take()
        {
            let _ = window.drop_image(old);
        }
        let asked = self.shared.menu.borrow_mut().take();
        if let Some(asked) = asked {
            let origin = self.origin.get();
            self.menu = Some(crate::menu::open(asked, &self.shared, origin, window, cx));
        }
        if let Some(next) = self.shared.take_focus.take() {
            cx.defer_in(window, move |_, window, cx| {
                if next {
                    window.focus_next(cx);
                } else {
                    window.focus_prev(cx);
                }
            });
        }

        self.shared
            .a11y
            .follow(self.shared.host(), window.is_a11y_active());
        let entity = cx.entity();
        let focus = self.focus.clone();
        let shared = self.shared.clone();
        let origin = self.origin.clone();
        let image = self.shown.clone();
        let popup = self.popup_shown.clone().zip(self.shared.popup_rect.get());
        let refused = self.shared.refused.borrow().clone();
        let title = self.shared.state.borrow().title.clone();
        // the context menu sits beside the page, not in it: the page's node
        // lists only the page's own tree to assistive technology, and keys
        // pressed in the menu do not bubble into the page
        let menu = self.menu.as_ref().map(|menu| menu.element());
        let page = div()
            .id("web-page")
            // the page to assistive technology: named by its title, with
            // the page's own tree inside
            .role(Role::Group)
            .aria_label(title)
            .a11y_synthetic_children({
                let shared = self.shared.clone();
                move |tree| crate::a11y::children(&shared, tree)
            })
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .cursor(self.shared.cursor.get())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::press))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::press))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::release))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::release))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::release))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                if let Some(host) = this.shared.host() {
                    let mouse = this.mouse(
                        event.position,
                        flags(&event.modifiers, event.pressed_button),
                    );
                    host.send_mouse_move_event(Some(&mouse), 0);
                }
            }))
            // moves reach the page only inside its box: the pointer leaving
            // it (onto the app, or out of the window) is told here, so the
            // page's hover ends; typing in the page is no leave
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, window, _| {
                if !hovered && let Some(host) = this.shared.host() {
                    let mouse =
                        this.mouse(window.mouse_position(), flags(&window.modifiers(), None));
                    host.send_mouse_move_event(Some(&mouse), 1);
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
                        crate::a11y::actions(&shared, &focus, window);
                        origin.set(bounds.origin);
                        paint(&shared, bounds, image, popup, window);
                    },
                )
                .size_full(),
            )
            .when_some(refused, |page, url| {
                page.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .px_2()
                        .py_1()
                        .bg(rgb(0xfff4d6))
                        .text_color(rgb(0x5c4400))
                        .child(format!("Only https:// pages open here; not {url}")),
                )
            });
        div()
            .size_full()
            .child(page)
            .children(menu)
            .into_any_element()
    }
}

/// A `<select>` popup's x, y, width and height in page pixels.
type PopupRect = (i32, i32, i32, i32);

/// The page's newest frame (and an open `<select>` popup) into `bounds`;
/// CEF told of a new size or scale first.
fn paint(
    shared: &Shared,
    bounds: Bounds<Pixels>,
    image: Option<Arc<RenderImage>>,
    popup: Option<(Arc<RenderImage>, PopupRect)>,
    window: &mut Window,
) {
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
        let _ = window.paint_image(bounds, drawn, Corners::default(), image, 0, false);
    }
    if let Some((popup, (x, y, width, height))) = popup {
        let at = Bounds::new(
            bounds.origin + point(px(x as f32), px(y as f32)),
            size(px(width as f32), px(height as f32)),
        );
        let _ = window.paint_image(at, at, Corners::default(), popup, 0, false);
    }
}

/// Text input and IME, the way gpui's platform layer drives them: a
/// preedit is `replace_and_mark_text_in_range`, a commit (and plain typed
/// text) is `replace_text_in_range`.
impl EntityInputHandler for Page {
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
            crate::handlers::commit(&host, text);
        } else {
            // plain typing: the character events a keyboard makes, so the
            // text goes where the page's focus is (a commit with no
            // composition lands in the last text field instead)
            crate::handlers::type_text(&host, text);
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
        crate::handlers::compose(&host, text, selected.unwrap_or(length..length));
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
