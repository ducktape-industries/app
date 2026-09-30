//! OS windows: where a new one goes (`cascade`, `centered`, `unseated`),
//! how the `Desktop` is made, and how the model opens a window
//! (`Desktop::open_window`, whose root is a `layers::WindowRoot`).

use super::*;
use gpui_kit::{Bounds, Pixels, Size, point, px, size};

/// A window's size before the person resizes it.
pub(super) const WINDOW_SIZE: (f32, f32) = (1280., 800.);

/// How far a pop-out steps down and right of the window it left.
const CASCADE: f32 = 32.;

/// The narrowest and lowest a pop-out goes, whatever its view: its own
/// title strip and bar are not laid out under this.
pub(super) const POPOUT_MIN: f32 = 480.;

/// The narrowest a pop-out holding `module`'s view goes: the view's own
/// minimum once it is drawn, never under [`POPOUT_MIN`].
pub(super) fn popout_min(module: &str) -> f32 {
    crate::runtime::min_width(module).map_or(POPOUT_MIN, |min_width| min_width.max(POPOUT_MIN))
}

/// Where a pop-out opens: stepped off its source window so it does not
/// land exactly on top of it, and pulled back inside `display` when the
/// step would push it off an edge.
pub(super) fn cascade(source: Bounds<Pixels>, display: Option<Bounds<Pixels>>) -> Bounds<Pixels> {
    let extent = size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1));
    let mut origin = point(source.origin.x + px(CASCADE), source.origin.y + px(CASCADE));
    if let Some(display) = display {
        let right = display.origin.x + display.size.width - extent.width;
        let bottom = display.origin.y + display.size.height - extent.height;
        origin.x = origin.x.min(right).max(display.origin.x);
        origin.y = origin.y.min(bottom).max(display.origin.y);
    }
    Bounds::new(origin, extent)
}

/// Where the launcher sits when the desk shrinks to it: centred in
/// `display`, and never past its top-left corner when it doesn't fit.
pub(super) fn centered(extent: Size<Pixels>, display: Bounds<Pixels>) -> Bounds<Pixels> {
    let origin = point(
        display.origin.x + ((display.size.width - extent.width) / 2.).max(px(0.)),
        display.origin.y + ((display.size.height - extent.height) / 2.).max(px(0.)),
    );
    Bounds::new(origin, extent)
}

/// Where a window leaving the desk opens: on the screen right where it sat,
/// under the console's bar rather than over it, at least `min_w` wide
/// ([`popout_min`]), and inside `display`. With no place on the desk to
/// keep, it cascades.
pub(super) fn unseated(
    source: Bounds<Pixels>,
    frame: Option<layout::Frame>,
    display: Option<Bounds<Pixels>>,
    min_w: f32,
) -> Bounds<Pixels> {
    let Some(frame) = frame else {
        return cascade(source, display);
    };
    let extent = size(px(frame.w.max(min_w)), px(frame.h.max(POPOUT_MIN)));
    let mut origin = point(
        source.origin.x + px(frame.x),
        source.origin.y + px(layers::BAR + frame.y),
    );
    if let Some(display) = display {
        let right = display.origin.x + display.size.width - extent.width;
        let bottom = display.origin.y + display.size.height - extent.height;
        origin.x = origin.x.min(right).max(display.origin.x);
        origin.y = origin.y.min(bottom).max(display.origin.y);
    }
    Bounds::new(origin, extent)
}

impl Desktop {
    pub(super) fn new(
        state: Ducktape,
        tray: crate::tray::Tray,
        entities: entities::Entities,
        cx: &mut Context<Self>,
    ) -> Self {
        let seats = cx.new(|cx| entities::Seats::new(&entities, cx));
        let seat_intents = cx.subscribe(&seats, |desktop, _, (module, intent), cx| {
            desktop.dispatch(Message::ViewEvent(module, intent.clone()), cx)
        });
        // what the reducer still does for the session and the account
        let mut launcher = *entities.screen.read(cx).get() != Screen::Desk;
        let subscriptions = vec![
            cx.subscribe(
                &entities.session,
                |desktop, _, event: &entities::SessionEvent, cx| match event {
                    entities::SessionEvent::LeftNetwork => {
                        desktop.dispatch(Message::LeftNetwork, cx)
                    }
                    entities::SessionEvent::Toast(said) => {
                        desktop.dispatch(Message::ShowToast(said.clone()), cx)
                    }
                    // the console comes forward, or opens, as the tray's Open
                    entities::SessionEvent::Connected => desktop.dispatch(Message::TrayOpen, cx),
                },
            ),
            cx.subscribe(
                &entities.account,
                |desktop, _, event: &entities::AccountEvent, cx| match event {
                    entities::AccountEvent::Toast(said) => {
                        desktop.dispatch(Message::ShowToast(said.clone()), cx)
                    }
                    // a new account starts on Help, greeted
                    entities::AccountEvent::Welcome => {
                        desktop.state.open_help();
                        desktop.settle(Task::none(), false, false, cx);
                    }
                    entities::AccountEvent::Approved => desktop.approved(cx),
                },
            ),
            // the launcher and the desk are one window: crossing from one to
            // the other takes the other's size
            cx.observe(&entities.screen, move |desktop, screen, cx| {
                let now = *screen.read(cx).get() != Screen::Desk;
                if now == launcher {
                    return;
                }
                launcher = now;
                if !now {
                    crate::perf::mark("desk");
                }
                desktop.swap_console(cx);
            }),
            cx.observe(&entities.session, |desktop, _, cx| desktop.sync_tray(cx)),
            cx.observe(&entities.chain, |desktop, _, cx| desktop.sync_tray(cx)),
        ];
        Self {
            drawn: state.beat_face(),
            state,
            tray,
            windows: BTreeMap::new(),
            views: BTreeMap::new(),
            streams: HashMap::new(),
            seats,
            _seat_intents: seat_intents,
            entities,
            desk_bounds: None,
            _subscriptions: subscriptions,
        }
    }

