//! An empty window's body: one field that finds a program to open in it,
//! with a switch beside it for what the field searches (Module | Chat).
//! Until agent chat is built ([`CHAT_READY`]), Tab there moves focus.

use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};

/// The key context of an empty window's body: Tab there switches modes,
/// once chat is built.
pub(super) const CONTEXT: &str = "EmptyWindow";

/// Agent chat isn't built yet: its side of the switch shows, blocked, and
/// Tab is left to move focus. True brings back what switches to it: the
/// `tab` binding (`keys::bind`), the "tab switch" hint and Help's line.
pub(super) const CHAT_READY: bool = false;

/// What an empty window's field searches.
#[derive(Clone, Copy, Default, PartialEq)]
enum Mode {
    #[default]
    Module,
    Chat,
}

impl Mode {
    /// Tab: the other side, while it is there to go to.
    fn toggled(self) -> Self {
        match self {
            Mode::Module if CHAT_READY => Mode::Chat,
            _ => Mode::Module,
        }
    }
}

/// The command line's state: the field, the picked row and the mode. Only a
/// window's focused empty pane shows it.
pub(super) struct CommandLine {
    field: Entity<InputState>,
    pick: usize,
    mode: Mode,
    _changed: gpui_kit::Subscription,
}

/// The programs an empty window offers whose names hold `query`.
pub(super) fn matching(
    rows: Vec<crate::runtime::RailRow>,
    query: &str,
) -> Vec<crate::runtime::RailRow> {
    let query = query.trim().to_lowercase();
    rows.into_iter()
        .filter(|row| {
            super::menubar::tab_label(row)
                .to_lowercase()
                .contains(&query)
        })
        .collect()
}

impl DesktopWindow {
    fn command(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &mut CommandLine {
        self.command.get_or_insert_with(|| {
            let field = cx.new(|cx| InputState::new(window, cx).placeholder("Open a program"));
            let changed = cx.subscribe_in(&field, window, |this, _, event, _, cx| {
                if let InputEvent::Change = event {
                    if let Some(command) = this.command.as_mut() {
                        command.pick = 0;
                    }
                    cx.notify();
                }
            });
            CommandLine {
                field,
                pick: 0,
                mode: Mode::default(),
                _changed: changed,
            }
        })
    }

    /// The field takes the keys when its window has them and nothing in it
    /// does: typing starts at once.
    pub(super) fn focus_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.command(window, cx).field.clone();
        window.defer(cx, move |window, cx| {
            field.update(cx, |field, cx| field.focus(window, cx))
        });
    }

