//! The sidebar (the Sidebar layout; the mock's `index-desk`): the chrome
//! as a column down the console's left, `SIDEBAR` wide. Top to bottom: the
//! network, Search, a hairline, the programs, each with the windows it has
//! open listed under it (on the desk, then in pop-outs, marked), then
//! after a hairline the windows no program row stands for (Help, empty
//! windows); and a footer with the bell, the node's breath, who is signed
//! in, and the gear. Its buttons and menus are the bar's (`Chrome`'s), its
//! menus hung beside the column (`Chrome::hanging`). The list is one Tab
//! stop the arrows move along (`a11y::roving`), as the bar's tabs are.

use super::{
    Bar, Chrome, Row, SIDEBAR_FOOTER_HEIGHT, accessible_program_name_with_status_and_unread,
    drag_handle, tab_label,
};
use crate::a11y::Control as _;
use crate::a11y::Named as _;
use crate::runtime::{RailRow, WindowKey};
use crate::shell::entities::Listed;
use crate::shell::ink::{Ink, mono, sans};
use crate::shell::{PaneMessage, layout, theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub(in crate::shell) fn window_label(
    label: &str,
    title: Option<&str>,
    ordinal: Option<usize>,
) -> String {
    match (title, ordinal) {
        (Some(title), _) => title.to_owned(),
        (None, Some(n)) if n > 1 => format!("{label} {n}"),
        (None, _) => label.to_owned(),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    row: Row,
    program: &'static str,
    label: String,
    kind: Kind,
    away: Option<WindowKey>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Program { head: bool, alone: Option<u64> },
    Window,
    Loose,
}

fn index(rows: &[RailRow], open: &[Listed]) -> Vec<Entry> {
    let mut untitled: Vec<&'static str> = Vec::new();
    let mut said = |label: &str, window: &Listed| {
        let ordinal = window.title.is_none().then(|| {
            untitled.push(window.program);
            untitled.iter().filter(|it| **it == window.program).count()
        });
        window_label(label, window.title.as_deref(), ordinal)
    };
    let mut entries = Vec::new();
    let rows: Vec<&RailRow> = rows.iter().filter(|row| !row.empty).collect();
    for row in &rows {
        let label = tab_label(row);
        let windows: Vec<&Listed> = open.iter().filter(|it| it.program == row.program).collect();
        let head = windows.len() > 1
            || windows
                .iter()
                .any(|it| it.title.is_some() || it.away.is_some());
        entries.push(Entry {
            row: Row::Program(row.program),
            program: row.program,
            label: label.clone(),
            kind: Kind::Program {
                head,
                alone: windows.first().filter(|_| !head).map(|it| it.instance),
            },
            away: None,
        });
        if head {
            for window in windows {
                entries.push(Entry {
                    row: Row::Window(window.instance),
                    program: window.program,
                    label: said(&label, window),
                    kind: Kind::Window,
                    away: window.away,
                });
            }
        }
    }
    for window in open
        .iter()
        .filter(|it| !rows.iter().any(|row| row.program == it.program))
    {
        let label = match window.program {
            layout::EMPTY => "New window",
            layout::HELP => "Help",
            layout::MEMORY => "Memory",
            program => crate::runtime::named(program),
        };
        entries.push(Entry {
            row: Row::Window(window.instance),
            program: window.program,
            label: said(label, window),
            kind: Kind::Loose,
            away: window.away,
        });
    }
    entries
}

impl Entry {
    /// Its element id: `rail/<program>` for a program, `rail/<program>/<instance>`
    /// for a window under it, `rail-help/<instance>`, `rail-memory/<instance>`
    /// and `rail-new/<instance>` for Help, Memory and an empty window.
    fn id(&self) -> SharedString {
        let id = match (self.row, self.program) {
            (Row::Program(program), _) => format!("rail/{}", crate::runtime::slug(program)),
            (Row::Window(instance), layout::HELP) => format!("rail-help/{instance}"),
            (Row::Window(instance), layout::MEMORY) => format!("rail-memory/{instance}"),
            (Row::Window(instance), layout::EMPTY) => format!("rail-new/{instance}"),
            (Row::Window(instance), program) => {
                format!("rail/{}/{instance}", crate::runtime::slug(program))
            }
        };
        id.into()
    }

    fn on(&self, front: Option<u64>) -> bool {
        let stands_for = match (self.row, self.kind) {
            (_, Kind::Program { alone, .. }) => alone,
            (Row::Window(instance), _) => Some(instance),
            _ => None,
        };
        stands_for.is_some() && stands_for == front
    }
}

impl Chrome {
    pub(super) fn sidebar(
        &mut self,
        bar: Bar,
        ink: &Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.bar_drawn = None;
        let titlebar = theme::traffic_lights(window);
        let (network, menu) = self.network(&bar, false, ink, cx);
        let network = div()
            .relative()
            .h(px(super::BAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(px(titlebar.unwrap_or(14.) - 10.))
            .child(network)
            .child(drag_handle("sidebar-handle", titlebar))
            .children(menu);
        let search = self
            .search(false, ink)
            .w_full()
            .px(px(14.))
            .justify_between();
        let line = || div().h(px(1.)).flex_shrink_0().bg(ink.line);
        let foot = |(button, menu): (Stateful<Div>, Option<AnyElement>)| {
            Self::footed(button.h_full().px(px(8.)), menu)
        };
        let footer = div()
            .h(px(SIDEBAR_FOOTER_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(px(6.))
            .pr(px(4.))
            .border_t_1()
            .border_color(ink.line)
            .child(foot(self.bell(&bar, ink, window, cx)))
            .child(foot(self.node(&bar, ink, window, cx)))
            .child({
                let (who, menu) = self.who(&bar, false, ink, window, cx);
                foot((who.flex_1().min_w_0().overflow_hidden(), menu))
                    .flex_1()
                    .min_w_0()
            })
            .child(self.gear(ink).h_full());
        div()
            .id("sidebar")
            .role_named_by_id(Role::MenuBar)
            .aria_label("Ducktape")
            .aria_orientation(accesskit::Orientation::Vertical)
            .size_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(ink.line)
            .bg(ink.bg)
            .children(self.menu_backdrop(&bar, window, cx))
            .child(network)
            .child(search)
            .child(line())
            .child(self.list(&bar, ink, cx))
            .child(footer)
            .into_any_element()
    }

    fn list(&self, bar: &Bar, ink: &Ink, cx: &mut Context<Self>) -> Stateful<Div> {
        let entries = index(&bar.rows, &bar.listed);
        let keys: Vec<Row> = entries.iter().map(|entry| entry.row).collect();
        let active = self
            .rail_cursor
            .and_then(|at| keys.iter().position(|row| *row == at))
            .or_else(|| entries.iter().position(|entry| entry.on(bar.front)))
            .unwrap_or_default();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (n, entry) in entries.iter().enumerate() {
            if entry.kind == Kind::Loose && n > 0 && entries[n - 1].kind != Kind::Loose {
                rows.push(div().h(px(1.)).my(px(8.)).bg(ink.line).into_any_element());
            }
            let stop = (n == active).then_some(&self.stop);
            rows.push(match entry.kind {
                Kind::Program { head, .. } => self.program_row(entry, head, stop, bar, ink, cx),
                Kind::Window | Kind::Loose => self.window_row(entry, stop, bar, ink),
            });
        }
        let moved = cx.entity().downgrade();
        crate::a11y::roving(
            div().id("rail-rows").control(Role::TabList, "Programs"),
            &self.stop,
            accesskit::Orientation::Vertical,
            [active, keys.len()],
            move |to, _, cx| {
                let row = keys[to];
                let _ = moved.update(cx, |this, cx| {
                    this.rail_cursor = Some(row);
                    cx.notify();
                });
            },
        )
        .flex_1()
        .min_h_0()
        .py(px(8.))
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .children(rows)
        .when(entries.is_empty(), |list| {
            list.child(
                sans(400, 13.)
                    .px(px(14.))
                    .text_color(ink.muted)
                    .child("No programs listed yet"),
            )
        })
    }

    #[allow(clippy::too_many_arguments, reason = "one row, read off the bar")]
    fn program_row(
        &self,
        entry: &Entry,
        head: bool,
        stop: Option<&FocusHandle>,
        bar: &Bar,
        ink: &Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let program = entry.program;
        let Some(row) = bar.rows.iter().find(|row| row.program == program) else {
            return div().into_any_element();
        };
        let badge = self.rail.read(cx).badge(program);
        let on = entry.on(bar.front);
        let open = head || bar.listed.iter().any(|it| it.program == program);
        let hover = ink.ink;
        let tab = crate::a11y::roving_item(
            sans(if head { 500 } else { 400 }, 13.).id(entry.id()),
            stop,
            ink.ink,
        )
        .control(
            Role::Tab,
            SharedString::from(accessible_program_name_with_status_and_unread(row, badge)),
        )
        .aria_selected(on)
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(14.))
        .cursor_pointer()
        .text_color(match open {
            true => ink.ink,
            false => ink.muted,
        })
        .hover(move |style| style.text_color(hover));
        let tab = self
            .opening(tab, program)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(entry.label.clone()),
            )
            .when(row.note == Some("Failed"), |tab| {
                tab.child(
                    div()
                        .size(px(5.))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(ink.danger),
                )
            })
            .when(badge > 0, |tab| {
                tab.child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .child(badge.to_string()),
                )
            });
        let plus = (self.hovered == Some(program)).then(|| {
            let desk = self.window.clone();
            let surface = ink.surface;
            div()
                .id(SharedString::from(format!(
                    "rail/{}/split",
                    crate::runtime::slug(program)
                )))
                .control(
                    Role::Button,
                    SharedString::from(format!("Open another {} window", entry.label)),
                )
                .size(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(ink.muted)
                .cursor_pointer()
                .hover(move |style| style.bg(surface))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = desk.update(cx, |desk, cx| {
                        desk.pane_message(PaneMessage::Split(program), window, cx)
                    });
                })
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Plus).size(px(14.)),
                )
        });
        div()
            .id(SharedString::from(format!(
                "rail/{}/row",
                crate::runtime::slug(program)
            )))
            .h(px(30.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pr(px(8.))
            .when(on, |row| row.bg(ink.raised))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let now = match hovered {
                    true => Some(program),
                    false => this.hovered.filter(|it| *it != program),
                };
                if this.hovered != now {
                    this.hovered = now;
                    cx.notify();
                }
            }))
            .child(tab)
            .children(plus)
            .into_any_element()
    }

    fn window_row(
        &self,
        entry: &Entry,
        stop: Option<&FocusHandle>,
        bar: &Bar,
        ink: &Ink,
    ) -> AnyElement {
        let Row::Window(instance) = entry.row else {
            return div().into_any_element();
        };
        let under = entry.kind == Kind::Window;
        let (desk, windows, away) = (self.window.clone(), self.windows.clone(), entry.away);
        crate::a11y::roving_item(sans(400, 13.).id(entry.id()), stop, ink.ink)
            .control(Role::Tab, SharedString::from(entry.label.clone()))
            .aria_selected(entry.on(bar.front))
            .h(px(if under { 28. } else { 30. }))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(px(if under { 28. } else { 14. }))
            .pr(px(8.))
            .cursor_pointer()
            .text_color(ink.ink)
            .when(entry.on(bar.front), |row| row.bg(ink.raised))
            .when(away.is_some(), |row| {
                row.aria_description("In a window of its own")
            })
            .on_click(move |_, window, cx| {
                if let Some(away) = away {
                    return windows.update(cx, |windows, cx| windows.raise(away, cx));
                }
                let _ = desk.update(cx, |desk, cx| {
                    let at = desk
                        .layout(cx)
                        .panes
                        .iter()
                        .position(|pane| pane.instance == instance);
                    if let Some(index) = at {
                        desk.pane_message(PaneMessage::Focus(index), window, cx);
                    }
                });
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(entry.label.clone()),
            )
            .when(away.is_some(), |row| {
                row.child(
                    div().flex_shrink_0().text_color(ink.muted).child(
                        gpui_kit::component::Icon::new(
                            gpui_kit::assets::IconName::SquareArrowOutUpRight,
                        )
                        .size(px(12.)),
                    ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Kind, Row, index, window_label};
    use crate::runtime::{RailRow, WindowKey};
    use crate::shell::entities::Listed;
    use crate::ui::layout::{EMPTY, HELP};

    fn row(program: &'static str, label: &str) -> RailRow {
        RailRow {
            program,
            label: label.into(),
            icon: String::new(),
            tint: view_wire::manifest::Tint::None,
            description: String::new(),
            note: None,
            empty: false,
            pending: false,
            prep: crate::runtime::Prep::Settled,
        }
    }

    fn window(program: &'static str, instance: u64, title: Option<&str>) -> Listed {
        Listed {
            program,
            instance,
            title: title.map(Into::into),
            away: None,
        }
    }

    fn said(entries: &[Entry]) -> Vec<(u8, &str, Row)> {
        entries
            .iter()
            .map(|entry| {
                let indent = match entry.kind {
                    Kind::Program { .. } => 0,
                    Kind::Window => 1,
                    Kind::Loose => 2,
                };
                (indent, entry.label.as_str(), entry.row)
            })
            .collect()
    }

    #[test]
    fn a_window_row_says_its_title_else_its_counted_program() {
        assert_eq!(window_label("Chat", Some("# general"), None), "# general");
        assert_eq!(window_label("Members", None, None), "Members");
        assert_eq!(window_label("Members", None, Some(1)), "Members");
        assert_eq!(window_label("Members", None, Some(2)), "Members 2");
        assert_eq!(window_label("Chat", Some("# design"), Some(3)), "# design");
    }

    #[test]
    fn a_program_lists_its_windows_when_it_has_two_or_a_titled_one() {
        let rows = [
            row("nodes", "Nodes"),
            row("members", "Members"),
            row("chat", "Chat"),
            row("forge", "Forge"),
            row("account", "Account"),
            RailRow {
                empty: true,
                ..row("headless", "Headless")
            },
        ];
        let open = [
            window("chat", 1, None),
            window(HELP, 2, None),
            window("members", 3, None),
            window("forge", 4, Some("website")),
            window("chat", 5, Some("# general")),
            window(EMPTY, 6, None),
            window("chat", 7, None),
            window("account", 8, None),
            window("account", 9, None),
            window("gone", 10, None),
        ];
        assert_eq!(
            said(&index(&rows, &open)),
            [
                (0, "Nodes", Row::Program("nodes")),
                (0, "Members", Row::Program("members")),
                (0, "Chat", Row::Program("chat")),
                (1, "Chat", Row::Window(1)),
                (1, "# general", Row::Window(5)),
                (1, "Chat 2", Row::Window(7)),
                (0, "Forge", Row::Program("forge")),
                (1, "website", Row::Window(4)),
                (0, "Account", Row::Program("account")),
                (1, "Account", Row::Window(8)),
                (1, "Account 2", Row::Window(9)),
                (2, "Help", Row::Window(2)),
                (2, "New window", Row::Window(6)),
                (2, "gone", Row::Window(10)),
            ]
        );
        let kinds: Vec<Kind> = index(&rows, &open)
            .iter()
            .filter(|entry| matches!(entry.row, Row::Program(_)))
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                Kind::Program {
                    head: false,
                    alone: None
                },
                Kind::Program {
                    head: false,
                    alone: Some(3)
                },
                Kind::Program {
                    head: true,
                    alone: None
                },
                Kind::Program {
                    head: true,
                    alone: None
                },
                Kind::Program {
                    head: true,
                    alone: None
                },
            ],
            "nothing, standing for its one window, three heads"
        );
        let open = [window("forge", 4, None)];
        assert_eq!(
            said(&index(&rows[3..4], &open)),
            [(0, "Forge", Row::Program("forge"))]
        );
    }

    #[test]
    fn a_pop_out_is_listed_under_its_program_after_the_desk() {
        let rows = [row("chat", "Chat"), row("forge", "Forge")];
        let away = |listed: Listed, key: u64| Listed {
            away: Some(WindowKey(key)),
            ..listed
        };
        let open = [
            window("chat", 1, None),
            away(window("chat", 2, None), 7),
            away(window("forge", 3, None), 8),
            away(window("gone", 4, None), 9),
        ];
        let entries = index(&rows, &open);
        assert_eq!(
            said(&entries),
            [
                (0, "Chat", Row::Program("chat")),
                (1, "Chat", Row::Window(1)),
                (1, "Chat 2", Row::Window(2)),
                (0, "Forge", Row::Program("forge")),
                (1, "Forge", Row::Window(3)),
                (2, "gone", Row::Window(4)),
            ]
        );
        let marked: Vec<Option<WindowKey>> = entries.iter().map(|entry| entry.away).collect();
        assert_eq!(
            marked,
            [
                None,
                None,
                Some(WindowKey(7)),
                None,
                Some(WindowKey(8)),
                Some(WindowKey(9)),
            ]
        );
    }
}