    /// Opens the OS window for `key` (deferred: the caller is mid-update)
    /// and remembers its handle and view. On failure a pane on its way
    /// there goes back to the desk and the model hears the window closed.
    pub(super) fn open_window(
        &mut self,
        key: WindowKey,
        kind: WindowKind,
        reply: oneshot::Sender<WindowKey>,
        at: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::*;
        let title = match kind {
            crate::shell::WindowKind::Console => "Ducktape".to_owned(),
            crate::shell::WindowKind::View { module } => {
                panes::label(self.entities.rail.read(cx).rows(), module)
            }
        };
        // the launcher is a small window of a fixed size; the desk grows
        let launcher = kind == crate::shell::WindowKind::Console && self.in_launcher(cx);
        let extent = match kind {
            crate::shell::WindowKind::Console if launcher => {
                size(px(layers::LAUNCHER_SIZE.0), px(layers::LAUNCHER_SIZE.1))
            }
            crate::shell::WindowKind::Console => size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
            crate::shell::WindowKind::View { .. } => size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
        };
        let model = cx.entity();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(
                at.unwrap_or_else(|| Bounds::centered(None, extent, cx)),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: cfg!(target_os = "macos"),
                // the buttons are 14px tall: centred in the 36px bar
                traffic_light_position: Some(point(px(11.), px(11.))),
            }),
            // one window serves the launcher and the desk, so it resizes;
            // a pop-out goes down to its view's minimum, as it stands when
            // the window opens (there is no setting it later)
            window_min_size: Some(match kind {
                crate::shell::WindowKind::Console => gpui_kit::size(px(720.), px(480.)),
                crate::shell::WindowKind::View { module } => {
                    gpui_kit::size(px(popout_min(module)), px(POPOUT_MIN))
                }
            }),
            app_id: Some("dev.ducktape.app".into()),
            kind: gpui_kit::WindowKind::Normal,
            icon: image::RgbaImage::from_raw(
                128,
                128,
                include_bytes!("../../assets/icon.rgba").to_vec(),
            )
            .map(std::sync::Arc::new),
            ..Default::default()
        };
        cx.defer(move |cx| {
            let mut opened_view = None;
            let window_model = model.clone();
            let opened = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| WindowRoot::new(window_model, key, kind, window, cx));
                opened_view = Some(view.downgrade());
                window.on_window_should_close(cx, move |window, cx| {
                    release_window_input(window, cx);
                    true
                });
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });
            match opened {
                Ok(handle) => {
                    crate::perf::mark("window");
                    model.update(cx, |model, cx| {
                        model.windows.insert(key, handle.into());
                        if let Some(view) = opened_view {
                            model.views.insert(key, view);
                        }
                        // the seat of a popped-out pane moves in now, not at
                        // the next dispatch; s11 moves this
                        model.reconcile_seats(cx);
                    });
                    let _ = reply.send(key);
                }
                Err(error) => {
                    tracing::error!(target: "ducktape::app", reason = "native_window_open_failed", %error, "window could not be opened");
                    model.update(cx, |model, cx| {
                        let error = format!("The window could not be opened: {error}");
                        model
                            .entities
                            .session
                            .update(cx, |session, cx| session.note_error(error, cx));
                        // a pane on its way to this window goes back to the desk
                        model.dispatch(Message::Pane(key, PaneMessage::PopIn), cx);
                        model.dispatch(Message::WindowWasClosed(key), cx);
                        cx.notify();
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn a_window_leaving_the_desk_opens_where_it_sat_under_the_bar() {
        let seat = layout::Frame {
            x: 100.,
            y: 50.,
            w: 900.,
            h: 600.,
        };
        let display = frame(0., 0., 2560., 1440.);
        let at = unseated(
            frame(200., 100., 1280., 800.),
            Some(seat),
            Some(display),
            POPOUT_MIN,
        );
        assert_eq!(at, frame(300., 100. + layers::BAR + 50., 900., 600.));
        assert!(
            at.origin.y >= px(100. + layers::BAR),
            "the console's bar stays uncovered"
        );
        // no narrower than its view: a 500 px frame of a view laid out from 680
        let narrow = layout::Frame { w: 500., ..seat };
        let at = unseated(
            frame(200., 100., 1280., 800.),
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

    #[test]
    fn a_popout_steps_off_its_source_window() {
        let display = frame(0., 0., 1600., 1000.);
        let at = cascade(frame(160., 100., 1280., 800.), Some(display));
        assert_eq!(at, frame(192., 132., 1280., 800.));
    }

    #[test]
    fn a_popout_stays_on_the_display() {
        let display = frame(0., 0., 1600., 1000.);
        let at = cascade(frame(320., 200., 1280., 800.), Some(display));
        assert_eq!(at, frame(320., 200., 1280., 800.));
        let small = frame(0., 0., 1024., 700.);
        assert_eq!(
            cascade(frame(0., 0., 1024., 700.), Some(small)).origin,
            point(px(0.), px(0.))
        );
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