    /// A program opened in the empty window: the next one starts blank.
    pub(super) fn clear_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.command.as_mut() {
            command.pick = 0;
            command
                .field
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// An empty window's body: the field and its switch, the programs its
    /// text matches, and the keys. Only the focused one holds the field.
    pub(super) fn command_view(
        &mut self,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let state = self.model.read(cx).state.facts();
        let ink = Ink::of(state.dark);
        let command = self.command(window, cx);
        let (field, mode) = (command.field.clone(), command.mode);
        let query = match focused {
            true => field.read(cx).value().to_string(),
            false => String::new(),
        };
        let rows = matching(super::panes::openable(), &query);
        let pick = command.pick.min(rows.len().saturating_sub(1));
        command.pick = pick;
        let bar = div()
            .h(px(tall(52.)))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .pl(px(16.))
            .pr(px(8.))
            .border(px(1.5))
            .border_color(match focused {
                true => ink.ink,
                false => ink.line,
            })
            .child(
                div().flex_1().min_w_0().child(match focused {
                    true => self.command_field(&field, &query, cx),
                    false => sans(400, 17.)
                        .text_color(ink.muted)
                        .child("Open a program")
                        .into_any_element(),
                }),
            )
            .child(self.mode_switch(mode, &ink, cx));
        let modules: Vec<&'static str> = rows.iter().map(|row| row.module).collect();
        let list = rows.iter().enumerate().map(|(nth, row)| {
            let module = row.module;
            let picked = focused && nth == pick;
            let name = super::menubar::tab_label(row);
            let note = match state.badges.get(module).copied().unwrap_or(0) {
                0 => String::new(),
                count => format!("{count} unread"),
            };
            sans(400, 15.)
                .id(SharedString::from(format!("empty/{module}")))
                .control(Role::MenuItem, SharedString::from(name.clone()))
                .flex()
                .items_baseline()
                .gap(px(12.))
                .px(px(16.))
                .py(px(10.))
                .cursor_pointer()
                .when(picked, |row| row.bg(ink.surface))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_view(module, window, cx);
                }))
                .child(div().text_color(ink.ink).child(name))
                .child(
                    sans(400, 13.)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.muted)
                        .child(note),
                )
                .child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .child(if picked { "↵" } else { "" }),
                )
        });
        let found = !modules.is_empty();
        let hints = [
            "↑↓ pick".to_owned(),
            "↵ open".to_owned(),
            "tab switch".to_owned(),
            format!("{} search everything", chord_label("K")),
        ]
        .into_iter()
        .filter(|hint| CHAT_READY || hint != "tab switch")
        .map(|hint| div().whitespace_nowrap().child(hint));
        div()
            .id("empty-window")
            .role(Role::Menu)
            .aria_label("Open in this window")
            .size_full()
            .flex()
            .justify_center()
            .px(px(24.))
            .pt(px(40.))
            .pb(px(16.))
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &super::keys::SwitchMode, _, cx| {
                if let Some(command) = this.command.as_mut() {
                    command.mode = command.mode.toggled();
                }
                cx.notify();
            }))
            .when(focused, |body| {
                body.capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    let Some(command) = this.command.as_mut() else {
                        return;
                    };
                    let last = modules.len().saturating_sub(1);
                    match event.keystroke.key.as_str() {
                        "up" => command.pick = command.pick.saturating_sub(1),
                        "down" => command.pick = (command.pick + 1).min(last),
                        "enter" => match modules.get(command.pick.min(last)) {
                            Some(module) => this.open_view(module, window, cx),
                            None => return,
                        },
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }))
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(560.))
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(bar)
                    .child(
                        div()
                            .id("empty-window/rows")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .border_1()
                            .border_t_0()
                            .border_color(ink.line)
                            .children(list)
                            .when(!found, |rows| {
                                rows.child(
                                    sans(400, 13.)
                                        .px(px(16.))
                                        .py(px(12.))
                                        .text_color(ink.muted)
                                        .child("Nothing here by that name."),
                                )
                            }),
                    )
                    .child(
                        mono(400, 12.)
                            .flex_shrink_0()
                            .pt(px(12.))
                            .flex()
                            .flex_wrap()
                            .gap_x(px(20.))
                            .gap_y(px(4.))
                            .text_color(ink.muted)
                            .children(hints),
                    ),
            )
            .into_any_element()
    }

    fn command_field(
        &self,
        field: &Entity<InputState>,
        query: &str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::{Focusable as _, StatefulInteractiveElement as _};
        let input = Input::new(field)
            .appearance(false)
            .text_size(gpui_kit::px(super::ink::fit(17.)))
            .line_height(gpui_kit::relative(1.4))
            .py_0()
            .px_0();
        crate::a11y::text_field(
            "empty-window/field",
            &field.read(cx).focus_handle(cx),
            {
                let field = field.clone();
                move |value, window, cx| {
                    field.update(cx, |field, cx| field.replace_all(value, window, cx))
                }
            },
            input.role(gpui_kit::component::RoleOverride::Presentational),
        )
        .aria_label("Open a program")
        .aria_value(query.to_owned())
        .role(gpui_kit::Role::TextInput)
        .into_any_element()
    }

    /// Module | Chat, Tab between them. Chat is blocked until agent chat
    /// is built, and says so on hover.
    fn mode_switch(
        &self,
        mode: Mode,
        ink: &super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let side = |id: &'static str, name: &'static str, on: bool| {
            sans(500, 13.)
                .id(id)
                .control(Role::Button, SharedString::from(name))
                .aria_toggled(on.into())
                .h_full()
                .px(px(10.))
                .flex()
                .items_center()
                .when(on, |side| side.bg(ink.ink).text_color(ink.bg))
                .when(!on, |side| side.text_color(ink.ink))
                .child(name)
        };
        let module = side("empty-window/module", "Module", mode == Mode::Module).on_click(
            cx.listener(|this, _, _, cx| {
                if let Some(command) = this.command.as_mut() {
                    command.mode = Mode::Module;
                }
                cx.notify();
            }),
        );
        let chat = side("empty-window/chat", "Chat", mode == Mode::Chat)
            .opacity(0.3)
            .cursor_not_allowed()
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Soon").build(window, cx)
            });
        div()
            .h(px(tall(34.)))
            .flex_shrink_0()
            .flex()
            .border_1()
            .border_color(ink.ink)
            .child(module)
            .child(chat.aria_disabled(true))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::matching;
    use crate::runtime::RailRow;

    fn row(module: &'static str, label: &str) -> RailRow {
        RailRow {
            module,
            label: label.into(),
            note: None,
            empty: false,
        }
    }

    #[test]
    fn the_field_keeps_the_programs_whose_names_hold_its_text() {
        let rows = || {
            vec![
                row("chat", "Chat"),
                row("forge", "Forge"),
                row("members", "Members"),
            ]
        };
        let modules = |query| -> Vec<_> {
            matching(rows(), query)
                .into_iter()
                .map(|row| row.module)
                .collect()
        };
        assert_eq!(modules(""), ["chat", "forge", "members"]);
        assert_eq!(modules("  "), ["chat", "forge", "members"]);
        assert_eq!(modules("OR"), ["forge"]);
        assert_eq!(modules("e"), ["forge", "members"]);
        assert!(modules("zz").is_empty());
    }
}
