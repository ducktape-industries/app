//! Settings: the app's own, in a dialog over the desk (the Settings
//! board). The account's settings are the network's, in its program;
//! these are this device's.

use super::*;
use crate::SettingsPage;
use ink::{Ink, mono, sans};

impl DesktopWindow {
    /// The dialog: `760 × 540; border: 1.5px solid ink`, a 34px title strip
    /// with its close, on a scrim below the bar. A click on the scrim, or
    /// Escape, closes it.
    pub(super) fn settings(&mut self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        // the Settings board: nav `padding: 12px 8px; gap: 2px`, each
        // entry `padding: 8px 14px; font: 400 14px`; the page `24px 32px`
        let ink = Ink::of(state.dark);
        let nav = [
            (
                "settings/appearance",
                "Appearance",
                SettingsPage::Appearance,
            ),
            (
                "settings/notifications",
                "Notifications",
                SettingsPage::Notifications,
            ),
            ("settings/networks", "Networks", SettingsPage::Networks),
            ("settings/about", "About", SettingsPage::About),
        ]
        .map(|(id, label, page)| {
            let model = self.model.clone();
            let on = state.settings_page == page;
            crate::a11y::keyboard(
                sans(400, 14.)
                    .id(id)
                    .control(Role::Tab, label)
                    .aria_selected(on)
                    .px(px(14.))
                    .py(px(8.))
                    .cursor_pointer()
                    .text_color(match on {
                        true => ink.ink,
                        false => ink.muted,
                    })
                    .when(on, |row| row.bg(ink.surface))
                    .on_click(move |_, _, cx| {
                        model.update(cx, |model, cx| {
                            model.dispatch(Message::ShowSettingsPage(page), cx)
                        })
                    })
                    .child(label),
            )
        });
        let page = match state.settings_page {
            SettingsPage::Appearance => self.appearance_page(state),
            SettingsPage::Notifications => self.notifications_page(state),
            SettingsPage::Networks => self.networks_page(state),
            SettingsPage::About => about_page(&ink),
        };
        let body = div()
            .id("settings-body")
            .flex_1()
            .min_h_0()
            .flex()
            .text_size(px(14.))
            .child(
                div()
                    .id("settings-nav")
                    .role(Role::TabList)
                    .w(px(180.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(8.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .border_r_1()
                    .border_color(ink.line)
                    .children(nav),
            )
            .child(
                div()
                    .id("settings-page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px(px(32.))
                    .py(px(24.))
                    .child(page),
            );
        let shut = self.model.clone();
        let surface = ink.surface;
        let title = div()
            .h(px(34.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(14.))
            .pr(px(6.))
            .border_b_1()
            .border_color(ink.line)
            .child(sans(500, 13.).child("Settings"))
            .child(crate::a11y::keyboard(
                div()
                    .id("settings-close")
                    .control(Role::Button, "Close settings")
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(ink.muted)
                    .hover(move |style| style.bg(surface))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        shut.update(cx, |model, cx| model.dispatch(Message::CloseSettings, cx))
                    })
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::X).size(px(16.)),
                    ),
            ));
        self.overlay(
            "settings-window",
            Role::Dialog,
            "Settings",
            || Message::CloseSettings,
            true,
            &ink,
            |card| {
                card.mt(px(74.))
                    .w(px(760.))
                    .h(px(540.))
                    .max_w_full()
                    .max_h_full()
                    .self_start()
                    .child(title)
                    .child(body)
            },
        )
    }

    fn appearance_page(&self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use crate::Appearance;
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let (fg, bg, line) = (ink.ink, ink.bg, ink.strong);
        let theme = div()
            .id("theme")
            .role(Role::RadioGroup)
            .aria_label("Theme")
            .flex()
            .border_1()
            .border_color(line)
            .children(
                [
                    ("Light", Appearance::Light),
                    ("Dark", Appearance::Dark),
                    ("System", Appearance::System),
                ]
                .into_iter()
                .enumerate()
                .map(|(nth, (label, mode))| {
                    let model = self.model.clone();
                    let on = state.appearance == mode;
                    crate::a11y::keyboard(
                        sans(400, 13.)
                            .id(SharedString::from(format!("theme/{label}")))
                            .control(Role::RadioButton, label)
                            .aria_toggled(match on {
                                true => gpui_kit::accesskit::Toggled::True,
                                false => gpui_kit::accesskit::Toggled::False,
                            })
                            .h(px(32.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .when(nth > 0, |cell| cell.border_l_1().border_color(line))
                            .when(on, |cell| cell.bg(fg).text_color(bg))
                            .on_click(move |_, _, cx| {
                                model.update(cx, |model, cx| {
                                    model.dispatch(Message::SetAppearance(mode), cx)
                                })
                            })
                            .child(label),
                    )
                }),
            );
        let motion = {
            let model = self.model.clone();
            let on = state.motion;
            crate::a11y::keyboard(
                div()
                    .id("motion")
                    .control(Role::CheckBox, "Moving figures")
                    .aria_toggled(match on {
                        true => gpui_kit::accesskit::Toggled::True,
                        false => gpui_kit::accesskit::Toggled::False,
                    })
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border(px(1.5))
                    .border_color(fg)
                    .when(on, |check| {
                        check.bg(fg).child(
                            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Check)
                                .size(px(12.))
                                .text_color(bg),
                        )
                    })
                    .on_click(move |_, _, cx| {
                        model.update(cx, |model, cx| model.dispatch(Message::SetMotion(!on), cx))
                    }),
            )
        };
        div()
            .flex()
            .flex_col()
            .child(heading("Appearance", &ink))
            .child(setting(
                "Theme",
                "Follows the system unless you pick one.",
                theme,
                &ink,
            ))
            .child(setting(
                "Moving figures",
                "The drawings in characters turn slowly, and the node's dot breathes. Off keeps them still.",
                motion,
                &ink,
            ))
            .into_any_element()
    }

    /// The NotifSettings board: the device's say over banners, then each
    /// view's.
    fn notifications_page(&self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use crate::runtime::notify::{self, Permission};
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let settings = notify::Settings::load();
        let banners = {
            let model = self.model.clone();
            let on = settings.banners;
            crate::a11y::keyboard(
                div()
                    .id("notify/banners")
                    .control(Role::Switch, "Desktop banners")
                    .aria_toggled(match on {
                        true => gpui_kit::accesskit::Toggled::True,
                        false => gpui_kit::accesskit::Toggled::False,
                    })
                    .w(px(32.))
                    .h(px(18.))
                    .p(px(2.))
                    .flex()
                    .when(on, |switch| switch.justify_end())
                    .cursor_pointer()
                    .border(px(1.5))
                    .border_color(ink.ink)
                    .when(on, |switch| switch.bg(ink.ink))
                    .on_click(move |_, _, cx| {
                        model.update(cx, |model, cx| {
                            model.dispatch(Message::SetNotifyBanners(!on), cx)
                        })
                    })
                    .child(div().size(px(11.)).bg(match on {
                        true => ink.bg,
                        false => ink.ink,
                    })),
            )
        };
        let front = self.segmented(
            "notify/front",
            "While Ducktape is in front",
            [("Show", true), ("Hide", false)].map(|(label, show)| {
                (label.into(), settings.in_front == show, move || {
                    Message::SetNotifyInFront(show)
                })
            }),
            &ink,
        );
        let burst = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .child(self.segmented(
                "notify/burst",
                "Burst limit",
                notify::BURSTS.map(|burst| {
                    (
                        burst.to_string().into(),
                        settings.burst == burst,
                        move || Message::SetNotifyBurst(burst),
                    )
                }),
                &ink,
            ))
            .child(mono(400, 12.).text_color(ink.muted).child("a minute"));
        let now = notify::wall();
        let views = crate::runtime::rail()
            .into_iter()
            .filter(|row| !row.empty)
            .map(|row| {
                let module = row.module;
                let name = super::desk::tab_label(&row);
                let chosen = settings.views.get(module).copied();
                let week = notify::center().this_week(module, now);
                let hint = match (chosen, week) {
                    (None, 0) => "Has not asked".to_owned(),
                    (_, week) => format!("{week} this week"),
                };
                let control = self.segmented(
                    &format!("notify/view/{module}"),
                    &format!("{name} notifications"),
                    Permission::ALL.map(|permission| {
                        (
                            permission.word().into(),
                            chosen == Some(permission),
                            move || Message::NotifyPermission(module, permission),
                        )
                    }),
                    &ink,
                );
                setting(name, hint, control, &ink)
            });
        div()
            .flex()
            .flex_col()
            .child(sans(400, 22.).child("Notifications"))
            .child(
                ink::note(
                    "On this device. Views ask; Ducktape decides what reaches the screen.",
                    ink.muted,
                )
                .pt(px(4.))
                .pb(px(12.))
                .border_b_1()
                .border_color(ink.line),
            )
            .child(setting(
                "Desktop banners",
                "Off keeps everything in Notifications, silently.",
                banners,
                &ink,
            ))
            .child(setting(
                "While Ducktape is in front",
                "Banners for a window you are looking at never show.",
                front,
                &ink,
            ))
            .child(setting(
                "Burst limit",
                "Per view. Past it, one banner says how many more are waiting.",
                burst,
                &ink,
            ))
            .child(
                mono(400, 12.)
                    .text_color(ink.muted)
                    .pt(px(24.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(ink.line)
                    .child("Views"),
            )
            .children(views)
            .into_any_element()
    }

    /// A row of choices, the chosen one on `surface` (the NotifSettings
    /// board's segmented controls).
    fn segmented<const N: usize>(
        &self,
        id: &str,
        name: &str,
        choices: [(gpui_kit::SharedString, bool, impl Fn() -> Message + 'static); N],
        ink: &Ink,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::*;
        let line = ink.strong;
        div()
            .id(SharedString::from(id.to_owned()))
            .role(Role::RadioGroup)
            .aria_label(SharedString::from(name.to_owned()))
            .flex()
            .border_1()
            .border_color(line)
            .children(
                choices
                    .into_iter()
                    .enumerate()
                    .map(|(nth, (label, on, message))| {
                        let model = self.model.clone();
                        crate::a11y::keyboard(
                            sans(if on { 500 } else { 400 }, 13.)
                                .id(SharedString::from(format!("{id}/{label}")))
                                .control(Role::RadioButton, label.clone())
                                .aria_toggled(match on {
                                    true => gpui_kit::accesskit::Toggled::True,
                                    false => gpui_kit::accesskit::Toggled::False,
                                })
                                .h(px(26.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .text_color(match on {
                                    true => ink.ink,
                                    false => ink.muted,
                                })
                                .when(nth > 0, |cell| cell.border_l_1().border_color(line))
                                .when(on, |cell| cell.bg(ink.surface))
                                .on_click(move |_, _, cx| {
                                    model.update(cx, |model, cx| model.dispatch(message(), cx))
                                })
                                .child(label),
                        )
                    }),
            )
    }

    fn networks_page(&self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let (muted, danger) = (ink.muted, ink.danger);
        let rows =
            state.recent_endpoints.iter().map(|entry| {
                let model = self.model.clone();
                let url = entry.url.clone();
                let current = entry.url == state.connected_rpc;
                let name = entry.name();
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(12.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(ink.line)
                    .child(sans(500, 15.).w(px(110.)).truncate().child(name))
                    .child(
                        mono(400, 13.)
                            .text_color(muted)
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(entry.host().to_owned()),
                    )
                    .child(mono(400, 12.).text_color(muted).child(
                        match (current, entry.other_chain) {
                            (true, _) => "Current",
                            (false, true) => "Other chain",
                            (false, false) => "",
                        },
                    ))
                    .child(crate::a11y::keyboard(
                        sans(400, 14.)
                            .id(SharedString::from(format!("settings/forget/{url}")))
                            .control(Role::Button, SharedString::from(format!("Forget {url}")))
                            .underline()
                            .cursor_pointer()
                            .text_color(muted)
                            .hover(move |style| style.text_color(danger))
                            .on_click(move |_, _, cx| {
                                let url = url.clone();
                                model.update(cx, |model, cx| {
                                    model.dispatch(Message::ForgetEndpoint(url), cx)
                                })
                            })
                            .child("Forget"),
                    ))
            });
        div()
            .flex()
            .flex_col()
            .child(heading("Networks", &ink))
            .child(
                ink::note("Nodes this device reached. Forgetting one takes it off the list; its key stays on this device.", muted)
                    .pb(px(12.)),
            )
            .children(rows)
            .into_any_element()
    }
}

fn heading(text: &'static str, ink: &Ink) -> gpui_kit::Div {
    use gpui_kit::*;
    mono(400, 12.).text_color(ink.muted).pb(px(6.)).child(text)
}

/// One row of a settings page: what it is, a line about it, its control
/// (`gap: 24px; padding: 16px 0`, a hairline under it).
fn setting(
    label: impl Into<gpui_kit::SharedString>,
    hint: impl Into<gpui_kit::SharedString>,
    control: impl gpui_kit::IntoElement,
    ink: &Ink,
) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .items_center()
        .gap(px(24.))
        .py(px(16.))
        .border_b_1()
        .border_color(ink.line)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(sans(500, 14.).child(label.into()))
                .child(
                    sans(400, 13.)
                        .line_height(px(13. * 1.5))
                        .text_color(ink.muted)
                        .child(hint.into()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}

fn about_page(ink: &Ink) -> gpui_kit::AnyElement {
    use gpui_kit::*;
    let log = crate::backend::app_log_path()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let muted = ink.muted;
    let line = |key: &'static str, value: String| {
        div()
            .flex()
            .gap(px(16.))
            .py(px(8.))
            .child(sans(400, 14.).w(px(120.)).text_color(muted).child(key))
            .child(
                mono(400, 13.)
                    .text_color(muted)
                    .flex_1()
                    .min_w_0()
                    .child(value),
            )
    };
    div()
        .flex()
        .flex_col()
        .child(heading("About", ink))
        .child(sans(400, 22.).pb(px(12.)).child("Ducktape"))
        .child(line("Version", env!("CARGO_PKG_VERSION").into()))
        .child(line(
            "Node contract",
            format!("v{}", crate::backend::noded::NODE_CONTRACT),
        ))
        .child(line("Log", log))
        .into_any_element()
}
