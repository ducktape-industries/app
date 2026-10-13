//! The bell's panel: the notification centre's rows.

use super::Chrome;
use crate::a11y::Control as _;
use crate::runtime::notify::{InboxState, Permission};
use crate::shell::entities::{Notifications, Overlay, Popover, SettingsPage};
use crate::shell::ink::{Ink, mono, sans, tall};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl Chrome {
    /// The bell's panel (the NotifCenter board): 400 wide under the bell,
    /// kept inside the window; the rows, newest first, under Today and
    /// Earlier. The ages count from the wall clock, drawn again by the
    /// menu's own second; the rows are read again when the log moves
    /// (`Notifications.entries_rev`).
    pub(super) fn bell_menu(
        &self,
        ink: &Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const WIDTH: f32 = 400.;
        let viewport = window.viewport_size();
        let (wide, high) = (f32::from(viewport.width), f32::from(viewport.height));
        let width = WIDTH.min(wide - 16.).max(0.);
        let now = crate::runtime::notify::wall();
        let midnight = local_midnight(now);
        let notifications = self.notifications.read(cx);
        let unread = notifications.unread();
        let entries = notifications.entries();
        let state = notifications.state().clone();
        // a change upstream (all read, clear) is offered only with the
        // inbox in hand: it would be local alone otherwise
        let ready = state == InboxState::Ready;
        let asking: Vec<&'static str> = notifications
            .asking()
            .iter()
            .map(|program| crate::runtime::intern(program))
            .collect();
        let rail = self.rail.read(cx).rows().to_vec();
        // where the inbox stands: no inbox in the count's place, anything
        // else said in full on its own line under the header
        let standing = match &state {
            InboxState::NoInbox => "This address keeps no notifications.".to_owned(),
            _ => format!("{unread} unread"),
        };
        let note = match &state {
            InboxState::Unreachable { since } => {
                Some(format!("Inbox unreachable since {}", clock(*since)))
            }
            InboxState::Denied(why) => Some(format!("Not signed in to the inbox: {why}")),
            InboxState::NoInbox | InboxState::Off | InboxState::Ready => notifications.stuck(),
        };
        let small_link = |id: &'static str, text: &'static str, run: Box<dyn Fn(&mut App)>| {
            let hover = ink.ink;
            crate::a11y::keyboard(
                sans(400, 13.)
                    .id(id)
                    .control(Role::Button, text)
                    .text_color(ink.muted)
                    .cursor_pointer()
                    .hover(move |style| style.text_color(hover))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        run(cx)
                    }),
                ink.ink,
            )
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(16.))
            .h(px(tall(46.)))
            .flex_shrink_0()
            .border_b_1()
            .border_color(ink.line)
            .child(sans(500, 14.).child("Notifications"))
            .child(
                mono(400, 12.)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(ink.muted)
                    .child(standing),
            )
            .when(unread > 0 && ready, |header| {
                header.child(
                    small_link(
                        "notif-mark-all",
                        "Mark all read",
                        Box::new(self.on_notifications(Notifications::mark_all_read)),
                    )
                    .child("Mark all read")
                    .text_color(ink.ink)
                    .underline(),
                )
            });
        let mut list = div()
            .id("notif-rows")
            .role(Role::Menu)
            .aria_label("Notifications")
            .flex()
            .flex_col()
            .max_h(px(
                (high - self.inset(cx).menus - 4. - 46. - 38. - 8.).max(80.)
            ))
            .overflow_y_scroll();
        // the question for a program whose first entry arrived before the
        // person said a word on its banners
        for module in asking {
            let name = crate::shell::panes::label(&rail, module);
            let answer = |word: Permission| {
                let (notifications, prefs) = (
                    self.notifications.entity().clone(),
                    self.prefs.entity().clone(),
                );
                crate::a11y::keyboard(
                    sans(500, 13.)
                        .id(SharedString::from(format!(
                            "notify-ask/{module}/{}",
                            word.word()
                        )))
                        .control(Role::Button, word.word())
                        .px(px(10.))
                        .h(px(tall(26.)))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .border_1()
                        .border_color(ink.ink)
                        .when(word == Permission::Allow, |button| {
                            button.bg(ink.ink).text_color(ink.bg)
                        })
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            crate::shell::entities::permission(
                                &notifications,
                                &prefs,
                                module,
                                word,
                                cx,
                            )
                        })
                        .child(word.word()),
                    ink.ring(word == Permission::Allow),
                )
            };
            list = list.child(
                crate::a11y::live(
                    div()
                        .id(SharedString::from(format!("notify-ask/{module}")))
                        .role(Role::Group),
                    accesskit::Live::Polite,
                    format!("Show banners from {name}?"),
                )
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(16.))
                .py(px(10.))
                .bg(ink.surface)
                .border_b_1()
                .border_color(ink.line)
                .child(
                    sans(500, 13.)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("Show banners from {name}?")),
                )
                .child(answer(Permission::Allow))
                .child(answer(Permission::Silent)),
            );
        }
        if let Some(note) = note {
            list = list.child(
                crate::a11y::live(
                    div().id("notif-state").role(Role::Status),
                    accesskit::Live::Polite,
                    note.clone(),
                )
                .px(px(16.))
                .py(px(8.))
                .border_b_1()
                .border_color(ink.line)
                .child(mono(400, 12.).text_color(ink.muted).child(note)),
            );
        }
        let mut section = None;
        for entry in &entries {
            let today = entry.at >= midnight;
            if section != Some(today) {
                section = Some(today);
                list = list.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .px(px(16.))
                        .pt(px(12.))
                        .pb(px(6.))
                        .border_b_1()
                        .border_color(ink.line)
                        .child(if today { "Today" } else { "Earlier" }),
                );
            }
            let (windows, notifications, overlays) = (
                self.windows.clone(),
                self.notifications.entity().clone(),
                self.overlays.entity().clone(),
            );
            let id = entry.id;
            let surface = ink.surface;
            let initial: String = entry
                .title
                .chars()
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .collect();
            let source = source(&rail, &entry.program, &entry.tag);
            let said = format!(
                "{}{}: {}",
                if entry.read { "" } else { "Unread. " },
                entry.title,
                entry.body
            );
            let line = |text: String, color: Hsla| {
                sans(400, 13.)
                    .text_color(color)
                    .whitespace_nowrap()
                    .truncate()
                    .child(text)
            };
            // when and from where, as the row shows them, after its name
            let about = format!("{} · {source}", ago(entry.at, now));
            list = list.child(crate::a11y::keyboard(
                div()
                    .id(SharedString::from(format!("notif/{id}")))
                    .control(Role::MenuItem, SharedString::from(said))
                    .aria_description(about)
                    .flex()
                    .gap(px(10.))
                    .pl(px(8.))
                    .pr(px(16.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(ink.line)
                    .cursor_pointer()
                    .hover(move |style| style.bg(surface))
                    // the bell closes, then the notice opens what it is about
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        overlays.update(cx, |overlays, cx| {
                            overlays.close(Overlay::Menu(Popover::Notifications), cx)
                        });
                        let opened = notifications
                            .update(cx, |notifications, cx| notifications.open(id, cx));
                        if let Some(opened) = opened {
                            windows.update(cx, |windows, cx| windows.open_notice(&opened, cx));
                        }
                    })
                    .child(
                        div()
                            .w(px(8.))
                            .h(px(28.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .when(!entry.read, |dot| {
                                dot.child(div().size(px(5.)).rounded_full().bg(ink.ink))
                            }),
                    )
                    .child(
                        sans(500, 12.)
                            .size(px(28.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(ink.surface)
                            .text_color(ink.muted)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        sans(if entry.read { 400 } else { 500 }, 14.)
                                            .min_w_0()
                                            .truncate()
                                            .text_color(match entry.read {
                                                true => ink.muted,
                                                false => ink.ink,
                                            })
                                            .child(entry.title.clone()),
                                    )
                                    .when(entry.count > 1, |row| {
                                        row.child(
                                            mono(400, 11.)
                                                .px(px(5.))
                                                .border_1()
                                                .border_color(ink.line)
                                                .text_color(ink.muted)
                                                .child(entry.count.to_string()),
                                        )
                                    })
                                    .child(div().flex_1())
                                    .child(
                                        mono(400, 12.)
                                            .flex_shrink_0()
                                            .text_color(ink.muted)
                                            .child(ago(entry.at, now)),
                                    ),
                            )
                            .when(!entry.body.is_empty(), |text| {
                                text.child(line(
                                    entry.body.replace('\n', " "),
                                    match entry.read {
                                        true => ink.muted,
                                        false => ink.figure,
                                    },
                                ))
                            })
                            .child(mono(400, 11.).text_color(ink.muted).child(source)),
                    ),
                ink.ink,
            ));
        }
        if entries.is_empty() {
            let (said, more) = match &state {
                InboxState::NoInbox => (
                    "No notifications here",
                    "This address keeps no inbox, so nothing is kept for you.",
                ),
                _ => (
                    "You\u{2019}re all caught up",
                    "Notices from your programs land here.",
                ),
            };
            list = list.child(
                crate::a11y::live(
                    div().id("notif-empty").role(Role::Status),
                    accesskit::Live::Polite,
                    said,
                )
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .py(px(44.))
                .child(sans(500, 14.).child(said))
                .child(sans(400, 13.).text_color(ink.muted).child(more)),
            );
        }
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(16.))
            .h(px(tall(38.)))
            .flex_shrink_0()
            .border_t_1()
            .border_color(ink.line)
            // only while something read is there to clear
            .when(ready && entries.iter().any(|entry| entry.read), |footer| {
                footer.child(
                    small_link(
                        "notif-clear-read",
                        "Clear read",
                        Box::new(self.on_notifications(Notifications::clear_read)),
                    )
                    .child("Clear read"),
                )
            })
            .child(
                // Settings in the bell's place, on its Notifications page
                small_link(
                    "notif-settings",
                    "Notification settings",
                    Box::new(self.overlaying(|overlays, cx| {
                        overlays.open(Overlay::Settings(SettingsPage::Notifications), cx)
                    })),
                )
                .ml_auto()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Settings)
                        .size(px(13.)),
                )
                .child("Notification settings"),
            );
        let body = div()
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .child(footer);
        self.popover(Popover::Notifications, width, body, ink, window, cx)
    }
}

