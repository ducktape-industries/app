//! App start: the gpui application, the entities and the console window,
//! with every outside source of events wired to the entity it moves:
//! open-URL requests and the links a banner posts (`Windows::open_link`),
//! the tray (it follows `Session`, `Chain` and `Prefs`; its rows call
//! `Windows` and `Prefs`), a window closing (`Windows::forget_closed`), the
//! AX door (`Windows::served`, `Seats::settle`), quitting
//! (`Windows::quit`). Fonts and the theme are registered here too.

use super::*;
use ducktape_host::contract::OsServices;
use futures::StreamExt as _;
use gpui_kit::{AppContext as _, AsyncApp};
use std::sync::Arc;

/// What the first window is: the console (the launcher, then the desk),
/// or, in a debug build's `--live`, one view on a node already named.
enum Boot {
    Console,
    #[cfg(debug_assertions)]
    Live(super::Live),
}

pub(crate) fn run(os: Arc<dyn OsServices>, profile: Option<crate::profile::Profile>) {
    start(Boot::Console, os, profile)
}

/// `ducktape-app --live`: the app as [`run`] starts it, with the one
/// view's window in place of the console.
#[cfg(debug_assertions)]
pub(crate) fn run_live(
    live: super::Live,
    os: Arc<dyn OsServices>,
    profile: Option<crate::profile::Profile>,
) {
    start(Boot::Live(live), os, profile)
}

fn start(boot: Boot, os: Arc<dyn OsServices>, profile: Option<crate::profile::Profile>) {
    let application = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    let (url_sender, mut urls) = futures::channel::mpsc::unbounded::<Vec<String>>();
    application.on_open_urls(move |urls| {
        let _ = url_sender.unbounded_send(urls);
    });
    application.run(move |cx| {
        crate::perf::mark("gpui");
        gpui_kit::init(cx);
        keys::bind(cx);
        keys::menus(false, cx);
        initialize_rendering(cx);
        crate::perf::mark("fonts");
        let mut posted = entities::posted();
        // the host's stores and the shared entities; what the OS feeds in
        // after this goes through `inbound` (the desktop: banner clicks)
        let (host, inbound) = ducktape_host::start::start(
            ducktape_host::contract::Boot {
                os: os.clone(),
                // Cranelift, signal-based traps, compiled code kept across
                // launches under the cache and compiled in a child of this
                // binary
                runner: Arc::new(ducktape_host::runner::WasmtimeRunner::new(
                    ducktape_host::contract::RunnerConfig {
                        signals_based_traps: true,
                        cache_dir: Some(os.dirs().cache.join("view-code")),
                        // Linux: the running binary even once an update
                        // replaced or removed its file
                        compile_apart: match cfg!(target_os = "linux") {
                            true => Some("/proc/self/exe".into()),
                            false => std::env::current_exe().ok(),
                        },
                    },
                )),
                open: entities::open_banner,
            },
            cx,
        );
        crate::os::install_inbound(inbound);
        let mut consent = host.consent;
        let entities = entities::Entities::new(host.shared, cx);
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
                        windows.open_link(&url, None, cx);
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
        let (handed, mut launches) = futures::channel::mpsc::unbounded::<Vec<String>>();
        if let Some(profile) = profile {
            profile.serve(move |args| handed.unbounded_send(args).is_ok());
        }
        let launch_windows = windows.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(args) = launches.next().await {
                if launch_windows
                    .update(cx, |windows, cx| windows.launched(args, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // an ask's request gone: the card follows it off the console
        let consent_windows = windows.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while consent.next().await.is_some() {
                if consent_windows
                    .update(cx, |windows, cx| windows.sync_consent(cx))
                    .is_err()
                {
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
        match boot {
            Boot::Console => {
                // the first window is the console; it draws the connect
                // screen until a node answers
                windows.update(cx, |windows, cx| {
                    windows.open(WindowKind::Console, None, None, cx);
                });
                // the first thing to do: reach the node last used, if there
                // was one
                if let Some(target) = entities::Session::boot_target() {
                    entities
                        .session
                        .update(cx, |session, cx| session.connect(target, cx));
                }
                // the other listed networks' notices, from the start: after
                // the boot connect is out, so its node is not followed
                let follower = cx.new(|cx| {
                    super::follow::Follower::new(&entities.session, &entities.account, cx)
                });
                windows.update(cx, |windows, cx| windows.notified_by(follower, cx));
            }
            #[cfg(debug_assertions)]
            Boot::Live(live) => super::live::boot(live, &entities, cx),
        }
        first_present(cx);
        #[cfg(feature = "ax-door")]
        if let Some((calls, written)) = crate::ax::open() {
            // the door file goes as the app quits
            let mut written = Some(written);
            cx.on_app_quit(move |_| {
                drop(written.take());
                async {}
            })
            .detach();
            let door_windows = windows.downgrade();
            let door_seats = entities.seats.downgrade();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let windows = move |cx: &gpui_kit::App| {
                    door_windows
                        .upgrade()
                        .map(|windows| windows.read(cx).served())
                        .unwrap_or_default()
                };
                let views_seats = door_seats.clone();
                let views = move |cx: &gpui_kit::App| {
                    views_seats
                        .upgrade()
                        .map(|seats| seats.read(cx).seated(cx))
                        .unwrap_or_default()
                };
                let settle = move |cx: &mut gpui_kit::App| {
                    door_seats
                        .update(cx, |seats, cx| seats.settle(cx))
                        .unwrap_or(true)
                };
                let keep = super::kept_restore;
                let door = crate::ax::Door {
                    windows: &windows,
                    settle: &settle,
                    keep: &keep,
                    chords: &chords,
                    views: &views,
                };
                crate::ax::serve(calls, door, cx).await;
            })
            .detach();
        }
        // the only strong handles to the entities and the tray: they live
        // until the app quits
        let mut kept = Some((tray, entities));
        cx.on_app_quit(move |_| {
            // the one hook every quit path reaches
            let drained = crate::runtime::quitting(crate::runtime::notify::center(), kept.take());
            crate::perf::summary();
            drained
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
