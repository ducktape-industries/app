//! A pane with no program in it, drawn by the app as a view of its own so
//! its body is cached apart from the window around it. Two places have
//! one: the desk itself while no window is on it (a figure, and either why
//! the network has nothing to open or the ways to open something), and an
//! empty window on the desk (one field that finds a program to open in it,
//! with a switch beside it for what the field searches, Module | Chat;
//! until agent chat is built, [`CHAT_READY`], Tab there moves focus).

use super::super::entities::{Desk, Entities, Overlays, Prefs, Rail, Slice};
use super::super::figure::Figure;
use super::super::ink::{self, Ink};
use super::super::spin::{self, Spin};
use super::super::{PaneMessage, WindowKey, WindowKind, WindowRoot, chord_label};
use crate::a11y::Control as _;
use crate::runtime::RailRow;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::BTreeMap;

/// The key context of an empty window's body: Tab there switches modes,
/// once chat is built.
pub(in crate::shell) const CONTEXT: &str = "EmptyWindow";

/// Agent chat isn't built yet: its side of the switch shows, blocked, and
/// Tab is left to move focus. True brings back what switches to it: the
/// `tab` binding (`keys::bind`), the "tab switch" hint and Help's line.
pub(in crate::shell) const CHAT_READY: bool = false;

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

/// An empty window's command line: the field, the picked row and the mode.
/// Only the window in front shows the field.
struct CommandLine {
    field: Entity<InputState>,
    pick: usize,
    mode: Mode,
    _changed: Subscription,
    _box_focused: Subscription,
}

/// The programs an empty window offers whose names hold `query`.
fn matching(rows: Vec<RailRow>, query: &str) -> Vec<RailRow> {
    let query = query.trim().to_lowercase();
    rows.into_iter()
        .filter(|row| super::tab_label(row).to_lowercase().contains(&query))
        .collect()
}

/// What an empty desk says when it has nothing to offer: `None` when there
/// is something to open, and the desk shows its buttons instead.
fn empty_panes_message(rail: &[RailRow]) -> Option<&'static str> {
    (!rail.iter().any(|row| !row.empty)).then_some("This network runs no program with a view.")
}

/// Which pane it is.
enum Place {
    /// The desk while no window is on it. `root` is the window's own focus,
    /// which takes the keys when the last window leaves; an overlay covers
    /// the desk only in the `console`; `bare` is whether the desk was bare
    /// when its observers last looked.
    Desk {
        spin: Entity<Spin>,
        root: FocusHandle,
        console: bool,
        bare: bool,
    },
    /// The empty window `instance` on the desk of `desk`.
    Window {
        instance: u64,
        desk: WeakEntity<WindowRoot>,
        command: CommandLine,
    },
}

/// What its body draws from the entities, as it last drew it: its
/// observers compare them against it, so a move of none of it leaves the
/// cached body alone.
#[derive(Default, PartialEq)]
struct Shown {
    dark: bool,
    rail: Vec<RailRow>,
    badges: BTreeMap<&'static str, i64>,
    /// An empty window's place on its desk, and whether it is in front.
    at: Option<(usize, bool)>,
}

