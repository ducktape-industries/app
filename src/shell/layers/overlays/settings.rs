//! Settings: the app's own, in a dialog over the desk (the Settings
//! board). The account's settings are the network's, in its program;
//! these are this device's.

use super::super::super::ink::{self, Ink, mono, sans, words};
use super::{OverlayLayer, dialog_fit, scrim};
use crate::a11y::Control as _;
use crate::shell::entities::{Overlay, Prefs, SettingsPage, Slice};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, Context, Window};
use std::cell::Cell;
use std::rc::Rc;

/// What a Settings control does when picked.
type Pick = Rc<dyn Fn(&mut App)>;

/// Settings' page as it scrolls: a handle on each row, never a Tab stop
/// itself, so the row that takes the keys scrolls into view.
#[derive(Default)]
pub(super) struct Page {
    scroll: gpui_kit::ScrollHandle,
    rows: Vec<gpui_kit::FocusHandle>,
    /// The row that held the keys when the page was last laid out.
    held: Rc<Cell<Option<usize>>>,
}

/// What Settings' picks change, as it stood: the prefs, and what they
/// drive in memory (the theme, which views are asking). The door's walk
/// keeps it before its arrows pick a radio group's choices, and a pick
/// saves, and puts it back after (docs/ax.md §1.1).
#[cfg(any(test, feature = "ax-door"))]
pub(crate) struct Kept {
    app: crate::shell::entities::Entities,
    /// None when the file could not be read: then nothing is put back.
    prefs: Option<serde_json::Value>,
    asking: std::collections::BTreeSet<String>,
}

#[cfg(any(test, feature = "ax-door"))]
impl Kept {
    /// The app behind `window` as it stands; none for a window that is not
    /// the shell's.
    pub(crate) fn of(window: &Window, cx: &gpui_kit::App) -> Option<Self> {
        let root = window.root::<gpui_kit::component::Root>().flatten()?;
        let view = root
            .read(cx)
            .view()
            .clone()
            .downcast::<super::super::WindowRoot>();
        let app = view.ok()?.read(cx).app.clone();
        Some(Self {
            prefs: crate::backend::read_prefs().ok(),
            asking: app.notifications.read(cx).center().lock().asking.clone(),
            app,
        })
    }

    /// Back as it stood: the file written only where it differs, then read
    /// again into `Prefs` (the theme follows), and the views asking put back.
    pub(crate) fn restore(self, cx: &mut gpui_kit::App) {
        if let Some(prefs) = self.prefs {
            crate::backend::edit_prefs(|now| *now = prefs);
        }
        self.app.prefs.update(cx, |prefs, cx| prefs.reload(cx));
        self.app.notifications.update(cx, |notifications, cx| {
            notifications.restore_asking(self.asking, cx)
        });
    }
}

