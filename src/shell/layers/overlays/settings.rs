//! Settings: the app's own, in a dialog over the desk (the Settings
//! board). The account's settings are the network's, in its program;
//! these are this device's.

use super::super::super::ink::{self, Ink, mono, sans, words};
use super::super::Inset;
use super::{DIALOG_EDGE, OverlayLayer, dialog_fit, scrim};
use crate::a11y::Control as _;
use crate::shell::entities::{Overlay, Prefs, SettingsPage, Slice};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, Context, Window};
use std::cell::Cell;
use std::rc::Rc;

type Pick = Rc<dyn Fn(&mut App)>;

#[derive(Default)]
pub(super) struct Page {
    scroll: gpui_kit::ScrollHandle,
    rows: Vec<gpui_kit::FocusHandle>,
    held: Rc<Cell<Option<usize>>>,
}

const WIDE: f32 = 760.;
const TALL: f32 = 680.;
const HANG: f32 = 74.;
const STRIP: f32 = 34.;

#[derive(Default)]
pub(super) struct Moved {
    by: (f32, f32),
    held: Option<((f32, f32), (f32, f32))>,
}

fn placed(size: (f32, f32), inset: Inset, by: (f32, f32)) -> (f32, f32, (f32, f32)) {
    let (top, tall) = dialog_fit(size.1, inset, TALL, HANG);
    let room = size.0 - inset.left - 2. * DIALOG_EDGE;
    let side = ((room - WIDE.min(room)) / 2.).max(0.);
    let (up, down) = (DIALOG_EDGE - top, tall - TALL.min(tall));
    (top, tall, (by.0.clamp(-side, side), by.1.clamp(up, down)))
}

fn size_of(window: &Window) -> (f32, f32) {
    let size = window.viewport_size();
    (size.width.into(), size.height.into())
}

#[cfg(any(test, feature = "ax-door"))]
pub(crate) struct Kept {
    app: crate::shell::entities::Entities,
    prefs: Option<serde_json::Value>,
    asking: std::collections::BTreeSet<String>,
}

