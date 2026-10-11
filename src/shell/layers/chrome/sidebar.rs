//! The sidebar (the Sidebar layout; the mock's `index-desk`): the chrome
//! as a column down the console's left, `SIDEBAR` wide. Top to bottom: the
//! network, Search, a hairline, the programs, each with the windows it has
//! open on the desk listed under it, then after a hairline the windows no
//! program row stands for (Help, empty windows); and a footer with the
//! bell, the node's breath, who is signed in, and the gear. Its buttons
//! and menus are the bar's (`Chrome`'s), its menus hung beside the column
//! (`Chrome::hanging`). The list is one Tab stop the arrows move along
//! (`a11y::roving`), as the bar's tabs are.

use super::{Bar, Chrome, FOOT, Row, drag_handle, program_name, tab_label};
use crate::a11y::Control as _;
use crate::runtime::RailRow;
use crate::shell::entities::Listed;
use crate::shell::ink::{Ink, mono, sans};
use crate::shell::{PaneMessage, layout, theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// What a window's row says: its title, else its program's label,
/// counted from the second (`Members 2`) when it is the `ordinal`th
/// untitled window of its program listed.
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

/// One row of the sidebar's list.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// What the keys and a click reach.
    row: Row,
    module: &'static str,
    /// What it says.
    label: String,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A program the roster lists. `head`: its windows are listed under
    /// it; else `alone`, its one window, which its row stands for.
    Program { head: bool, alone: Option<u64> },
    /// One of a program's windows, under it.
    Window,
    /// After the hairline: Help, an empty window, or a window of a program
    /// the roster does not list.
    Loose,
}

/// The sidebar's list: one row per program the roster lists with a view,
/// in roster order, each followed by its windows in desk order when it has
/// two or more, or one with a title; then the windows none of them lists,
/// in desk order. A window's row says its title, else its program's name,
/// counted among the program's untitled windows (`window_label`).
fn index(rows: &[RailRow], open: &[Listed]) -> Vec<Entry> {
    let mut untitled: Vec<&'static str> = Vec::new();
    let mut said = |label: &str, window: &Listed| {
        let ordinal = window.title.is_none().then(|| {
            untitled.push(window.module);
            untitled.iter().filter(|it| **it == window.module).count()
        });
        window_label(label, window.title.as_deref(), ordinal)
    };
    let mut entries = Vec::new();
    let rows: Vec<&RailRow> = rows.iter().filter(|row| !row.empty).collect();
    for row in &rows {
        let label = tab_label(row);
        let windows: Vec<&Listed> = open.iter().filter(|it| it.module == row.module).collect();
        let head = windows.len() > 1 || windows.iter().any(|it| it.title.is_some());
        entries.push(Entry {
            row: Row::Program(row.module),
            module: row.module,
            label: label.clone(),
            kind: Kind::Program {
                head,
                alone: windows.first().filter(|_| !head).map(|it| it.instance),
            },
        });
        if head {
            for window in windows {
                entries.push(Entry {
                    row: Row::Window(window.instance),
                    module: window.module,
                    label: said(&label, window),
                    kind: Kind::Window,
                });
            }
        }
    }
    for window in open
        .iter()
        .filter(|it| !rows.iter().any(|row| row.module == it.module))
    {
        let label = match window.module {
            layout::EMPTY => "New window",
            layout::HELP => "Help",
            module => module,
        };
        entries.push(Entry {
            row: Row::Window(window.instance),
            module: window.module,
            label: said(label, window),
            kind: Kind::Loose,
        });
    }
    entries
}

impl Entry {
    /// Its element id: `rail/<module>` for a program, `rail/<module>/<instance>`
    /// for a window under it, `rail-help/<instance>` and `rail-new/<instance>`
    /// for Help and an empty window.
    fn id(&self) -> SharedString {
        let id = match (self.row, self.module) {
            (Row::Program(module), _) => format!("rail/{module}"),
            (Row::Window(instance), layout::HELP) => format!("rail-help/{instance}"),
            (Row::Window(instance), layout::EMPTY) => format!("rail-new/{instance}"),
            (Row::Window(instance), module) => format!("rail/{module}/{instance}"),
        };
        id.into()
    }

