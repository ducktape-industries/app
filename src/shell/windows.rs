//! Where an OS window goes: a pop-out off its source window (`cascade`,
//! `unseated`), a view's float in its display's corner (`corner`), the
//! launcher centred (`centered`), above other apps' windows or back among
//! them (`keep_above`), on every Space (`join_all_spaces`); and how one
//! leaves the screen (`remove`). Opening one, and which are kept above, is
//! `entities::Windows`'.

use super::layout;
use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};

/// A window's size before the person resizes it.
pub(in crate::shell) const WINDOW_SIZE: (f32, f32) = (1280., 800.);

/// How far a pop-out steps down and right of the window it left.
const CASCADE: f32 = 32.;

/// The narrowest and lowest a pop-out goes, whatever its view: its own
/// title strip and bar are not laid out under this.
pub(in crate::shell) const POPOUT_MIN: f32 = 480.;

/// The narrowest a pop-out holding `program`'s view goes: the view's own
/// minimum once it is drawn, never under [`POPOUT_MIN`].
pub(in crate::shell) fn popout_min(program: &str) -> f32 {
    crate::runtime::min_width(program).map_or(POPOUT_MIN, |min_width| min_width.max(POPOUT_MIN))
}

pub(in crate::shell) fn min_size(kind: super::WindowKind) -> Size<Pixels> {
    match kind {
        super::WindowKind::Console => size(px(720.), px(480.)),
        super::WindowKind::View { program } => size(px(popout_min(program)), px(POPOUT_MIN)),
    }
}

/// Where a pop-out opens: stepped off its source window so it does not
/// land exactly on top of it, and pulled back inside `display` when the
/// step would push it off an edge.
pub(in crate::shell) fn cascade(
    source: Bounds<Pixels>,
    display: Option<Bounds<Pixels>>,
) -> Bounds<Pixels> {
    let extent = size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1));
    let origin = point(source.origin.x + px(CASCADE), source.origin.y + px(CASCADE));
    inside(origin, extent, display)
}

/// How far a view's float sits in from its display's work area.
const FLOAT_INSET: f32 = 24.;

/// Where a view's first float opens: `extent` in the bottom-right corner
/// of `display`'s work area, [`FLOAT_INSET`] in from both edges, and
/// never past its top-left corner when it doesn't fit.
pub(in crate::shell) fn corner(extent: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let origin = point(
        display.origin.x + display.size.width - extent.width - px(FLOAT_INSET),
        display.origin.y + display.size.height - extent.height - px(FLOAT_INSET),
    );
    inside(origin, extent, Some(display))
}

/// Where the launcher sits when the desk shrinks to it: centred in
/// `display`, and never past its top-left corner when it doesn't fit.
pub(in crate::shell) fn centered(extent: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let origin = point(
        display.origin.x + ((display.size.width - extent.width) / 2.).max(px(0.)),
        display.origin.y + ((display.size.height - extent.height) / 2.).max(px(0.)),
    );
    Bounds::new(origin, extent)
}

/// Where a window leaving the desk opens: on the screen right where it sat,
/// past the console's chrome (`inset`: under the bar, right of the
/// sidebar) rather than over it, at least `min_w` wide ([`popout_min`]),
/// and inside `display`. With no place on the desk to keep, it cascades.
pub(in crate::shell) fn unseated(
    source: Bounds<Pixels>,
    inset: super::layers::Inset,
    frame: Option<layout::Frame>,
    display: Option<Bounds<Pixels>>,
    min_w: f32,
) -> Bounds<Pixels> {
    let Some(frame) = frame else {
        return cascade(source, display);
    };
    let extent = size(px(frame.w.max(min_w)), px(frame.h.max(POPOUT_MIN)));
    let origin = point(
        source.origin.x + px(inset.left + frame.x),
        source.origin.y + px(inset.top + frame.y),
    );
    inside(origin, extent, display)
}

/// `extent` at `origin`, pulled back inside `display` where it would hang
/// off an edge, and never past its top-left corner.
fn inside(
    mut origin: Point<Pixels>,
    extent: Size<Pixels>,
    display: Option<Bounds<Pixels>>,
) -> Bounds<Pixels> {
    if let Some(display) = display {
        let right = display.origin.x + display.size.width - extent.width;
        let bottom = display.origin.y + display.size.height - extent.height;
        origin.x = origin.x.min(right).max(display.origin.x);
        origin.y = origin.y.min(bottom).max(display.origin.y);
    }
    Bounds::new(origin, extent)
}

