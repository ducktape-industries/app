//! Settings: the app's own, in a window of their own that floats over the
//! desk (its title bar says "Settings"). The account's settings are the
//! network's, in its program; these are this device's.

use super::*;
use crate::SettingsPage;
use ink::{Ink, mono, sans};

pub(super) const SETTINGS_SIZE: (f32, f32) = (760., 540.);

impl DesktopWindow {
    pub(super) fn settings(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        // the Settings board: nav `padding: 12px 8px; gap: 2px`, each
        // entry `padding: 8px 14px; font: 400 14px`; the page `24px 32px`
        let ink = Ink::of(state.dark);
        let nav = [
            (
                "settings/appearance",
                "Appearance",
                SettingsPage::Appearance,
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
            SettingsPage::Appearance => self.appearance_page(&state),
            SettingsPage::Networks => self.networks_page(&state),
            SettingsPage::About => about_page(&ink),
        };
        div()
            .id("settings-window")
            .size_full()
            .flex()
            .bg(ink.bg)
            .text_color(ink.ink)
            .font_family(super::theme::FAMILY_UI)
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
            )
            .into_any_element()
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

    fn networks_page(&self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = Ink::of(state.dark);
        let (muted, danger) = (ink.muted, ink.danger);
        let rows =
            state.recent_endpoints.iter().map(|entry| {
                let model = self.model.clone();
                let url = entry.url.clone();
                let current = entry.url == state.connected_rpc;
                let name = match entry.network.is_empty() {
                    true => entry.host().to_owned(),
                    false => entry.network.clone(),
                };
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
    label: &'static str,
    hint: &'static str,
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
                .child(sans(500, 14.).child(label))
                .child(
                    sans(400, 13.)
                        .line_height(px(13. * 1.5))
                        .text_color(ink.muted)
                        .child(hint),
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