    /// The window in front is the one it stands for.
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
    /// The sidebar (the mock's `.side`): `width: 220px; border-right: 1px
    /// solid line; font: 400 13px`.
    pub(super) fn sidebar(
        &mut self,
        bar: Bar,
        ink: &Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // the bar is measured again when it comes back: it never folds here
        self.bar_drawn = None;
        // the network's row (`.srow.net`: `height: 36px; padding: 0 14px`),
        // on macOS clear of the traffic lights and the window's handle
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
        // `.srow.search`: the word muted, the chord at the far end
        let search = self
            .search(false, ink)
            .w_full()
            .px(px(14.))
            .justify_between();
        let line = || div().h(px(1.)).flex_shrink_0().bg(ink.line);
        // the footer (`.sfoot`: `height: 40px; padding: 0 4px 0 6px`, a
        // hairline above), each button `height: 40px; padding: 0 8px`, its
        // menu opening upward from it
        let foot = |(button, menu): (Stateful<Div>, Option<AnyElement>)| {
            Self::footed(button.h_full().px(px(8.)), menu)
        };
        let footer = div()
            .h(px(FOOT))
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
            .role(Role::MenuBar)
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

    /// The programs and their windows (`.mods`: `padding: 8px 0`, scrolling
    /// when long): one Tab stop, on the row the arrows moved to while it has
    /// the keys, else the window in front's, else the first; up and down
    /// move the keys along it, Return opens what the row stands for.
    fn list(&self, bar: &Bar, ink: &Ink, cx: &mut Context<Self>) -> Stateful<Div> {
        let entries = index(&bar.rows, &bar.on_desk);
        let keys: Vec<Row> = entries.iter().map(|entry| entry.row).collect();
        let active = self
            .rail_cursor
            .and_then(|at| keys.iter().position(|row| *row == at))
            .or_else(|| entries.iter().position(|entry| entry.on(bar.front)))
            .unwrap_or_default();
        // room on the desk for another window: the `+` a program's row shows
        let room = bar.on_desk.len() < layout::MAX_PANES;
        let mut rows: Vec<AnyElement> = Vec::new();
        for (n, entry) in entries.iter().enumerate() {
            if entry.kind == Kind::Loose && n > 0 && entries[n - 1].kind != Kind::Loose {
                rows.push(div().h(px(1.)).my(px(8.)).bg(ink.line).into_any_element());
            }
            let stop = (n == active).then_some(&self.stop);
            rows.push(match entry.kind {
                Kind::Program { head, .. } => {
                    self.program_row(entry, head, stop, room, bar, ink, cx)
                }
                Kind::Window | Kind::Loose => self.window_row(entry, stop, bar, ink),
            });
        }
        let moved = cx.entity().downgrade();
        crate::a11y::roving(
            div().id("rail-rows").control(Role::TabList, "Programs"),
            &self.stop,
            accesskit::Orientation::Vertical,
            [active, keys.len()],
            // the arrows move the keys; Return opens what the row stands for
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

    /// A program's row (`.mod`: `height: 30px; padding: 0 8px 0 14px`):
    /// muted, ink while it has a window on the desk, ink 500 over its
    /// listed windows, `raised` while it stands for the window in front;
    /// its unread count and its failed dot at the right, and under the
    /// pointer a `+` that opens another window of it. A press is the bar's
    /// tab's (`opening`). The `+` sits beside the tab, not in it: a tab's
    /// press is the whole of it (AX-119), and the list stays one Tab stop.
    #[allow(clippy::too_many_arguments, reason = "one row, read off the bar")]
    fn program_row(
        &self,
        entry: &Entry,
        head: bool,
        stop: Option<&FocusHandle>,
        room: bool,
        bar: &Bar,
        ink: &Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let module = entry.module;
        let Some(row) = bar.rows.iter().find(|row| row.module == module) else {
            return div().into_any_element();
        };
        let badge = self.rail.read(cx).badge(module);
        let on = entry.on(bar.front);
        let open = head || bar.on_desk.iter().any(|it| it.module == module);
        let hover = ink.ink;
        let tab = crate::a11y::roving_item(
            sans(if head { 500 } else { 400 }, 13.).id(entry.id()),
            stop,
            ink.ink,
        )
        .control(Role::Tab, SharedString::from(program_name(row, badge)))
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
            .opening(tab, module)
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
        let plus = (room && self.hovered == Some(module)).then(|| {
            let desk = self.window.clone();
            let surface = ink.surface;
            div()
                .id(SharedString::from(format!("rail/{module}/split")))
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
                        desk.pane_message(PaneMessage::Split(module), window, cx)
                    });
                })
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Plus).size(px(14.)),
                )
        });
        div()
            .id(SharedString::from(format!("rail/{module}/row")))
            .h(px(30.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pr(px(8.))
            .when(on, |row| row.bg(ink.raised))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let now = match hovered {
                    true => Some(module),
                    false => this.hovered.filter(|it| *it != module),
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

    /// A window's row: under its program (`.tb`: `height: 28px;
    /// padding-left: 28px`), or after the hairline (`.mod.open`); ink,
    /// `raised` while it is the window in front. A press brings that window
    /// to the front, found by its instance then (a close moves the others'
    /// places).
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
        let desk = self.window.clone();
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
            .on_click(move |_, window, cx| {
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
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, Kind, Row, index, window_label};
    use crate::runtime::RailRow;
    use crate::shell::entities::Listed;
    use crate::ui::layout::{EMPTY, HELP};

    fn row(module: &'static str, label: &str) -> RailRow {
        RailRow {
            module,
            label: label.into(),
            icon: String::new(),
            note: None,
            empty: false,
        }
    }

    fn window(module: &'static str, instance: u64, title: Option<&str>) -> Listed {
        Listed {
            module,
            instance,
            title: title.map(Into::into),
        }
    }

    /// Each row as `(indent, what it says, what it reaches)`: 0 a program,
    /// 1 a window under it, 2 after the hairline.
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

    /// The fold rule (SPEC §5): a program with no window, or one untitled
    /// window, is its row alone (the second standing for that window); one
    /// titled window, or two, are listed under it in desk order, the
    /// untitled counted from the second. Help and empty windows, and a
    /// window of a program the roster does not list, come after, in desk
    /// order; a program that ships no view is not listed.
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
        // the titled window closed: the program is its row alone again
        let open = [window("forge", 4, None)];
        assert_eq!(
            said(&index(&rows[3..4], &open)),
            [(0, "Forge", Row::Program("forge"))]
        );
    }
}
