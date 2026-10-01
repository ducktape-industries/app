//! App start: the gpui application, the entities and the console window,
//! with every outside source of events wired to the entity it moves:
//! open-URL requests and the links a banner posts (`Windows::open_link`),
//! the tray (it follows `Session`, `Chain` and `Prefs`; its rows call
//! `Windows` and `Prefs`), a window closing (`Windows::forget_closed`), the
//! AX door (`Windows::served`, `Seats::settle`), quitting
//! (`Windows::quit`). Fonts and the theme are registered here too.

use super::*;
use futures::StreamExt as _;
use gpui_kit::AsyncApp;

pub(crate) fn run() {
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    let (url_sender, mut urls) = futures::channel::mpsc::unbounded::<Vec<String>>();
    application.on_open_urls(move |urls| {
        let _ = url_sender.unbounded_send(urls);
    });
    application.run(move |cx| {
        crate::perf::mark("gpui");
        gpui_kit::init(cx);
        keys::bind(cx);
        keys::menus(cx);
        initialize_rendering(cx);
        crate::perf::mark("fonts");
        let mut posted = entities::posted();
        // the app's two stores: the notification centre (its log read
        // off disk) and the roster the node's reads fill
        let center = crate::runtime::notify::center().clone();
        let roster = crate::runtime::roster().clone();
        crate::perf::mark("boot");
        let changes = crate::runtime::changes_channel();
        let entities = entities::Entities::new(roster, center, changes, cx);
        let (tray, mut tray_events) = crate::tray::init(&entities, cx);
        let windows = entities.windows.clone();
        windows.update(cx, |windows, cx| windows.sync_appearance(cx));
        let quitting = windows.downgrade();
        cx.on_action(move |_: &keys::Quit, cx| {
            let _ = quitting.update(cx, |windows, cx| windows.quit(cx));
        });
        let url_windows = windows.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(urls) = urls.next().await {
                let result = url_windows.update(cx, |windows, cx| {
                    for url in urls {
                        windows.open_link(&url, cx);
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
        let posted_windows = windows.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(posted) = posted.next().await {
                if posted_windows
                    .update(cx, |windows, cx| windows.posted(posted, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let (tray_windows, tray_prefs) = (windows.downgrade(), entities.prefs.downgrade());
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(row) = tray_events.next().await {
                let Some(pick) = crate::tray::pick(row) else {
                    continue;
                };
                let result = match pick {
                    crate::tray::Pick::Open => {
                        tray_windows.update(cx, |windows, cx| windows.raise_console(cx))
                    }
                    crate::tray::Pick::Appearance(mode) => {
                        tray_prefs.update(cx, |prefs, cx| prefs.set_appearance(mode, cx))
                    }
                    crate::tray::Pick::Quit => {
                        tray_windows.update(cx, |windows, cx| windows.quit(cx))
                    }
                };
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
        entities::Windows::forget_closed(&windows, cx);
        // the first window is the console; it draws the connect screen
        // until a node answers
        windows.update(cx, |windows, cx| {
            windows.open(WindowKind::Console, None, cx);
        });
        // the first thing to do: reach the node last used, if there was one
        if let Some(target) = entities::Session::boot_target() {
            entities
                .session
                .update(cx, |session, cx| session.connect(target, cx));
        }
        first_present(cx);
        if let Some(calls) = crate::ax::open() {
            let door_windows = windows.downgrade();
            let door_seats = entities.seats.downgrade();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let windows = move |cx: &gpui_kit::App| {
                    door_windows
                        .upgrade()
                        .map(|windows| windows.read(cx).served())
                        .unwrap_or_default()
                };
                let settle = move |cx: &mut gpui_kit::App| {
                    let _ = door_seats.update(cx, |seats, cx| seats.settle(cx));
                };
                let door = crate::ax::Door {
                    windows: &windows,
                    settle: &settle,
                };
                crate::ax::serve(calls, door, cx).await;
            })
            .detach();
        }
        // the only strong handles to the entities and the tray: they live
        // until the app quits
        let mut kept = Some((tray, entities));
        cx.on_app_quit(move |_| {
            drop(kept.take());
            // the one hook every quit path reaches
            crate::perf::summary();
            async {}
        })
        .detach();
    });
}

/// With `perf-deep` and perf on: gpui's first present as the
/// `first_present` mark, read off the foreground journal once a second
/// until it lands. A default build has no journal.
#[cfg(feature = "perf-deep")]
fn first_present(cx: &mut gpui_kit::App) {
    use std::time::Duration;
    if !crate::perf::on() {
        return;
    }
    // only its first-present latch is read; the thresholds are its hang
    // rules, which nothing here reports
    let mut detector = gpui_kit::hang::HangDetector::new(
        cx.foreground_journal(),
        Duration::from_millis(100),
        Duration::from_millis(16),
    );
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            detector.poll();
            if let Some(at) = detector.first_present_at() {
                crate::perf::mark_at("first_present", at);
                return;
            }
        }
    })
    .detach();
}

#[cfg(not(feature = "perf-deep"))]
fn first_present(_: &mut gpui_kit::App) {}

/// The bundled fonts into gpui's text system, then the product theme.
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