/// A pane the app draws when no program is in it: the desk's own, or an
/// empty window's. It observes its window's `Desk`, the `Rail` and the
/// `Prefs`, and reads them at its draw; what is open over the desk it reads
/// without observing (`moved`).
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
    /// The desk of window `key` (a `kind` window), for as long as the
    /// window lives: drawn while no window is on it.
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

    /// Empty window `instance` on the desk of `desk`, whose box holds the
    /// keys by `own` when nothing inside it does.
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

    /// Over the app's entities (read here, never kept) and window `key`'s
    /// own.
    fn made(
        app: &Entities,
        key: WindowKey,
        place: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let own = app.windows.read(cx).own(key).expect("its window").clone();
        let (desk, overlays) = (own.desk.clone(), own.overlays.clone());
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

    /// An empty window's place on its desk, and whether it is in front.
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

    /// The desk, the rail or the preferences moved: the figure takes the
    /// motion switch and the ink, the keys go to the window itself once its
    /// last pane leaves, and the body draws again only if what it shows
    /// moved.
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
            // Not while something open over the desk keeps the keys: its
            // close gives them back (`OverlayLayer::moved`), and to the root
            // when what had them left with the last window (`focus_lost`).
            // This reads `Overlays` without observing it, so `bare` moves
            // either way: a close moves no desk.
            if now_bare && !*bare && !(*console && overlay) {
                root.focus(window, cx);
            }
            *bare = now_bare;
            if !now_bare {
                // not drawn: it reads the entities afresh when it is
                return;
            }
        }
        let shown = self.shown(cx);
        if shown != self.shown {
            self.shown = shown;
            cx.notify();
        }
    }

    /// The bare box of this empty window took the keys (a press on its
    /// background, or they came back to it): when it is in front and the
    /// keyboard does not hold it (⌘⇧M, whose box keeps them), the field
    /// takes them, so typing starts at once.
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

    /// Something done to the desk this window is on, by the desk.
    fn on_desk(&self, cx: &mut App, act: impl FnOnce(&mut WindowRoot, &mut Context<WindowRoot>)) {
        if let Place::Window { desk, .. } = &self.place {
            let _ = desk.update(cx, act);
        }
    }

    /// A press on a row of empty window `index`: that window takes the keys
    /// first, so the program opens there (`WindowRoot::open_here`).
    fn open_here(&self, index: usize, module: &'static str, window: &mut Window, cx: &mut App) {
        self.on_desk(cx, |desk, cx| desk.open_here(index, module, window, cx));
    }

    /// The desk while no window is on it: the figure, and either the reason
    /// the network has nothing to open or the ways to open something.
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

    /// An empty window's body (window `index`): the field and its switch,
    /// the programs its text matches, and the keys. Only the focused one
    /// holds the field; a press on another's rows or switch gives that
    /// window the keys first, pointer and assistive technology alike.
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
        // the bar is the field's box: it wears the field's ring
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
        let modules: Vec<&'static str> = rows.iter().map(|row| row.module).collect();
        let list = rows.iter().enumerate().map(|(nth, row)| {
            let module = row.module;
            let picked = focused && nth == pick;
            let name = super::tab_label(row);
            let note = match badges.get(module).copied().unwrap_or(0) {
                0 => String::new(),
                count => format!("{count} unread"),
            };
            sans(400, 15.)
                .id(SharedString::from(format!("empty/{module}")))
                // in a window without the keys too: the keyboard reaches it
                // by the chord that moves to that window (`PaneView`)
                .control(Role::ListBoxOption, SharedString::from(name.clone()))
                // the picked row is the one the field's ↑↓ move
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
                    this.open_here(index, module, window, cx);
                }))
                // assistive technology's press lands on this window's row,
                // not on whatever window covers its middle (gpui's own
                // Click is a pointer press there)
                .on_a11y_action(AccessibleAction::Click, {
                    let this = cx.entity().downgrade();
                    move |_, window, cx| {
                        let _ =
                            this.update(cx, |this, cx| this.open_here(index, module, window, cx));
                    }
                })
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
        // focused, the field and its rows are one combo box: the field holds
        // the keys, the picked row is what they pick
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
            .role(Role::Group)
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
                    let last = modules.len().saturating_sub(1);
                    match event.keystroke.key.as_str() {
                        "up" => command.pick = command.pick.saturating_sub(1),
                        "down" => command.pick = (command.pick + 1).min(last),
                        "enter" => match modules.get(command.pick.min(last)) {
                            Some(&module) => {
                                this.on_desk(cx, |desk, cx| desk.open_view(module, window, cx))
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
            // gone from its desk: dropped at the desk's next draw
            None => div().into_any_element(),
        }
    }
}

/// An empty desk's way out: the chord and what it does, and a press does
/// what the chord would.
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
    // its chord, as Help lists it (AX-114)
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
    // no node of its own: the combo box around it and its rows speaks
    // for it
    crate::a11y::text_field(
        "empty-window/field",
        &field.read(cx).focus_handle(cx),
        {
            let field = field.clone();
            move |value, window, cx| {
                field.update(cx, |field, cx| field.replace_all(value, window, cx))
            }
        },
        input.role(component::RoleOverride::Presentational),
    )
    .into_any_element()
}

/// Module | Chat, Tab between them. Chat is blocked until agent chat is
/// built, and says so on hover. A press gives window `index` the keys
/// first, as a row's does.
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
            command.mode = Mode::Module;
        }
        cx.notify();
    };
    let this = cx.entity().downgrade();
    let module = side("empty-window/module", "Module", mode == Mode::Module)
        .on_click(cx.listener(move |this, _, window, cx| pick(this, window, cx)))
        // on this window's switch, as a row's press is on its row
        .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
            let _ = this.update(cx, |this, cx| pick(this, window, cx));
        });
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
        .child(module)
        .child(chat.aria_disabled(true))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{empty_panes_message, matching};
    use crate::runtime::RailRow;

    fn row(module: &'static str, label: &str, empty: bool) -> RailRow {
        RailRow {
            module,
            label: label.into(),
            note: None,
            empty,
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
