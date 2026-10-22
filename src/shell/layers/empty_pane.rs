//! A pane with no program in it, drawn by the app as a view of its own so
//! its body is cached apart from the window around it. Two places have
//! one: the desk itself while no window is on it (a figure, and either why
//! the network has nothing to open or the ways to open something), and an
//! empty window on the desk (one field that finds a program to open in it,
//! with a switch beside it for what the field searches, Program | Chat;
//! until agent chat is built, [`CHAT_READY`], Tab there moves focus).

use super::super::entities::{Desk, Entities, Overlays, Prefs, Rail, Slice};
use super::super::figure::Figure;
use super::super::ink::{self, Ink};
use super::super::spin::{self, Spin};
use super::super::{PaneMessage, WindowKey, WindowKind, WindowRoot, chord_label};
use crate::a11y::Control as _;
use crate::a11y::Named as _;
use crate::runtime::RailRow;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;

pub(in crate::shell) const CONTEXT: &str = "EmptyWindow";

pub(in crate::shell) const CHAT_READY: bool = false;

#[derive(Clone, Copy, Default, PartialEq)]
enum Mode {
    #[default]
    Program,
    Chat,
}

impl Mode {
    fn toggled(self) -> Self {
        match self {
            Mode::Program if CHAT_READY => Mode::Chat,
            _ => Mode::Program,
        }
    }
}

struct CommandLine {
    field: Entity<InputState>,
    pick: usize,
    mode: Mode,
    _changed: Subscription,
    _box_focused: Subscription,
}

fn matching(rows: Vec<RailRow>, query: &str) -> Vec<RailRow> {
    let query = query.trim().to_lowercase();
    rows.into_iter()
        .filter(|row| super::tab_label(row).to_lowercase().contains(&query))
        .collect()
}

fn empty_panes_message(rail: &[RailRow]) -> Option<&'static str> {
    (!rail.iter().any(|row| !row.empty)).then_some("This network runs no program with a view.")
}

enum Place {
    Desk {
        spin: Entity<Spin>,
        root: FocusHandle,
        console: bool,
        bare: bool,
    },
    Window {
        instance: u64,
        desk: WeakEntity<WindowRoot>,
        command: CommandLine,
    },
}

#[derive(Default, PartialEq)]
struct Shown {
    dark: bool,
    rail: Vec<RailRow>,
    badges: BTreeMap<&'static str, i64>,
    at: Option<(usize, bool)>,
}

pub(in crate::shell) struct EmptyPane {
    key: WindowKey,
    desk: Entity<Desk>,
    overlays: Entity<Overlays>,
    rail: Entity<Rail>,
    prefs: Entity<Slice<Prefs>>,
    place: Place,
    shown: Shown,
    _observing: [Subscription; 3],
}

impl EmptyPane {
    pub(in crate::shell) fn desk(
        app: &Entities,
        key: WindowKey,
        kind: WindowKind,
        root: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (dark, motion, bare) = {
            let prefs = app.prefs.read(cx).get();
            let layout = app
                .windows
                .read(cx)
                .own(key)
                .expect("its window")
                .desk
                .read(cx)
                .get();
            (prefs.dark(), prefs.motion, layout.panes.is_empty())
        };
        let spin = cx.new(|cx| Spin::new(Figure::Roll, motion, Ink::of(dark).figure, cx));
        let console = kind == WindowKind::Console;
        let place = Place::Desk {
            spin,
            root,
            console,
            bare,
        };
        Self::made(app, key, place, window, cx)
    }

    pub(in crate::shell) fn window(
        app: &Entities,
        key: WindowKey,
        instance: u64,
        desk: WeakEntity<WindowRoot>,
        own: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Open a program"));
        let changed = cx.subscribe_in(&field, window, |this, _, event, _, cx| {
            if let InputEvent::Change = event {
                if let Some(command) = this.command() {
                    command.pick = 0;
                }
                cx.notify();
            }
        });
        let box_focused = cx.on_focus(own, window, |this, window, cx| {
            this.box_took_the_keys(window, cx)
        });
        let command = CommandLine {
            field,
            pick: 0,
            mode: Mode::default(),
            _changed: changed,
            _box_focused: box_focused,
        };
        let place = Place::Window {
            instance,
            desk,
            command,
        };
        Self::made(app, key, place, window, cx)
    }