/// Lets go of a window's input: the keys, and whatever its last draw
/// held.
pub(in crate::shell) fn release_window_input(
    window: &mut gpui_kit::Window,
    cx: &mut gpui_kit::App,
) {
    window.blur(cx);
    window.draw(cx).clear(cx);
}

/// Takes a window off the screen, its input let go first. Deferred:
/// releasing input draws the window, and the caller is most often in the
/// middle of updating it. `Windows::forget_closed` then forgets it.
pub(in crate::shell) fn remove(window: gpui_kit::AnyWindowHandle, cx: &mut gpui_kit::App) {
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            release_window_input(window, cx);
            window.remove_window();
        });
    });
}

/// Whether this platform keeps a window above other apps' windows: macOS
/// does (a window level) and X11 does (EWMH's `_NET_WM_STATE_ABOVE`, which
/// the window manager carries out). Wayland's xdg-shell has no such
/// request, so there Keep on Top is not offered at all.
pub(in crate::shell) fn can_keep_above(cx: &gpui_kit::App) -> bool {
    cx.compositor_name() != "Wayland"
}

/// Tells the platform to keep `window` above other apps' windows, or to let
/// it back among them. gpui sets a window's level only as it opens it
/// (`WindowKind`), and nothing it does later (raising, activation, full
/// screen) sets it again, so this holds until it is told otherwise.
pub(in crate::shell) fn keep_above(window: &gpui_kit::Window, above: bool) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let raw = match HasWindowHandle::window_handle(window) {
        Ok(handle) => handle.as_raw(),
        // the test platform's windows have no native handle: what it was
        // told is kept for the test to read
        #[cfg(test)]
        Err(raw_window_handle::HandleError::NotSupported) => {
            KEPT_ABOVE
                .with_borrow_mut(|told| told.push((window.window_handle().window_id(), above)));
            return Ok(());
        }
        Err(error) => return Err(format!("no native window: {error}")),
    };
    match raw {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(handle) => mac_keep_above(handle.ns_view, above),
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(handle) => x11_keep_above(window, handle.window.get(), above),
        other => Err(format!("{other:?} windows can't be kept on top")),
    }
}

/// macOS: `window` on every Space and over another app's full screen, as
/// a floating call window is (a view's float). Elsewhere a window manager
/// keeps a window above on every workspace already, or not at all.
pub(in crate::shell) fn activate(
    window: &gpui_kit::Window,
    token: Option<&str>,
) -> Result<(), String> {
    use raw_window_handle::HasWindowHandle;
    let raw = match HasWindowHandle::window_handle(window) {
        Ok(handle) => handle.as_raw(),
        #[cfg(test)]
        Err(raw_window_handle::HandleError::NotSupported) => {
            let id = window.window_handle().window_id();
            ACTIVATED.with_borrow_mut(|told| told.push((id, token.map(str::to_owned))));
            return Ok(());
        }
        Err(error) => return Err(format!("no native window: {error}")),
    };
    #[cfg(target_os = "linux")]
    if let raw_window_handle::RawWindowHandle::Xcb(handle) = raw {
        return x11_activate(window, handle.window.get(), token);
    }
    #[cfg(target_os = "linux")]
    if let (raw_window_handle::RawWindowHandle::Wayland(handle), Some(token)) = (raw, token) {
        return wayland_activate(window, handle.surface, token);
    }
    let _ = (raw, token);
    Ok(())
}

pub(in crate::shell) fn traffic_lights(kind: super::WindowKind) -> Point<Pixels> {
    let bar = match kind {
        super::WindowKind::Console => super::layers::BAR,
        super::WindowKind::View { .. } => super::layers::TITLE - 1.,
    };
    point(px(11.), px((bar - 14.) / 2.))
}

