//! A launch that finds another copy holding the profile and can't hand over to it: one small
//! window that says so, with Quit, while the profile's lock is waited on. Once it frees, the
//! window goes and the launch goes on.

use super::ink::{self, Ink, Kind};
use crate::profile::{Busy, Profile};
use futures::channel::oneshot;
use gpui_kit::{AnyWindowHandle, App, AppContext as _, Context, IntoElement, Render, Window};

pub(super) fn wait(busy: Busy, cx: &mut App, then: impl FnOnce(Profile, &mut App) + 'static) {
    let reason = busy.reason().to_owned();
    let activation = busy.activation().map(str::to_owned);
    let (freed, free) = oneshot::channel();
    let waiting = std::thread::Builder::new()
        .name("profile-wait".into())
        .spawn(move || match busy.wait() {
            Ok(profile) => drop(freed.send(profile)),
            Err(error) => {
                tracing::warn!(target: "ducktape::app", event = "profile_wait_failed", %error, "the profile's lock was not waited on");
            }
        });
    if let Err(error) = waiting {
        tracing::warn!(target: "ducktape::app", event = "profile_wait_failed", %error, "the profile's lock was not waited on");
    }
    showing(reason, activation, free, then, cx);
}

fn showing(
    reason: String,
    activation: Option<String>,
    free: oneshot::Receiver<Profile>,
    then: impl FnOnce(Profile, &mut App) + 'static,
    cx: &mut App,
) {
    let window = match open(reason, cx) {
        Ok(window) => Some(window),
        Err(error) => {
            tracing::error!(target: "ducktape::app", reason = "busy_window_failed", %error, "waiting for the profile with no window");
            None
        }
    };
    if cfg!(target_os = "macos") {
        cx.activate(true);
    }
    if let Some(window) = window {
        cx.defer(move |cx| {
            let told = window.update(cx, |_, window, _| {
                super::windows::activate(window, activation.as_deref())
            });
            if let Ok(Err(error)) = told {
                tracing::warn!(target: "ducktape::app", reason = "activate_failed", %error, "the busy window was not brought to the front");
            }
        });
    }
    cx.spawn(async move |cx| {
        let Ok(profile) = free.await else {
            return;
        };
        cx.update(|cx| {
            then(profile, cx);
            cx.defer(move |cx| {
                if let Some(window) = window {
                    let _ = window.update(cx, |_, window, _| window.remove_window());
                }
            });
        });
    })
    .detach();
}

fn open(reason: String, cx: &mut App) -> Result<AnyWindowHandle, String> {
    use gpui_kit::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(440.), px(220.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Ducktape".into()),
            ..Default::default()
        }),
        is_resizable: false,
        app_id: Some("dev.ducktape.app".into()),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        window.on_window_should_close(cx, |_, cx| {
            cx.quit();
            true
        });
        if let Err(error) = super::windows::set_resizable(window, false, None) {
            tracing::warn!(target: "ducktape::app", reason = "set_resizable_failed", %error, "the busy window's size was not fixed");
        }
        let busy = cx.new(|_| BusyView { reason });
        cx.new(|cx| gpui_kit::component::Root::new(busy, window, cx))
    })
    .map(Into::into)
    .map_err(|error| error.to_string())
}

struct BusyView {
    reason: String,
}

impl Render for BusyView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::*;
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let ink = Ink::of(dark);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(24.))
            .bg(ink.bg)
            .text_color(ink.ink)
            .child(ink::sans(500, 17.).child("Another Ducktape is using this profile"))
            .child(ink::sans(400, 13.).text_color(ink.muted).child(format!(
                "This one opens when that one quits. It couldn't hand over to it ({}).",
                self.reason
            )))
            .child(div().flex_1())
            .child(div().flex().justify_end().child(ink::button(
                "busy/quit",
                "Quit",
                Kind::Primary,
                |cx: &mut App| cx.quit(),
                false,
                &ink,
            )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Claim, Launch, claim_at};
    use gpui_kit::TestAppContext;
    use std::cell::Cell;
    use std::rc::Rc;

    #[gpui_kit::test]
    fn a_busy_launch_shows_quit_and_goes_on_once_the_profile_frees(cx: &mut TestAppContext) {
        let (config, sockets) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let holder = std::fs::File::create(config.path().join("lock")).unwrap();
        holder.try_lock().unwrap();
        let launch = Launch {
            args: Vec::new(),
            activation: Some("busy-1_TIME7".into()),
        };
        let Claim::Busy(busy) = claim_at(config.path(), sockets.path(), &launch).unwrap() else {
            panic!("nothing took the second launch");
        };
        cx.update(gpui_kit::init);
        let (freed, free) = oneshot::channel();
        let went_on = Rc::new(Cell::new(false));
        let going = went_on.clone();
        cx.update(|cx| {
            let then = move |_: Profile, cx: &mut App| {
                going.set(true);
                let opened = cx.open_window(Default::default(), |_, cx| {
                    cx.new(|_| BusyView {
                        reason: "the app".into(),
                    })
                });
                opened.unwrap();
            };
            let activation = busy.activation().map(str::to_owned);
            showing(busy.reason().to_owned(), activation, free, then, cx)
        });
        cx.run_until_parked();
        let window = cx.update(|cx| cx.windows()[0]);
        let nodes = cx
            .update_window(window, |_, window, cx| {
                window.activate_a11y();
                ducktape_host::render::draw_frame(window, cx);
                window.simulate_next_frame(cx);
                ducktape_host::render::draw_frame(window, cx);
                serde_json::to_value(crate::ax::snapshot("shell", window, false)).unwrap()
            })
            .unwrap();
        let quit = nodes
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["role"] == "Button" && node["name"] == "Quit");
        assert!(quit, "no Quit button in {nodes}");
        assert!(!went_on.get(), "it went on while the profile was held");
        let activated = super::super::windows::ACTIVATED.with_borrow(|told| told.clone());
        assert_eq!(
            activated.last().map(|(_, told)| told.as_deref()),
            Some(Some("busy-1_TIME7")),
            "the busy window did not end its launch's startup"
        );
        let waiting = std::thread::spawn(move || busy.wait().unwrap());
        drop(holder);
        let profile = waiting.join().unwrap();
        assert!(freed.send(profile).is_ok());
        cx.run_until_parked();
        assert!(went_on.get(), "the launch did not go on");
        let left = cx.update(|cx| cx.windows());
        assert_eq!(
            left.len(),
            1,
            "the busy window stayed, or went before the app's"
        );
        assert_ne!(left[0], window, "the busy window stayed");
    }
}