#[cfg(any(test, feature = "ax-door"))]
impl Kept {
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
    pub(super) fn settings(
        &mut self,
        shown: SettingsPage,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
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
            ("settings/updates", "Updates", SettingsPage::Updates),
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
            SettingsPage::Updates => self.updates_page(&ink, cx),
            SettingsPage::About => self.about_page(&ink, cx),
        };
        let page = &mut self.settings_rows;
        while page.rows.len() < rows.len() {
            page.rows.push(cx.focus_handle().tab_stop(false));
        }
        let held = {
            let (handles, held) = (page.rows[..rows.len()].to_vec(), page.held.clone());
            let scroll = page.scroll.clone();
            let this = cx.entity();
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
                move |_, _, window, _| {
                    crate::shell::pane_drag::follow_pointer(this, window, |this, to, window, cx| {
                        let Some((from, start)) = this.settings_moved.held else {
                            return;
                        };
                        let Some(to) = to else {
                            this.settings_moved.held = None;
                            cx.notify();
                            return;
                        };
                        let by = (start.0 + to.0 - from.0, start.1 + to.1 - from.1);
                        let (.., by) = placed(size_of(window), this.inset(window, cx), by);
                        if by != this.settings_moved.by {
                            this.settings_moved.by = by;
                            cx.notify();
                        }
                    })
                },
            )
            .absolute()
            .size_0()
        };
        let rows = rows.into_iter().zip(&page.rows).map(|(row, handle)| {
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
        let inset = self.inset(window, cx);
        let (top, tall, by) = placed(size_of(window), inset, self.settings_moved.by);
        let title = div()
            .h(px(STRIP))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(14.))
            .pr(px(6.))
            .border_b_1()
            .border_color(ink.line)
            .cursor(match self.settings_moved.held {
                Some(_) => CursorStyle::ClosedHand,
                None => CursorStyle::OpenHand,
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    let at = (event.position.x.into(), event.position.y.into());
                    let (.., by) = placed(
                        size_of(window),
                        this.inset(window, cx),
                        this.settings_moved.by,
                    );
                    this.settings_moved = Moved {
                        by,
                        held: Some((at, by)),
                    };
                    cx.notify();
                }),
            )
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
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
        scrim(
            "settings-window",
            Role::Dialog,
            "Settings",
            Overlay::Settings(shown),
            self.overlays.entity(),
            &self.modal,
            inset,
            &ink,
            |card| {
                card.mt(px(top))
                    .w(px(WIDE))
                    .h(px(TALL))
                    .max_w_full()
                    .max_h(px(tall))
                    .self_start()
                    .left(px(by.0))
                    .top(px(by.1))
                    .child(title)
                    .child(body)
                    .into_any_element()
            },
        )
    }

    fn on_prefs(
        &self,
        call: impl Fn(&mut Slice<Prefs>, &mut Context<Slice<Prefs>>) + 'static,
    ) -> Pick {
        let prefs = self.prefs.entity().clone();
        Rc::new(move |cx| prefs.update(cx, |prefs, cx| call(prefs, cx)))
    }

    fn appearance_page(&mut self, ink: &Ink, cx: &gpui_kit::App) -> Vec<gpui_kit::Div> {
        use crate::backend::{Appearance, Layout};
        let (appearance, layout, motion) = {
            let prefs = self.prefs.read(cx).get();
            (prefs.appearance, prefs.layout_or_default(), prefs.motion)
        };
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
        let layout = self.segmented(
            "layout",
            "Layout",
            [("Menu bar", Layout::MenuBar), ("Sidebar", Layout::Sidebar)].map(|(label, pick)| {
                (
                    label.into(),
                    layout == pick,
                    self.on_prefs(move |prefs, cx| prefs.set_layout(pick, cx)),
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
                "Layout",
                "Where the programs and their open windows are listed: across the top, or down the side.",
                layout,
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
                let program = row.program;
                let name = super::super::tab_label(&row);
                let chosen = settings.views.get(program).copied();
                let week = self.notifications.read(cx).this_week(program, now);
                let hint = match week {
                    0 => "None yet".to_owned(),
                    week => format!("{week} this week"),
                };
                let control = self.segmented(
                    &format!("notify/view/{program}"),
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
                                program,
                                permission,
                                cx,
                            )
                        });
                        (permission.word().into(), chosen == Some(permission), pick)
                    }),
                    &ink,
                    cx,
                );
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
                                SharedString::from(format!("notify/view/{program}/name")),
                                name,
                            )))
                            .child(sans(400, 13.).text_color(ink.muted).child(words(
                                SharedString::from(format!("notify/view/{program}/week")),
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
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, _, cx| picks[nth](cx))
                    .child(label),
                (nth == chosen).then_some(&stop),
                ink.ink,
            )
        }))
    }

    fn updates_page(&mut self, ink: &Ink, cx: &gpui_kit::App) -> Vec<gpui_kit::Div> {
        use crate::update::{Channel, Updates};
        use gpui_kit::*;
        let mut rows = vec![
            heading("Updates", ink),
            line("Version", crate::update::version(), ink),
        ];
        let status = |said: String| {
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
                        .child(sans(500, 14.).child(words("Status", "Status")))
                        .child(
                            sans(400, 13.)
                                .line_height(px(13. * 1.5))
                                .text_color(ink.muted)
                                .child(words("Status/value", said)),
                        ),
                )
        };
        let shown = match crate::update::updates() {
            Updates::Here(shown) => shown,
            Updates::Off => {
                rows.push(status("Off: this copy is not an installed one".into()));
                return rows;
            }
            Updates::Elsewhere => {
                rows.push(status(
                    "Another running Ducktape updates this install".into(),
                ));
                return rows;
            }
        };
        let channel = self.segmented(
            "updates/channel",
            "Channel",
            Channel::ALL.map(|channel| {
                let label = match channel {
                    Channel::Stable => "Stable",
                    Channel::Nightly => "Nightly",
                };
                let pick: Pick = Rc::new(move |cx| {
                    crate::update::choose(channel);
                    cx.refresh_windows();
                });
                (label.into(), shown.channel == channel, pick)
            }),
            ink,
            cx,
        );
        let on = shown.automatic;
        let automatic = self.switch(
            "updates/automatic",
            "Update automatically",
            on,
            Rc::new(move |cx| {
                crate::update::automate(!on);
                cx.refresh_windows();
            }),
            ink,
        );
        let button = |id: &'static str, label: &'static str| {
            sans(500, 14.)
                .id(id)
                .control(Role::Button, label)
                .px(px(12.))
                .py(px(6.))
                .border_1()
                .border_color(ink.ink)
                .cursor_pointer()
                .child(label)
        };
        let windows = self.app.windows.clone();
        let restart = crate::a11y::keyboard(
            button("updates/restart", "Restart to update").on_click(move |_, _, cx| {
                if crate::update::restart().is_ok() {
                    windows.update(cx, |windows, cx| windows.quit(cx));
                }
            }),
            ink.ink,
        );
        let update_now = crate::a11y::keyboard(
            button("updates/now", "Update now").on_click(|_, _, _| crate::update::update_now()),
            ink.ink,
        );
        let check = match shown.checking {
            true => crate::a11y::Patch::default()
                .busy()
                .disabled()
                .on(crate::a11y::keyboard(
                    button("updates/check", "Checking…")
                        .cursor_default()
                        .opacity(0.3),
                    ink.ink,
                )),
            false => crate::a11y::keyboard(
                button("updates/check", "Check now").on_click(|_, _, cx| {
                    crate::update::check_now();
                    cx.refresh_windows();
                }),
                ink.ink,
            ),
        };
        rows.push(setting(
            "Channel",
            "Stable follows each release; Nightly is the newest work, built once a day.",
            channel,
            ink,
        ));
        rows.push(setting(
            "Update automatically",
            "Off still checks and says what is newer, and downloads or installs nothing until you press Update now.",
            automatic,
            ink,
        ));
        let said = match shown.checked {
            Some(checked) => format!("{} · {checked}", shown.status),
            None => shown.status,
        };
        rows.push(
            status(said)
                .child(div().flex_shrink_0().child(check))
                .when(shown.restart, |row| {
                    row.child(div().flex_shrink_0().child(restart))
                })
                .when(shown.update_now, |row| {
                    row.child(div().flex_shrink_0().child(update_now))
                }),
        );
        rows
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