pub(in crate::shell) fn set_resizable(
    window: &gpui_kit::Window,
    resizable: bool,
    lights: Option<Point<Pixels>>,
) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let raw = match HasWindowHandle::window_handle(window) {
        Ok(handle) => handle.as_raw(),
        #[cfg(test)]
        Err(raw_window_handle::HandleError::NotSupported) => {
            let id = window.window_handle().window_id();
            RESIZABLE.with_borrow_mut(|told| told.push((id, resizable)));
            return Ok(());
        }
        Err(error) => return Err(format!("no native window: {error}")),
    };
    match raw {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(handle) => {
            mac_set_resizable(handle.ns_view, resizable)?;
            if let Some(lights) = lights {
                window.set_traffic_light_position(lights);
            }
            Ok(())
        }
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(handle) => x11_set_resizable(window, handle.window.get(), resizable),
        _ => {
            let _ = lights;
            Ok(())
        }
    }
}

pub(in crate::shell) fn set_bounds(
    window: &mut gpui_kit::Window,
    bounds: Bounds<Pixels>,
    cx: &gpui_kit::App,
) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let raw = match HasWindowHandle::window_handle(window) {
        Ok(handle) => handle.as_raw(),
        #[cfg(test)]
        Err(raw_window_handle::HandleError::NotSupported) => {
            window.set_bounds(bounds);
            return Ok(());
        }
        Err(error) => return Err(format!("no native window: {error}")),
    };
    match raw {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(_) => {
            mac_set_bounds(window, bounds, cx);
            Ok(())
        }
        #[cfg(target_os = "linux")]
        RawWindowHandle::Xcb(handle) => x11_set_bounds(window, handle.window.get(), bounds),
        _ => {
            let _ = cx;
            window.resize(bounds.size);
            Ok(())
        }
    }
}

