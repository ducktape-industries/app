use super::*;

// ---------- launch ----------

pub(crate) fn run() {
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    let (url_sender, mut urls) = mpsc::unbounded::<Vec<String>>();
    application.on_open_urls(move |urls| {
        let _ = url_sender.unbounded_send(urls);
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        initialize_rendering(cx);
        let mut commands = commands();
        let (state, initial) = Ducktape::boot();
        let (mut tray, mut tray_events) = crate::tray::init(cx);
        tray.sync(&state);
        let desktop = cx.new(|_| Desktop {
            state,
            tray,
            windows: BTreeMap::new(),
            views: BTreeMap::new(),
            streams: HashMap::new(),
            desk_bounds: None,
        });
        desktop.update(cx, |desktop, cx| desktop.sync_appearance(cx));
        let url_desktop = desktop.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(urls) = urls.next().await {
                let result = url_desktop.update(cx, |desktop, cx| {
                    for url in urls {
                        desktop.dispatch(Message::OpenLink(url), cx);
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
        let tray_desktop = desktop.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(row) = tray_events.next().await {
                let Some(message) = crate::tray::message(row) else {
                    continue;
                };
                if tray_desktop
                    .update(cx, |desktop, cx| desktop.dispatch(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let weak = desktop.downgrade();
        cx.on_window_closed(move |cx, id| {
            let weak = weak.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |desktop, cx| {
                    let key = desktop
                        .windows
                        .iter()
                        .find_map(|(key, handle)| (handle.window_id() == id).then_some(*key));
                    let Some(key) = key else {
                        return;
                    };
                    desktop.windows.remove(&key);
                    desktop.views.remove(&key);
                    desktop.dispatch(Message::WindowWasClosed(key), cx);
                });
            });
        })
        .detach();
        desktop.update(cx, |desktop, cx| {
            // the first window is the console; it draws the connect screen
            // until a node answers
            let (key, opened) = open(WindowKind::Console);
            desktop.state.console_win = Some(key);
            desktop
                .start(opened.map(Message::ConsoleOpened), cx)
                .detach();
            desktop.start(initial, cx).detach();
            desktop.subscriptions(cx);
        });
        if let Some(calls) = crate::ax::open() {
            let door_desktop = desktop.downgrade();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let windows = move |cx: &gpui_kit::App| {
                    door_desktop
                        .upgrade()
                        .map(|desktop| desktop.read(cx).ax_windows(cx))
                        .unwrap_or_default()
                };
                crate::ax::serve(calls, windows, cx).await;
            })
            .detach();
        }
        let command_desktop = desktop.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(pending) = commands.next().await {
                let _ =
                    command_desktop.update(cx, |desktop, cx| desktop.execute(pending.command, cx));
                let _ = pending.completed.send(());
            }
        })
        .detach();
        let mut desktop = Some(desktop);
        cx.on_app_quit(move |_| {
            drop(desktop.take());
            async {}
        })
        .detach();
    });
}

pub(super) fn initialize_rendering(cx: &mut gpui_kit::App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = BUNDLED_FACES
        .iter()
        .copied()
        .map(std::borrow::Cow::Borrowed)
        .collect();
    // CoreGraphics cannot load Noto's CBDT color font; macOS supplies emoji.
    #[cfg(not(target_os = "macos"))]
    let fonts = {
        let mut fonts = fonts;
        fonts.push(std::borrow::Cow::Borrowed(EMOJI_FACE));
        fonts
    };
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        tracing::error!(target: "ducktape::app", reason = "font_registration_failed", %error, "bundled desktop fonts could not be registered");
    }
    configure_native_theme(cx);
}