    fn made(
        app: &Entities,
        key: WindowKey,
        place: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let own = app.windows.read(cx).own(key).expect("its window").clone();
        let (desk, overlays) = (own.desk.clone(), own.overlays);
        let (rail, prefs) = (app.rail.clone(), app.prefs.clone());
        let observing = [
            cx.observe_in(&desk, window, |this, _, window, cx| this.moved(window, cx)),
            cx.observe_in(&rail, window, |this, _, window, cx| this.moved(window, cx)),
            cx.observe_in(&prefs, window, |this, _, window, cx| this.moved(window, cx)),
        ];
        let mut this = Self {
            key,
            desk,
            overlays,
            rail,
            prefs,
            place,
            shown: Shown::default(),
            _observing: observing,
        };
        this.shown = this.shown(cx);
        this
    }

    fn shown(&self, cx: &App) -> Shown {
        let rail = self.rail.read(cx);
        Shown {
            dark: self.prefs.read(cx).get().dark(),
            rail: rail.rows().to_vec(),
            badges: rail.badges().clone(),
            at: self.at(cx),
        }
    }

    fn at(&self, cx: &App) -> Option<(usize, bool)> {
        let Place::Window { instance, .. } = &self.place else {
            return None;
        };
        let layout = self.desk.read(cx).get();
        let index = layout
            .panes
            .iter()
            .position(|pane| pane.instance == *instance)?;
        Some((index, index == layout.focused))
    }