pub(in crate::shell) fn double_click_fixed(window: &gpui_kit::Window) {
    #[cfg(target_os = "macos")]
    if mac_double_click_minimizes() {
        window.minimize_window();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

pub(in crate::shell) fn join_all_spaces(window: &gpui_kit::Window) {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        /// `NSWindowCollectionBehaviorCanJoinAllSpaces`,
        /// `NSWindowCollectionBehaviorFullScreenAuxiliary`.
        const ALL_SPACES: usize = 1 << 0;
        const OVER_FULL_SCREEN: usize = 1 << 8;
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        // SAFETY: gpui's own NSView, alive while its `Window` is borrowed;
        // this runs on the main thread, as all of gpui's window calls do.
        unsafe {
            let ns_window: *mut AnyObject = msg_send![view, window];
            if ns_window.is_null() {
                return;
            }
            let behavior: usize = msg_send![ns_window, collectionBehavior];
            let _: () = msg_send![
                ns_window,
                setCollectionBehavior: behavior | ALL_SPACES | OVER_FULL_SCREEN
            ];
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

#[cfg(test)]
thread_local! {
    pub(in crate::shell) static ACTIVATED: std::cell::RefCell<Vec<(gpui_kit::WindowId, Option<String>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    pub(in crate::shell) static RESIZABLE: std::cell::RefCell<Vec<(gpui_kit::WindowId, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    /// What the test platform was told to keep above, in order.
    pub(in crate::shell) static KEPT_ABOVE: std::cell::RefCell<Vec<(gpui_kit::WindowId, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// macOS: the window's level. Floating is a palette's level, above every
/// app's normal windows, where Zoom's and Slack's floating call windows sit.
#[cfg(target_os = "macos")]
fn mac_keep_above(ns_view: std::ptr::NonNull<std::ffi::c_void>, above: bool) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    /// `NSFloatingWindowLevel` and `NSNormalWindowLevel`.
    const FLOATING: isize = 3;
    const NORMAL: isize = 0;
    let view = ns_view.as_ptr().cast::<AnyObject>();
    // SAFETY: gpui's own NSView, alive while its `Window` is borrowed; this
    // runs on the main thread, as all of gpui's window calls do.
    unsafe {
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return Err("the view is in no window".to_owned());
        }
        let _: () = msg_send![ns_window, setLevel: if above { FLOATING } else { NORMAL }];
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mac_window(
    ns_view: std::ptr::NonNull<std::ffi::c_void>,
) -> Result<*mut objc2::runtime::AnyObject, String> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let view = ns_view.as_ptr().cast::<AnyObject>();
    let ns_window: *mut AnyObject = unsafe { msg_send![view, window] };
    if ns_window.is_null() {
        return Err("the view is in no window".to_owned());
    }
    Ok(ns_window)
}

#[cfg(target_os = "macos")]
fn mac_set_resizable(
    ns_view: std::ptr::NonNull<std::ffi::c_void>,
    resizable: bool,
) -> Result<(), String> {
    use objc2::msg_send;
    const NS_WINDOW_STYLE_MASK_RESIZABLE: usize = 1 << 3;
    let ns_window = mac_window(ns_view)?;
    unsafe {
        let mask: usize = msg_send![ns_window, styleMask];
        let mask = if resizable {
            mask | NS_WINDOW_STYLE_MASK_RESIZABLE
        } else {
            mask & !NS_WINDOW_STYLE_MASK_RESIZABLE
        };
        let _: () = msg_send![ns_window, setStyleMask: mask];
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mac_set_bounds(window: &gpui_kit::Window, bounds: Bounds<Pixels>, cx: &gpui_kit::App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window
        .spawn(cx, async move |cx| {
            let view = cx.update(|window, _| {
                match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
                    Ok(RawWindowHandle::AppKit(handle)) => Some(handle.ns_view),
                    _ => None,
                }
            });
            if let Ok(Some(view)) = view
                && let Err(error) = mac_move(view, bounds)
            {
                tracing::warn!(target: "ducktape::app", reason = "set_bounds_failed", %error, "the window was not moved");
            }
        })
        .detach();
}

#[cfg(target_os = "macos")]
fn mac_move(
    ns_view: std::ptr::NonNull<std::ffi::c_void>,
    bounds: Bounds<Pixels>,
) -> Result<(), String> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let ns_window = mac_window(ns_view)?;
    unsafe {
        let screen: *mut AnyObject = msg_send![ns_window, screen];
        if screen.is_null() {
            return Err("the window is on no screen".to_owned());
        }
        let on: NSRect = msg_send![screen, frame];
        let (x, y) = appkit_origin(bounds, (on.origin.x, on.origin.y), on.size.height);
        let extent = NSSize::new(
            f64::from(f32::from(bounds.size.width)),
            f64::from(f32::from(bounds.size.height)),
        );
        let frame = NSRect::new(NSPoint::new(x, y), extent);
        let _: () = msg_send![ns_window, setFrame: frame, display: true];
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn appkit_origin(bounds: Bounds<Pixels>, screen: (f64, f64), height: f64) -> (f64, f64) {
    let x = f64::from(f32::from(bounds.origin.x));
    let y = f64::from(f32::from(bounds.origin.y));
    let tall = f64::from(f32::from(bounds.size.height));
    (screen.0 + x, height + screen.1 - y - tall)
}

#[cfg(target_os = "macos")]
fn mac_double_click_minimizes() -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    let domain = NSString::from_str("NSGlobalDomain");
    let key = NSString::from_str("AppleActionOnDoubleClick");
    let minimize = NSString::from_str("Minimize");
    unsafe {
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let global: *mut AnyObject = msg_send![defaults, persistentDomainForName: &*domain];
        if global.is_null() {
            return false;
        }
        let action: *mut AnyObject = msg_send![global, objectForKey: &*key];
        if action.is_null() {
            return false;
        }
        msg_send![action, isEqual: &*minimize]
    }
}

#[cfg(target_os = "linux")]
type SizeRange = (Option<(i32, i32)>, Option<(i32, i32)>);

#[cfg(target_os = "linux")]
thread_local! {
    static UNFIXED: std::cell::RefCell<std::collections::BTreeMap<gpui_kit::WindowId, SizeRange>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

#[cfg(target_os = "linux")]
fn said(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(target_os = "linux")]
fn held(
    mut hints: x11rb::properties::WmSizeHints,
    (min, max): SizeRange,
) -> x11rb::properties::WmSizeHints {
    hints.min_size = min;
    hints.max_size = max;
    hints
}

#[cfg(target_os = "linux")]
fn x11_hints(
    xcb: &x11rb::xcb_ffi::XCBConnection,
    id: u32,
) -> Result<x11rb::properties::WmSizeHints, String> {
    x11rb::properties::WmSizeHints::get_normal_hints(xcb, id)
        .map_err(said)?
        .reply()
        .map(Option::unwrap_or_default)
        .map_err(said)
}

#[cfg(target_os = "linux")]
fn x11_write_hints(
    xcb: &x11rb::xcb_ffi::XCBConnection,
    id: u32,
    hints: x11rb::properties::WmSizeHints,
) -> Result<(), String> {
    hints
        .set_normal_hints(xcb, id)
        .map_err(said)?
        .check()
        .map_err(said)
}

#[cfg(target_os = "linux")]
fn x11_set_resizable(window: &gpui_kit::Window, id: u32, resizable: bool) -> Result<(), String> {
    let key = window.window_handle().window_id();
    let was = UNFIXED.with_borrow(|unfixed| unfixed.get(&key).copied());
    if resizable && was.is_none() {
        return Ok(());
    }
    let (xcb, _) = x11_connection(window)?;
    let hints = x11_hints(&xcb, id)?;
    let range = match (resizable, was) {
        (true, Some(was)) => was,
        _ => {
            let size = window.bounds().size.to_device_pixels(window.scale_factor());
            let size = Some((size.width.0, size.height.0));
            (size, size)
        }
    };
    x11_write_hints(&xcb, id, held(hints, range))?;
    UNFIXED.with_borrow_mut(|unfixed| {
        if resizable {
            unfixed.remove(&key);
        } else {
            unfixed
                .entry(key)
                .or_insert((hints.min_size, hints.max_size));
        }
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn x11_set_bounds(
    window: &gpui_kit::Window,
    id: u32,
    bounds: Bounds<Pixels>,
) -> Result<(), String> {
    use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _};
    let (xcb, _) = x11_connection(window)?;
    let at = bounds.to_device_pixels(window.scale_factor());
    let size = (at.size.width.0, at.size.height.0);
    let key = window.window_handle().window_id();
    if UNFIXED.with_borrow(|unfixed| unfixed.contains_key(&key)) {
        x11_write_hints(
            &xcb,
            id,
            held(x11_hints(&xcb, id)?, (Some(size), Some(size))),
        )?;
    }
    let width = u32::try_from(size.0).map_err(said)?;
    let height = u32::try_from(size.1).map_err(said)?;
    let to = ConfigureWindowAux::new()
        .x(at.origin.x.0)
        .y(at.origin.y.0)
        .width(width)
        .height(height);
    xcb.configure_window(id, &to)
        .map_err(said)?
        .check()
        .map_err(said)
}

#[cfg(target_os = "linux")]
fn x11_connection(
    window: &gpui_kit::Window,
) -> Result<(x11rb::xcb_ffi::XCBConnection, u32), String> {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use x11rb::connection::Connection as _;
    let display = HasDisplayHandle::display_handle(window).map_err(said)?;
    let RawDisplayHandle::Xcb(display) = display.as_raw() else {
        return Err("not an XCB display".to_owned());
    };
    let connection = display.connection.ok_or("no XCB connection")?;
    let xcb = unsafe {
        x11rb::xcb_ffi::XCBConnection::from_raw_xcb_connection(connection.as_ptr(), false)
    }
    .map_err(said)?;
    let root = usize::try_from(display.screen)
        .ok()
        .and_then(|screen| xcb.setup().roots.get(screen))
        .ok_or("no such screen")?
        .root;
    Ok((xcb, root))
}

#[cfg(target_os = "linux")]
x11rb::atom_manager! {
    /// The two atoms Keep on Top asks with.
    AboveAtoms: AboveAtomsCookie {
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
    }
}

/// X11: EWMH's `_NET_WM_STATE` request to the root window, on gpui's own
/// connection (so it follows whatever gpui sent before it, the window's
/// map included). The window manager carries it out.
#[cfg(target_os = "linux")]
fn x11_keep_above(window: &gpui_kit::Window, id: u32, above: bool) -> Result<(), String> {
    use x11rb::protocol::xproto::EventMask;
    x11_send(window, |xcb| {
        let said = |error: &dyn std::fmt::Display| error.to_string();
        let atoms = AboveAtoms::new(xcb)
            .map_err(|error| said(&error))?
            .reply()
            .map_err(|error| said(&error))?;
        let request = above_request(id, atoms._NET_WM_STATE, atoms._NET_WM_STATE_ABOVE, above);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        Ok(vec![(mask, request)])
    })
}

#[cfg(target_os = "linux")]
x11rb::atom_manager! {
    ActiveAtoms: ActiveAtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_STARTUP_INFO_BEGIN,
        _NET_STARTUP_INFO,
    }
}

#[cfg(target_os = "linux")]
#[cfg(target_os = "linux")]
fn wayland_activate(
    window: &gpui_kit::Window,
    surface: std::ptr::NonNull<std::ffi::c_void>,
    token: &str,
) -> Result<(), String> {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use wayland_backend::client::{Backend, ObjectId};
    use wayland_client::Proxy as _;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
    let said = |error: &dyn std::fmt::Display| error.to_string();
    let display = HasDisplayHandle::display_handle(window).map_err(|error| said(&error))?;
    let RawDisplayHandle::Wayland(display) = display.as_raw() else {
        return Err("not a Wayland display".to_owned());
    };
    let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
    let connection = wayland_client::Connection::from_backend(backend);
    let (globals, queue) = wayland_client::globals::registry_queue_init::<Activating>(&connection)
        .map_err(|error| said(&error))?;
    let activation: XdgActivationV1 = globals
        .bind(&queue.handle(), 1..=1, ())
        .map_err(|error| said(&error))?;
    let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
        .map_err(|error| said(&error))?;
    let surface = WlSurface::from_id(&connection, id).map_err(|error| said(&error))?;
    activation.activate(token.to_owned(), &surface);
    activation.destroy();
    connection.flush().map_err(|error| said(&error))
}

#[cfg(target_os = "linux")]
struct Activating;

#[cfg(target_os = "linux")]
impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        wayland_client::globals::GlobalListContents,
    > for Activating
{
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &wayland_client::Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

#[cfg(target_os = "linux")]
wayland_client::delegate_noop!(
    Activating: ignore wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1
);

fn x11_activate(window: &gpui_kit::Window, id: u32, token: Option<&str>) -> Result<(), String> {
    use x11rb::protocol::xproto::EventMask;
    x11_send(window, |xcb| {
        let said = |error: &dyn std::fmt::Display| error.to_string();
        let atoms = ActiveAtoms::new(xcb)
            .map_err(|error| said(&error))?
            .reply()
            .map_err(|error| said(&error))?;
        let time = token.and_then(startup_time).unwrap_or(x11rb::CURRENT_TIME);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let mut sent = vec![(mask, active_request(id, atoms._NET_ACTIVE_WINDOW, time))];
        let ended = token.into_iter().flat_map(startup_removed).enumerate();
        sent.extend(ended.map(|(at, chunk)| {
            let kind = match at {
                0 => atoms._NET_STARTUP_INFO_BEGIN,
                _ => atoms._NET_STARTUP_INFO,
            };
            let message = x11rb::protocol::xproto::ClientMessageEvent::new(8, id, kind, chunk);
            (EventMask::PROPERTY_CHANGE, message)
        }));
        Ok(sent)
    })
}

#[cfg(target_os = "linux")]
fn startup_time(id: &str) -> Option<u32> {
    id.rsplit_once("_TIME")?.1.parse().ok()
}

#[cfg(target_os = "linux")]
fn startup_removed(id: &str) -> Vec<[u8; 20]> {
    let quoted = id.replace('\\', "\\\\").replace('"', "\\\"");
    let mut message = format!("remove: ID=\"{quoted}\"").into_bytes();
    message.push(0);
    message
        .chunks(20)
        .map(|chunk| {
            let mut padded = [0; 20];
            padded[..chunk.len()].copy_from_slice(chunk);
            padded
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn active_request(
    id: u32,
    net_active_window: u32,
    time: u32,
) -> x11rb::protocol::xproto::ClientMessageEvent {
    const APPLICATION: u32 = 1;
    x11rb::protocol::xproto::ClientMessageEvent::new(
        32,
        id,
        net_active_window,
        [APPLICATION, time, 0, 0, 0],
    )
}

#[cfg(target_os = "linux")]
type Requests = Vec<(
    x11rb::protocol::xproto::EventMask,
    x11rb::protocol::xproto::ClientMessageEvent,
)>;

#[cfg(target_os = "linux")]
fn x11_send(
    window: &gpui_kit::Window,
    requests: impl FnOnce(&x11rb::xcb_ffi::XCBConnection) -> Result<Requests, String>,
) -> Result<(), String> {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let said = |error: &dyn std::fmt::Display| error.to_string();
    let (xcb, root) = x11_connection(window)?;
    for (mask, request) in requests(&xcb)? {
        // checked: a round trip, which also sends it
        xcb.send_event(false, root, mask, request)
            .map_err(|error| said(&error))?
            .check()
            .map_err(|error| said(&error))?;
    }
    Ok(())
}

/// EWMH's request that window `id` gain (`above`) or lose
/// `_NET_WM_STATE_ABOVE`, from an application.
#[cfg(target_os = "linux")]
fn above_request(
    id: u32,
    net_wm_state: u32,
    net_wm_state_above: u32,
    above: bool,
) -> x11rb::protocol::xproto::ClientMessageEvent {
    /// `_NET_WM_STATE_REMOVE`, `_NET_WM_STATE_ADD`.
    const REMOVE: u32 = 0;
    const ADD: u32 = 1;
    /// The source: a normal application (not a pager).
    const APPLICATION: u32 = 1;
    let action = if above { ADD } else { REMOVE };
    x11rb::protocol::xproto::ClientMessageEvent::new(
        32,
        id,
        net_wm_state,
        [action, net_wm_state_above, 0, APPLICATION, 0],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn a_window_leaving_the_desk_opens_where_it_sat_under_the_bar() {
        use crate::backend::Layout;
        use crate::shell::layers::chrome_inset;
        let bar = chrome_inset(Layout::MenuBar);
        let seat = layout::Frame {
            x: 100.,
            y: 50.,
            w: 900.,
            h: 600.,
        };
        let display = frame(0., 0., 2560., 1440.);
        let at = unseated(
            frame(200., 100., 1280., 800.),
            bar,
            Some(seat),
            Some(display),
            POPOUT_MIN,
        );
        assert_eq!(at, frame(300., 100. + bar.top + 50., 900., 600.));
        assert!(
            at.origin.y >= px(100. + bar.top),
            "the console's bar stays uncovered"
        );
        // the sidebar's layout: right of the column, at the window's top
        let side = chrome_inset(Layout::Sidebar);
        let at = unseated(
            frame(200., 100., 1280., 800.),
            side,
            Some(seat),
            Some(display),
            POPOUT_MIN,
        );
        assert_eq!(at, frame(200. + side.left + 100., 150., 900., 600.));
        // no narrower than its view: a 500 px frame of a view laid out from 680
        let narrow = layout::Frame { w: 500., ..seat };
        let at = unseated(
            frame(200., 100., 1280., 800.),
            bar,
            Some(narrow),
            Some(display),
            680.,
        );
        assert_eq!(at.size.width, px(680.));
    }

    #[test]
    fn a_popout_is_never_narrower_than_its_view_nor_480() {
        assert_eq!(
            popout_min("popout-unseated-view"),
            POPOUT_MIN,
            "not drawn yet"
        );
        crate::runtime::seat_for_test("popout-wide-view", 640);
        assert_eq!(popout_min("popout-wide-view"), 640.);
        crate::runtime::seat_for_test("popout-narrow-view", 320);
        assert_eq!(popout_min("popout-narrow-view"), POPOUT_MIN);
    }

    /// A pop-out steps 32 px off its source window, unless that would
    /// take it off the display: then it stays where the source is.
    #[test]
    fn a_popout_cascades_off_its_source_and_stays_on_the_display() {
        let display = frame(0., 0., 1600., 1000.);
        let small = frame(0., 0., 1024., 700.);
        for (source, on, at) in [
            (frame(160., 100., 1280., 800.), display, (192., 132.)),
            (frame(320., 200., 1280., 800.), display, (320., 200.)),
            (frame(0., 0., 1024., 700.), small, (0., 0.)),
        ] {
            assert_eq!(
                cascade(source, Some(on)).origin,
                point(px(at.0), px(at.1)),
                "{source:?} on {on:?}"
            );
        }
    }

    /// Keep on Top on X11 is EWMH's state request to the window manager:
    /// add, then remove, `_NET_WM_STATE_ABOVE` on the window asked about,
    /// as an application.
    #[cfg(target_os = "linux")]
    #[test]
    fn keep_on_top_asks_the_x11_window_manager_for_above() {
        let (window, state, above) = (0x0420_0007, 301, 302);
        let on = above_request(window, state, above, true);
        assert_eq!((on.format, on.window, on.type_), (32, window, state));
        assert_eq!(on.data.as_data32(), [1, above, 0, 1, 0]);
        let off = above_request(window, state, above, false);
        assert_eq!(off.data.as_data32(), [0, above, 0, 1, 0]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_handed_over_launch_activates_at_its_startup_time_and_ends_its_startup() {
        let active = active_request(
            0x0420_0007,
            303,
            startup_time("gnome-7-x_TIME4242").unwrap(),
        );
        assert_eq!(active.data.as_data32(), [1, 4242, 0, 0, 0]);
        assert_eq!(startup_time("no-time-here"), None);
        let chunks = startup_removed("a\"b");
        let joined: Vec<u8> = chunks.concat();
        let said = joined.split(|byte| *byte == 0).next().unwrap();
        assert_eq!(said, br#"remove: ID="a\"b""#);
        assert_eq!(chunks.len(), 1);
        let long = startup_removed(&"x".repeat(30));
        assert_eq!(long.len(), 3);
        assert_eq!(
            long.concat().iter().filter(|byte| **byte == 0).count(),
            60 - 43
        );
    }

    /// A view's first float sits in its display's bottom-right corner, 24
    /// in from the work area's edges (a second display, under a menu bar),
    /// and on a display too small for it at the work area's corner.
    #[test]
    fn a_float_opens_in_the_corner_of_its_display() {
        let float = size(px(320.), px(232.));
        let display = frame(1920., 25., 2560., 1415.);
        assert_eq!(
            corner(float, display),
            frame(
                1920. + 2560. - 320. - 24.,
                25. + 1415. - 232. - 24.,
                320.,
                232.
            )
        );
        let small = frame(0., 0., 300., 200.);
        assert_eq!(corner(float, small).origin, point(px(0.), px(0.)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_fixed_x11_window_keeps_where_and_how_it_was_placed() {
        use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
        use x11rb::protocol::xproto::Gravity;
        let mut opened = WmSizeHints::new();
        opened.position = Some((WmSizeHintsSpecification::ProgramSpecified, 300, 200));
        opened.win_gravity = Some(Gravity::STATIC);
        opened.min_size = Some((720, 480));
        opened.max_size = Some((16384, 16384));
        let fixed = held(opened, (Some((960, 640)), Some((960, 640))));
        assert_eq!(fixed.min_size, Some((960, 640)));
        assert_eq!(fixed.max_size, Some((960, 640)));
        assert!(matches!(
            fixed.position,
            Some((WmSizeHintsSpecification::ProgramSpecified, 300, 200))
        ));
        assert_eq!(fixed.win_gravity, Some(Gravity::STATIC));
        let again = held(fixed, (opened.min_size, opened.max_size));
        assert_eq!(again.min_size, Some((720, 480)));
        assert_eq!(again.max_size, Some((16384, 16384)));
    }

    #[test]
    fn a_frame_set_on_macos_is_the_frame_its_bounds_were_read_from() {
        let (screen, height) = ((1920., -200.), 1440.);
        let appkit = (2100., 150., 960., 640.);
        let read = frame(
            (appkit.0 - screen.0) as f32,
            (height - appkit.1 - appkit.3 + screen.1) as f32,
            appkit.2 as f32,
            appkit.3 as f32,
        );
        assert_eq!(appkit_origin(read, screen, height), (appkit.0, appkit.1));
    }

    #[test]
    fn the_launcher_comes_up_centred_on_its_display() {
        let launcher = size(px(960.), px(640.));
        // a second display to the right, under a 25px menu bar
        let display = frame(1920., 25., 2560., 1415.);
        assert_eq!(
            centered(launcher, display),
            frame(1920. + 800., 25. + 387.5, 960., 640.)
        );
        // too small to hold it: pinned to the corner, not pushed off it
        let small = frame(0., 0., 800., 600.);
        assert_eq!(centered(launcher, small).origin, point(px(0.), px(0.)));
    }
}
