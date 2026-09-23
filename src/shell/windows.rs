use super::*;
use gpui_kit::{Bounds, Pixels, point, px, size};

/// A window's size before the person resizes it.
const WINDOW_SIZE: (f32, f32) = (1280., 800.);

/// How far a pop-out steps down and right of the window it left.
const CASCADE: f32 = 32.;

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

impl Desktop {
    pub(super) fn open_window(
        &mut self,
        key: WindowKey,
        kind: WindowKind,
        reply: oneshot::Sender<WindowKey>,
        mut transferred: Option<(
            layout::Pane,
            panes::MountedPane,
            gpui_kit::WeakEntity<DesktopWindow>,
        )>,
        at: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::*;
        let title = match kind {
            crate::shell::WindowKind::Console => "Ducktape".to_owned(),
            crate::shell::WindowKind::View { module } => panes::label(module),
        };
        let extent = size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1));
        let model = cx.entity();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(
                at.unwrap_or_else(|| Bounds::centered(None, extent, cx)),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: Some(point(px(12.), px(12.))),
            }),
            window_min_size: Some(gpui_kit::size(px(720.), px(480.))),
            is_resizable: true,
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
                let view = cx.new(|cx| {
                    cx.on_release(DesktopWindow::released).detach();
                    let observer = cx.observe(&window_model, |_, _, cx| cx.notify());
                    let activation = cx.observe_window_activation(
                        window,
                        move |this: &mut DesktopWindow, window, cx| {
                            let message = match window.is_window_active() {
                                true => Message::WindowFocused(key),
                                false => Message::WindowUnfocused(key),
                            };
                            let model = this.model.clone();
                            cx.defer(move |cx| {
                                model.update(cx, |model, cx| model.dispatch(message, cx))
                            });
                        },
                    );
                    let focus = cx.focus_handle();
                    focus.focus(window, cx);
                    let keystrokes = DesktopWindow::intercept_global_keys(window, cx);
                    DesktopWindow {
                        model: window_model,
                        key,
                        kind,
                        layout: layout::Layout::default(),
                        mounted: BTreeMap::new(),
                        initialized: false,
                        resize: None,
                        measured_widths: Default::default(),
                        inputs: HashMap::new(),
                        focus,
                        _activation: activation,
                        _observer: observer,
                        _keystrokes: keystrokes,
                        _focus_lost: cx
                            .on_focus_lost(window, |this, window, cx| this.focus_lost(window, cx)),
                    }
                });
                if let Some((pane, mounted, _)) = transferred.take() {
                    view.update(cx, |this, _| {
                        this.mounted.insert(pane.instance, mounted);
                        this.layout.popin(pane);
                        this.initialized = true;
                    });
                }
                opened_view = Some(view.downgrade());
                let closing = view.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    let _ = closing.update(cx, |this, cx| {
                        this.observe_window(view_wire::events::Window::CloseRequested, cx)
                    });
                    release_window_input(window, cx);
                    true
                });
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });
            match opened {
                Ok(handle) => {
                    model.update(cx, |model, _| {
                        model.windows.insert(key, handle.into());
                        if let Some(view) = opened_view {
                            model.views.insert(key, view);
                        }
                    });
                    let _ = reply.send(key);
                }
                Err(error) => {
                    if let Some((pane, mounted, source)) = transferred.take() {
                        let _ = source.update(cx, |source, cx| {
                            source.mounted.insert(pane.instance, mounted);
                            source.layout.popin(pane);
                            cx.notify();
                        });
                    }
                    tracing::error!(target: "ducktape::app", reason = "native_window_open_failed", %error, "window could not be opened");
                    model.update(cx, |model, cx| {
                        model.state.error = format!("The window could not be opened: {error}");
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
}