    fn moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (dark, motion, now_bare, overlay) = {
            let prefs = self.prefs.read(cx).get();
            (
                prefs.dark(),
                prefs.motion,
                self.desk.read(cx).get().panes.is_empty(),
                self.overlays.read(cx).get().is_some(),
            )
        };
        if let Place::Desk {
            spin,
            root,
            console,
            bare,
        } = &mut self.place
        {
            spin.update(cx, |spin, cx| {
                spin.set(Figure::Roll, motion, Ink::of(dark).figure, cx)
            });
            if now_bare && !*bare && !(*console && overlay) {
                root.focus(window, cx);
            }
            *bare = now_bare;
            if !now_bare {
                return;
            }
        }
        let shown = self.shown(cx);
        if shown != self.shown {
            self.shown = shown;
            cx.notify();
        }
    }

    fn box_took_the_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Place::Window {
            instance, command, ..
        } = &self.place
        else {
            return;
        };
        let layout = self.desk.read(cx).get();
        let front = layout
            .panes
            .get(layout.focused)
            .is_some_and(|pane| pane.instance == *instance);
        if front && layout.held.is_none() {
            command
                .field
                .update(cx, |field, cx| field.focus(window, cx));
        }
    }

    fn command(&mut self) -> Option<&mut CommandLine> {
        match &mut self.place {
            Place::Window { command, .. } => Some(command),
            Place::Desk { .. } => None,
        }
    }

    fn on_desk(&self, cx: &mut App, act: impl FnOnce(&mut WindowRoot, &mut Context<WindowRoot>)) {
        if let Place::Window { desk, .. } = &self.place {
            let _ = desk.update(cx, act);
        }
    }

    fn open_here(&self, index: usize, program: &'static str, window: &mut Window, cx: &mut App) {
        self.on_desk(cx, |desk, cx| desk.open_here(index, program, window, cx));
    }

    fn desk_body(spin: &Entity<Spin>, rail: &[RailRow], ink: &Ink) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(20.))
            .child(spin::drawing(spin))
            .child(match empty_panes_message(rail) {
                Some(message) => ink::mono(400, 12.)
                    .text_color(ink.muted)
                    .child(ink::words("empty-desk/message", message))
                    .into_any_element(),
                None => div()
                    .flex()
                    .gap(px(12.))
                    .child(desk_button(
                        "empty-desk/new",
                        "N",
                        "New window",
                        ink,
                        || Box::new(super::super::keys::NewWindow),
                    ))
                    .child(desk_button("empty-desk/search", "K", "Search", ink, || {
                        Box::new(super::super::keys::ToggleSpotlight)
                    }))
                    .child(desk_button("empty-desk/help", "/", "Help", ink, || {
                        Box::new(super::super::keys::OpenHelp)
                    }))
                    .into_any_element(),
            })
            .into_any_element()
    }

    fn command_view(
        &mut self,
        index: usize,
        focused: bool,
        ink: &Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use ink::*;
        let rail: Vec<RailRow> = self
            .shown
            .rail
            .iter()
            .filter(|row| !row.empty)
            .cloned()
            .collect();
        let badges = self.shown.badges.clone();
        let Some(command) = self.command() else {
            return div().into_any_element();
        };
        let (field, mode) = (command.field.clone(), command.mode);
        let query = match focused {
            true => field.read(cx).value().to_string(),
            false => String::new(),
        };
        let rows = matching(rail, &query);
        let pick = command.pick.min(rows.len().saturating_sub(1));
        command.pick = pick;
        let typing = focused && field.read(cx).focus_handle(cx).is_focused(window);
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
                    true => command_field(&field, cx),
                    false => sans(400, 17.)
                        .text_color(ink.muted)
                        .child("Open a program")
                        .into_any_element(),
                }),
            )
            .child(mode_switch(index, mode, focused, ink, cx));
        let bar = crate::a11y::around_field(bar, typing, ink.ring(false));
        let programs: Vec<&'static str> = rows.iter().map(|row| row.program).collect();
        let list = rows.iter().enumerate().map(|(nth, row)| {
            let program = row.program;
            let picked = focused && nth == pick;
            let name = super::tab_label(row);
            let note = match badges.get(program).copied().unwrap_or(0) {
                0 => String::new(),
                count => format!("{count} unread"),
            };
            sans(400, 15.)
                .id(SharedString::from(format!(
                    "empty/{}",
                    crate::runtime::slug(program)
                )))
                .control(Role::ListBoxOption, SharedString::from(name.clone()))
                .aria_selected(picked)
                .when(picked, |row| row.aria_active_descendant())
                .flex()
                .items_baseline()
                .gap(px(12.))
                .px(px(16.))
                .py(px(10.))
                .cursor_pointer()
                .when(picked, |row| row.bg(ink.surface))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_here(index, program, window, cx);
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
        let found = !programs.is_empty();
        let hints = [
            "↑↓ pick".to_owned(),
            "↵ open".to_owned(),
            "tab switch".to_owned(),
            format!("{} search everything", chord_label("K")),
        ]
        .into_iter()
        .filter(|hint| CHAT_READY || hint != "tab switch")
        .map(|hint| div().whitespace_nowrap().child(hint));
        let rows = div()
            .id("empty-window/rows")
            .control(Role::ListBox, "Programs")
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
            });
        let search = match focused {
            true => {
                crate::a11y::combo_box("empty-window/search", &field.read(cx).focus_handle(cx), {
                    let field = field.clone();
                    move |value, window, cx| {
                        field.update(cx, |field, cx| field.replace_all(value, window, cx))
                    }
                })
                .aria_label("Open a program")
                .aria_value(query)
                .aria_expanded(found)
            }
            false => div().id("empty-window/search"),
        };
        div()
            .id("empty-window")
            .role_named_by_id(Role::Group)
            .aria_label("Open in this window")
            .size_full()
            .flex()
            .justify_center()
            .px(px(24.))
            .pt(px(40.))
            .pb(px(16.))
            .key_context(CONTEXT)
            .on_action(
                cx.listener(|this, _: &super::super::keys::SwitchMode, _, cx| {
                    if let Some(command) = this.command() {
                        command.mode = command.mode.toggled();
                    }
                    cx.notify();
                }),
            )
            .when(focused, |body| {
                body.capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    let Some(command) = this.command() else {
                        return;
                    };
                    let last = programs.len().saturating_sub(1);
                    match event.keystroke.key.as_str() {
                        "up" => command.pick = command.pick.saturating_sub(1),
                        "down" => command.pick = (command.pick + 1).min(last),
                        "enter" => match programs.get(command.pick.min(last)) {
                            Some(&program) => {
                                this.on_desk(cx, |desk, cx| desk.open_view(program, window, cx))
                            }
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
                    .child(
                        search
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(bar)
                            .child(rows),
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
}

impl Render for EmptyPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.empty", 1);
        self.shown = self.shown(cx);
        let ink = Ink::of(self.shown.dark);
        if let Place::Desk { spin, .. } = &self.place {
            return Self::desk_body(spin, &self.shown.rail, &ink);
        }
        match self.shown.at {
            Some((index, focused)) => self.command_view(index, focused, &ink, window, cx),
            None => div().into_any_element(),
        }
    }
}

fn desk_button(
    id: &'static str,
    key: &str,
    name: &'static str,
    ink: &Ink,
    action: fn() -> Box<dyn Action>,
) -> AnyElement {
    use ink::*;
    let hover = ink.surface;
    let button = sans(500, 14.)
        .id(id)
        .control(Role::Button, SharedString::from(name))
        .h(px(tall(34.)))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .border_1()
        .border_color(ink.line)
        .text_color(ink.ink)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            window.dispatch_action(action(), cx);
        })
        .child(mono(400, 12.).text_color(ink.muted).child(chord_label(key)))
        .child(name);
    crate::a11y::keyboard(button, ink.ink)
        .aria_keyshortcuts(chord_label(key))
        .into_any_element()
}