/// A row's "from where": the program as the rail names it (its view's
/// title, else its id, as every other place names it), then its tag.
fn source(rail: &[crate::runtime::RailRow], program: &str, tag: &str) -> String {
    let name = crate::shell::panes::label(rail, program);
    match tag.is_empty() {
        true => name,
        false => format!("{name} · {tag}"),
    }
}

/// "14:02": the local clock at the wall second `wall`.
fn clock(wall: i64) -> String {
    let local = (wall + crate::runtime::local_offset(wall)).rem_euclid(86_400);
    format!("{:02}:{:02}", local / 3_600, (local % 3_600) / 60)
}

/// Unix seconds at the start of the local day `wall` falls in.
fn local_midnight(wall: i64) -> i64 {
    let offset = crate::runtime::local_offset(wall);
    (wall + offset).div_euclid(86_400) * 86_400 - offset
}

/// "now", "14m", "2h", "Yesterday", "3d".
fn ago(at: i64, now: i64) -> String {
    let seconds = (now - at).max(0);
    match seconds {
        ..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        _ if at >= local_midnight(now) => format!("{}h", seconds / 3_600),
        _ if at >= local_midnight(now) - 86_400 => "Yesterday".into(),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, local_midnight, source};

    /// A row names its program as the rail does: the view's title for a
    /// program on the rail, its id otherwise; the tag as the inbox gave it.
    #[test]
    fn a_row_names_a_rail_program_by_its_label() {
        let rail = vec![crate::runtime::RailRow {
            module: "chat",
            label: "Chat".into(),
            icon: String::new(),
            note: None,
            empty: false,
        }];
        assert_eq!(source(&rail, "chat", "general"), "Chat · general");
        assert_eq!(source(&rail, "chat", ""), "Chat");
        assert_eq!(source(&rail, "forge", "web#3"), "forge · web#3");
    }

    #[test]
    fn times_read_short() {
        let now = local_midnight(1_790_121_600) + 12 * 3_600;
        assert_eq!(ago(now - 5, now), "now");
        assert_eq!(ago(now - 14 * 60, now), "14m");
        assert_eq!(ago(now - 2 * 3_600, now), "2h");
        assert_eq!(ago(now - 20 * 3_600, now), "Yesterday");
        assert_eq!(ago(now - 4 * 86_400, now), "4d");
    }
}
