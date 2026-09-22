use super::*;

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
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::*;
        let title = match kind {
            crate::shell::WindowKind::Console => "Ducktape".to_owned(),
            crate::shell::WindowKind::View { module } => panes::label(module),
        };
        let size = size(px(1280.0), px(800.0));
        let model = cx.entity();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
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
                        _focus_lost: cx.on_focus_lost(window, |_, window, cx| window.blur(cx)),
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