fn command_field(field: &Entity<InputState>, cx: &mut App) -> AnyElement {
    let input = Input::new(field)
        .appearance(false)
        .text_size(px(ink::fit(17.)))
        .line_height(relative(1.4))
        .py_0()
        .px_0();
    let element = crate::a11y::text_field(
        "empty-window/field",
        &field.read(cx).focus_handle(cx),
        {
            let field = field.clone();
            move |value, window, cx| {
                field.update(cx, |field, cx| field.replace_all(value, window, cx))
            }
        },
        input.role(component::RoleOverride::Presentational),
    );
    let native = || super::fields::inspected(field, None, None, false, cx);
    crate::a11y::inspected(element, native).into_any_element()
}

fn mode_switch(
    index: usize,
    mode: Mode,
    focused: bool,
    ink: &Ink,
    cx: &mut Context<EmptyPane>,
) -> AnyElement {
    use ink::*;
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
    let pick = move |this: &mut EmptyPane, window: &mut Window, cx: &mut Context<EmptyPane>| {
        if !focused {
            this.on_desk(cx, |desk, cx| {
                desk.pane_message(PaneMessage::Focus(index), window, cx)
            });
        }
        if let Some(command) = this.command() {
            command.mode = Mode::Program;
        }
        cx.notify();
    };
    let program = side("empty-window/program", "Program", mode == Mode::Program)
        .on_click(cx.listener(move |this, _, window, cx| pick(this, window, cx)));
    let chat = side("empty-window/chat", "Chat", mode == Mode::Chat)
        .opacity(0.3)
        .cursor_not_allowed()
        .tooltip(|window, cx| component::tooltip::Tooltip::new("Soon").build(window, cx));
    div()
        .h(px(tall(34.)))
        .flex_shrink_0()
        .flex()
        .border_1()
        .border_color(ink.ink)
        .child(program)
        .child(crate::a11y::Patch::default().disabled().on(chat))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{empty_panes_message, matching};
    use crate::runtime::RailRow;

    fn row(program: &'static str, label: &str, empty: bool) -> RailRow {
        RailRow {
            program,
            label: label.into(),
            icon: String::new(),
            tint: view_wire::manifest::Tint::None,
            description: String::new(),
            note: None,
            empty,
            pending: false,
            prep: crate::runtime::Prep::Settled,
        }
    }

    #[test]
    fn the_field_keeps_the_programs_whose_names_hold_its_text() {
        let rows = || {
            vec![
                row("chat", "Chat", false),
                row("forge", "Forge", false),
                row("members", "Members", false),
            ]
        };
        let programs = |query| -> Vec<_> {
            matching(rows(), query)
                .into_iter()
                .map(|row| row.program)
                .collect()
        };
        assert_eq!(programs(""), ["chat", "forge", "members"]);
        assert_eq!(programs("  "), ["chat", "forge", "members"]);
        assert_eq!(programs("OR"), ["forge"]);
        assert_eq!(programs("e"), ["forge", "members"]);
        assert!(programs("zz").is_empty());
    }

    #[test]
    fn names_the_network_only_when_the_rail_itself_is_empty() {
        assert_eq!(
            empty_panes_message(&[]),
            Some("This network runs no program with a view.")
        );
        assert_eq!(
            empty_panes_message(&[row("chat", "Chat", true)]),
            Some("This network runs no program with a view."),
            "a rail of empty-slot rows still has nothing to open"
        );
        assert_eq!(
            empty_panes_message(&[row("chat", "Chat", true), row("chat", "Chat", false)]),
            None
        );
    }
}