impl OverlayLayer {
    /// The dialog on `shown`: `760 × 680; border: 1.5px solid ink` (the
    /// board's 540, taller so every view's row fits), a 34px title strip
    /// with its close, on a scrim below the bar. A click on the scrim, or
    /// Escape, closes it.
    pub(super) fn settings(
        &mut self,
        shown: SettingsPage,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        // the Settings board: nav `padding: 12px 8px; gap: 2px`, each
        // entry `padding: 8px 14px; font: 400 14px`; the page `24px 32px`
        let ink = Ink::of(self.prefs.read(cx).get().dark());
        let pages = [
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
        ];
        let at = pages
            .iter()
            .position(|(_, _, page)| *page == shown)
            .unwrap_or_default();
        let stop = self.stop("settings-nav", cx);
        let nav = pages.map(|(id, label, page)| {
            let overlays = self.overlays.entity().clone();
            let on = shown == page;
            crate::a11y::roving_item(
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
                    // a press leaves the keys where they were
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, _, cx| {
                        overlays.update(cx, |overlays, cx| {
                            overlays.open(Overlay::Settings(page), cx)
                        })
                    })
                    .child(label),
                on.then_some(&stop),
                ink.ink,
            )
        });
        let rows = match shown {
            SettingsPage::Appearance => self.appearance_page(&ink, cx),
            SettingsPage::Notifications => self.notifications_page(&ink, cx),
            SettingsPage::Networks => self.networks_page(&ink, cx),
            SettingsPage::About => about_page(&ink),
        };
        let page = &mut self.settings_rows;
        while page.rows.len() < rows.len() {
            page.rows.push(cx.focus_handle().tab_stop(false));
        }
        // the row that has the keys scrolls into view when it is another
        // than last time: found at prepaint, ahead of the page's own (the
        // door reads the frame a Tab draws, so the row shows in that one)
        let held = {
            let (handles, held) = (page.rows[..rows.len()].to_vec(), page.held.clone());
            let scroll = page.scroll.clone();
            canvas(
                move |_, window, cx| {
                    let now = handles
                        .iter()
                        .position(|row| row.contains_focused(window, cx));
                    if now != held.replace(now)
                        && let Some(row) = now
                    {
                        scroll.scroll_to_item(row);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0()
        };
        let rows = rows.into_iter().zip(&page.rows).map(|(row, handle)| {
            // a press on a row's words leaves the keys where they were
            row.flex_shrink_0()
                .track_focus(handle)
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        });
        let picked = self.overlays.entity().clone();
        let body = div()
            .id("settings-body")
            .flex_1()
            .min_h_0()
            .flex()
            .text_size(px(ink::fit(14.)))
            .child(held)
            .child(
                crate::a11y::roving(
                    div()
                        .id("settings-nav")
                        .control(Role::TabList, "Settings pages"),
                    &stop,
                    accesskit::Orientation::Vertical,
                    [at, pages.len()],
                    // the arrows open the page, as a view's settings do
                    move |to, _, cx| {
                        let page = pages[to].2;
                        picked.update(cx, |overlays, cx| {
                            overlays.open(Overlay::Settings(page), cx)
                        })
                    },
                )
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
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&page.scroll)
                    .px(px(32.))
                    .py(px(24.))
                    .children(rows),
            );
        let shut = self.overlays.entity().clone();
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
                        shut.update(cx, |overlays, cx| {
                            overlays.close(Overlay::Settings(shown), cx)
                        })
                    })
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::X).size(px(16.)),
                    ),
                ink.ink,
            ));
        let (top, tall) = dialog_fit(f32::from(window.viewport_size().height), 680., 74.);
        scrim(
            "settings-window",
            Role::Dialog,
            "Settings",
            Overlay::Settings(shown),
            self.overlays.entity(),
            &self.modal,
            &ink,
            |card| {
                card.mt(px(top))
                    .w(px(760.))
                    .h(px(680.))
                    .max_w_full()
                    .max_h(px(tall))
                    .self_start()
                    .child(title)
                    .child(body)
                    .into_any_element()
            },
        )
    }

    /// What a pick does: `call` on the prefs.
    fn on_prefs(
        &self,
        call: impl Fn(&mut Slice<Prefs>, &mut Context<Slice<Prefs>>) + 'static,
    ) -> Pick {
        let prefs = self.prefs.entity().clone();
        Rc::new(move |cx| prefs.update(cx, |prefs, cx| call(prefs, cx)))
    }

    fn appearance_page(&mut self, ink: &Ink, cx: &gpui_kit::App) -> Vec<gpui_kit::Div> {
        use crate::backend::Appearance;
        let (appearance, motion) = {
            let prefs = self.prefs.read(cx).get();
            (prefs.appearance, prefs.motion)
        };
        // one look for every choice in Settings: the segmented row
        let theme = self.segmented(
            "theme",
            "Theme",
            [
                ("Light", Appearance::Light),
                ("Dark", Appearance::Dark),
                ("System", Appearance::System),
            ]
            .map(|(label, mode)| {
                (
                    label.into(),
                    appearance == mode,
                    self.on_prefs(move |prefs, cx| prefs.set_appearance(mode, cx)),
                )
            }),
            ink,
            cx,
        );
        let motion = self.switch(
            "motion",
            "Moving figures",
            motion,
            self.on_prefs(move |prefs, cx| prefs.set_motion(!motion, cx)),
            ink,
        );
        vec![
            heading("Appearance", ink),
            setting(
                "Theme",
                "Follows the system unless you pick one.",
                theme,
                ink,
            ),
            setting(
                "Moving figures",
                "The drawings in characters turn slowly, and the node's dot breathes. Off keeps them still.",
                motion,
                ink,
            ),
        ]
    }

    /// The NotifSettings board: the device's say over banners, then each
    /// view's.
    fn notifications_page(&mut self, ink: &Ink, cx: &gpui_kit::App) -> Vec<gpui_kit::Div> {
        use crate::runtime::notify::{self, Permission};
        use gpui_kit::*;
        let ink = *ink;
        let settings = self.prefs.read(cx).get().notify.clone();
        let banners = self.switch(
            "notify/banners",
            "Desktop banners",
            settings.banners,
            self.on_prefs({
                let on = settings.banners;
                move |prefs, cx| prefs.set_notify_banners(!on, cx)
            }),
            &ink,
        );
        let front = self.segmented(
            "notify/front",
            "While Ducktape is in front",
            [("Show", true), ("Hide", false)].map(|(label, show)| {
                (
                    label.into(),
                    settings.in_front == show,
                    self.on_prefs(move |prefs, cx| prefs.set_notify_in_front(show, cx)),
                )
            }),
            &ink,
            cx,
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
                        self.on_prefs(move |prefs, cx| prefs.set_notify_burst(burst, cx)),
                    )
                }),
                &ink,
                cx,
            ))
            .child(
                mono(400, 12.)
                    .text_color(ink.muted)
                    .child(words("burst-unit", "a minute")),
            );
        let now = notify::wall();
        let listed: Vec<_> = self
            .rail
            .read(cx)
            .rows()
            .iter()
            .filter(|row| !row.empty)
            .cloned()
            .collect();
        let views: Vec<_> = listed
            .into_iter()
            .map(|row| {
                let module = row.module;
                let name = super::super::tab_label(&row);
                let chosen = settings.views.get(module).copied();
                let week = self.notifications.read(cx).this_week(module, now);
                let hint = match week {
                    0 => "None yet".to_owned(),
                    week => format!("{week} this week"),
                };
                let control = self.segmented(
                    &format!("notify/view/{module}"),
                    &format!("{name} notifications"),
                    Permission::ALL.map(|permission| {
                        let (notifications, prefs) = (
                            self.notifications.entity().clone(),
                            self.prefs.entity().clone(),
                        );
                        let pick: Pick = Rc::new(move |cx| {
                            crate::shell::entities::permission(
                                &notifications,
                                &prefs,
                                module,
                                permission,
                                cx,
                            )
                        });
                        (permission.word().into(), chosen == Some(permission), pick)
                    }),
                    &ink,
                    cx,
                );
                // one line a view, so the whole roster fits under the fold
                div()
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(ink.line)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(12.))
                            .child(sans(500, 14.).child(words(
                                SharedString::from(format!("notify/view/{module}/name")),
                                name,
                            )))
                            .child(sans(400, 13.).text_color(ink.muted).child(words(
                                SharedString::from(format!("notify/view/{module}/week")),
                                hint,
                            ))),
                    )
                    .child(div().flex_shrink_0().child(control))
            })
            .collect();
        [
            heading("Notifications", &ink),
            ink::note(
                "notifications-intro",
                "On this device. Views ask; Ducktape decides what reaches the screen.",
                ink.muted,
            )
            .pb(px(12.))
            .border_b_1()
            .border_color(ink.line),
            setting(
                "Desktop banners",
                "Off keeps everything in Notifications, silently.",
                banners,
                &ink,
            ),
            setting(
                "While Ducktape is in front",
                "Banners for a window you are looking at never show.",
                front,
                &ink,
            ),
            setting(
                "Burst limit",
                "Per view. Past it, one banner says how many more are waiting.",
                burst,
                &ink,
            ),
            mono(400, 12.)
                .text_color(ink.muted)
                .pt(px(24.))
                .pb(px(8.))
                .border_b_1()
                .border_color(ink.line)
                .child(words("views", "Views")),
        ]
        .into_iter()
        .chain(views)
        .collect()
    }

    /// An on/off setting: on, an ink track with the knob at the right; off,
    /// a `strong` outline with a muted knob at the left.
    fn switch(
        &self,
        id: &'static str,
        name: &'static str,
        on: bool,
        flip: Pick,
        ink: &Ink,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        crate::a11y::keyboard(
            div()
                .id(id)
                .control(Role::Switch, name)
                .aria_toggled(on.into())
                .w(px(36.))
                .h(px(20.))
                .p(px(3.))
                .flex()
                .items_center()
                .when(on, |track| track.justify_end())
                .cursor_pointer()
                .rounded_full()
                .border_1()
                .border_color(match on {
                    true => ink.ink,
                    false => ink.strong,
                })
                .when(on, |track| track.bg(ink.ink))
                .on_click(move |_, _, cx| flip(cx))
                .child(div().size(px(12.)).rounded_full().bg(match on {
                    true => ink.bg,
                    false => ink.muted,
                })),
            ink.ring(on),
        )
        .into_any_element()
    }

    /// A row of choices, the chosen one on `surface` (the NotifSettings
    /// board's segmented controls): a radio group, one Tab stop, whose
    /// arrows pick the next choice, as a view's do (`a11y::roving`).
    fn segmented<const N: usize>(
        &mut self,
        id: &str,
        name: &str,
        choices: [(gpui_kit::SharedString, bool, Pick); N],
        ink: &Ink,
        cx: &gpui_kit::App,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::*;
        let line = ink.strong;
        let stop = self.stop(id, cx);
        // the chosen one is the stop; with none chosen yet, the first
        let chosen = choices
            .iter()
            .position(|(_, on, _)| *on)
            .unwrap_or_default();
        let mut picks = Vec::with_capacity(N);
        let choices = choices.map(|(label, on, pick)| {
            picks.push(pick);
            (label, on)
        });
        let picks = Rc::new(picks);
        let arrows = picks.clone();
        crate::a11y::roving(
            div()
                .id(SharedString::from(id.to_owned()))
                .role(Role::RadioGroup)
                .aria_label(SharedString::from(name.to_owned())),
            &stop,
            accesskit::Orientation::Horizontal,
            [chosen, N],
            move |to, _, cx| arrows[to](cx),
        )
        .flex()
        .border_1()
        .border_color(line)
        .children(choices.into_iter().enumerate().map(|(nth, (label, on))| {
            let picks = picks.clone();
            crate::a11y::roving_item(
                sans(if on { 500 } else { 400 }, 13.)
                    .id(SharedString::from(format!("{id}/{label}")))
                    .control(Role::RadioButton, label.clone())
                    .aria_toggled(on.into())
                    .h(px(ink::tall(26.)))
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
                    // a press leaves the keys where they were
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, _, cx| picks[nth](cx))
                    .child(label),
                (nth == chosen).then_some(&stop),
                ink.ink,
            )
        }))
    }

    fn networks_page(&self, ink: &Ink, cx: &gpui_kit::App) -> Vec<gpui_kit::Div> {
        use gpui_kit::*;
        let ink = *ink;
        let (muted, danger) = (ink.muted, ink.danger);
        let session = self.session.read(cx).get();
        let rows =
            session.recent_endpoints.iter().map(|entry| {
                let entity = self.session.entity().clone();
                let url = entry.url.clone();
                let current = entry.url == session.connected_rpc;
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
                                entity.update(cx, |session, cx| session.forget(&url, cx))
                            })
                            .child("Forget"),
                        ink.ink,
                    ))
            });
        [
            heading("Networks", &ink),
            ink::note("networks-intro", "Nodes this device reached. Forgetting one takes it off the list; its key stays on this device.", muted)
                .pb(px(12.)),
        ]
        .into_iter()
        .chain(rows)
        .collect()
    }
}