fn heading(text: &'static str, ink: &Ink) -> gpui_kit::Div {
    use gpui_kit::*;
    mono(400, 12.)
        .text_color(ink.muted)
        .pb(px(6.))
        .child(words(text, text))
}

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

fn line(key: &'static str, value: String, ink: &Ink) -> gpui_kit::Div {
    use gpui_kit::*;
    div()
        .flex()
        .gap(px(16.))
        .py(px(8.))
        .child(
            sans(400, 14.)
                .w(px(120.))
                .text_color(ink.muted)
                .child(words(key, key)),
        )
        .child(
            mono(400, 13.)
                .text_color(ink.muted)
                .flex_1()
                .min_w_0()
                .child(words(SharedString::from(format!("{key}/value")), value)),
        )
}

impl OverlayLayer {
    /// Settings › About: the product, its version and log; and on a
    /// machine whose call picture is Cisco's OpenH264 binary, the switch
    /// that fetches and uses it, with Cisco's line (its licence's terms).
    fn about_page(&self, ink: &Ink, cx: &gpui_kit::App) -> Vec<gpui_kit::Div> {
        use gpui_kit::*;
        let log = crate::backend::app_log_path()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let rows = vec![
            heading("About", ink),
            sans(400, 22.)
                .pb(px(12.))
                .child(words("product", "Ducktape")),
            line("Version", crate::update::version(), ink),
            line(
                "Node contract",
                format!("v{}", noded_client::wire::NODE_CONTRACT),
                ink,
            ),
            line("Log", log, ink),
        ];
        let debug = self.prefs.read(cx).get().debug;
        let switch = self.switch(
            "about/debug",
            "Debug mode",
            debug,
            self.on_prefs(move |prefs, cx| prefs.set_debug(!debug, cx)),
            ink,
        );
        let mut rows = rows;
        rows.push(setting(
            "Debug mode",
            "Turns on the app's counters, and the account menu's Memory window: what the app holds, by owner.",
            switch,
            ink,
        ));
        #[cfg(all(feature = "call", target_os = "linux"))]
        let rows = {
            use crate::backend::openh264;
            let on = openh264::enabled();
            let switch = self.switch(
                "about/openh264",
                "Video calls with H.264",
                on,
                Rc::new(move |cx| {
                    openh264::set_enabled(!on);
                    cx.refresh_windows();
                }),
                ink,
            );
            rows.push(setting(
                "Video calls with H.264",
                openh264::ATTRIBUTION,
                switch,
                ink,
            ));
            rows.push(
                sans(400, 13.)
                    .pt(px(8.))
                    .text_color(ink.muted)
                    .child(words(
                        "about/openh264/hint",
                        "Fetched from Cisco onto this machine the first time a call sends or shows a picture; off, calls go on by voice.",
                    )),
            );
            rows.push(line("H.264 licence", openh264::license_path(), ink));
            rows
        };
        rows
    }
}
