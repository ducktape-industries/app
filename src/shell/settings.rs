//! Settings: the app's own, in a window of their own that floats over the
//! desk (its title bar says "Settings"). The account's settings are the
//! network's, in its program; these are this device's.

use super::*;
use crate::SettingsPage;
use launcher::mono;

pub(super) const SETTINGS_SIZE: (f32, f32) = (760., 540.);

impl DesktopWindow {
    pub(super) fn settings(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let palette = design::palette(state.dark);
        let muted = hsla_of(palette.muted);
        let border = hsla_of(palette.border);
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
                div()
                    .id(id)
                    .control(Role::Tab, label)
                    .aria_selected(on)
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .text_color(match on {
                        true => hsla_of(palette.foreground),
                        false => muted,
                    })
                    .when(on, |row| row.bg(hsla_of(palette.surface_raised)))
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
            SettingsPage::About => about_page(muted),
        };
        div()
            .id("settings-window")
            .size_full()
            .flex()
            .text_size(px(14.))
            .child(
                div()
                    .id("settings-nav")
                    .role(Role::TabList)
                    .w(px(180.))
                    .h_full()
                    .flex_shrink_0()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .border_r_1()
                    .border_color(border)
                    .children(nav),
            )
            .child(
                div()
                    .id("settings-page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_8()
                    .py_6()
                    .child(page),
            )
            .into_any_element()
    }

    fn appearance_page(&self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use crate::Appearance;
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let fg = hsla_of(palette.foreground);
        let bg = hsla_of(palette.background);
        let line = hsla_of(palette.border_strong);
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
                        div()
                            .id(SharedString::from(format!("theme/{label}")))
                            .control(Role::RadioButton, label)
                            .aria_toggled(match on {
                                true => gpui_kit::accesskit::Toggled::True,
                                false => gpui_kit::accesskit::Toggled::False,
                            })
                            .px_3()
                            .py_1p5()
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
            .child(heading("Appearance", hsla_of(palette.muted)))
            .child(setting(
                "Theme",
                "Follows the system unless you pick one.",
                theme,
                palette,
            ))
            .child(setting(
                "Moving figures",
                "The drawings in characters turn slowly, and the node's dot breathes. Off keeps them still.",
                motion,
                palette,
            ))
            .into_any_element()
    }

    fn networks_page(&self, state: &screens::Facts) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let palette = design::palette(state.dark);
        let muted = hsla_of(palette.muted);
        let danger = hsla_of(palette.danger);
        let rows = state.recent_endpoints.iter().map(|entry| {
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
                .gap_4()
                .py_2p5()
                .border_b_1()
                .border_color(hsla_of(palette.border))
                .child(
                    div()
                        .w(px(120.))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(name),
                )
                .child(
                    mono(entry.host().to_owned(), muted)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .child(div().text_size(px(12.5)).text_color(muted).child(
                    match (current, entry.other_chain) {
                        (true, _) => "Current",
                        (false, true) => "Other chain",
                        (false, false) => "",
                    },
                ))
                .child(crate::a11y::keyboard(
                    div()
                        .id(SharedString::from(format!("settings/forget/{url}")))
                        .control(Role::Button, SharedString::from(format!("Forget {url}")))
                        .px_1()
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
            .child(heading("Networks", muted))
            .child(
                div()
                    .pb_3()
                    .text_size(px(13.))
                    .text_color(muted)
                    .child("Nodes this device reached. Forgetting one takes it off the list; its key stays on this device."),
            )
            .children(rows)
            .into_any_element()
    }
}

fn heading(text: &'static str, color: gpui_kit::Hsla) -> gpui_kit::Div {
    mono(text, color).pb_2()
}

/// One row of a settings page: what it is, a line about it, its control.
fn setting(
    label: &'static str,
    hint: &'static str,
    control: impl gpui_kit::IntoElement,
    palette: &design::Palette,
) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .items_center()
        .gap_6()
        .py_4()
        .border_b_1()
        .border_color(hsla_of(palette.border))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().font_weight(FontWeight::MEDIUM).child(label))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .text_color(hsla_of(palette.muted))
                        .child(hint),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}

fn about_page(muted: gpui_kit::Hsla) -> gpui_kit::AnyElement {
    use gpui_kit::*;
    let log = crate::backend::app_log_path()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let line = |key: &'static str, value: String| {
        div()
            .flex()
            .gap_4()
            .py_2()
            .child(div().w(px(120.)).text_color(muted).child(key))
            .child(mono(value, muted).flex_1().min_w_0())
    };
    div()
        .flex()
        .flex_col()
        .child(heading("About", muted))
        .child(div().text_size(px(22.)).pb_3().child("Ducktape"))
        .child(line("Version", env!("CARGO_PKG_VERSION").into()))
        .child(line(
            "Node contract",
            format!("v{}", crate::backend::noded::NODE_CONTRACT),
        ))
        .child(line("Log", log))
        .into_any_element()
}