/// A page's title, `400 12px MONO` muted; its words are its id.
fn heading(text: &'static str, ink: &Ink) -> gpui_kit::Div {
    use gpui_kit::*;
    mono(400, 12.)
        .text_color(ink.muted)
        .pb(px(6.))
        .child(words(text, text))
}

/// One row of a settings page: what it is, a line about it, its control
/// (`gap: 24px; padding: 16px 0`, a hairline under it). The label names
/// its two lines of text: `<label>` and `<label>/hint`.
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
                .child(sans(500, 14.).child(words(label, label)))
                .child(
                    sans(400, 13.)
                        .line_height(px(13. * 1.5))
                        .text_color(ink.muted)
                        .child(words(SharedString::from(format!("{label}/hint")), hint)),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}

fn about_page(ink: &Ink) -> Vec<gpui_kit::Div> {
    use gpui_kit::*;
    let log = crate::backend::app_log_path()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let muted = ink.muted;
    // the key and its value, each a Label: `Version`, `0.1.0`
    let line = |key: &'static str, value: String| {
        div()
            .flex()
            .gap(px(16.))
            .py(px(8.))
            .child(
                sans(400, 14.)
                    .w(px(120.))
                    .text_color(muted)
                    .child(words(key, key)),
            )
            .child(
                mono(400, 13.)
                    .text_color(muted)
                    .flex_1()
                    .min_w_0()
                    .child(words(SharedString::from(format!("{key}/value")), value)),
            )
    };
    vec![
        heading("About", ink),
        sans(400, 22.)
            .pb(px(12.))
            .child(words("product", "Ducktape")),
        line("Version", env!("CARGO_PKG_VERSION").into()),
        line(
            "Node contract",
            format!("v{}", crate::backend::noded::NODE_CONTRACT),
        ),
        line("Log", log),
    ]
}
