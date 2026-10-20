//! The app's own keys: gpui actions, bound in one table under key contexts
//! the windows set, and the same actions in the macOS menu bar. `secondary`
//! is ⌘ on a Mac and Ctrl elsewhere.
//!
//! What decides where a key goes is the key context of the focused element
//! and its parents: a window's root says what it is (`Ducktape`, `console`),
//! whether it is past the launcher (`on_desk`: ⌘K works here even with
//! something open), whether the desk's keys reach it (`desk`: on the desk,
//! nothing open over it), and whether something is open over it
//! (`overlay`). A view's text field takes its claimed keys at the
//! keystroke interceptor, before any binding. An empty window's ↑↓ and Enter are its field's
//! (`layers::EmptyPane` takes them); Tab there is `SwitchMode` once agent chat is built
//! (`layers::CHAT_READY`), under the empty window's own context
//! (`layers::CONTEXT`), and until then moves focus. A desk window is a
//! pane; an OS window's root is a `WindowRoot`.

use super::entities::{Overlay, Popover};
use super::*;
use crate::ui::layout::Side;
use ducktape_host::session::TextSize;
use gpui_kit::{Action, Context, KeyBinding, KeyContext, Menu, MenuItem};

gpui_kit::actions!(
    desk,
    [
        Quit,
        NewWindow,
        CloseWindow,
        CycleForward,
        CycleBack,
        FillPane,
        HoldPane,
        KeepOnTop,
        ToggleSpotlight,
        OpenSettings,
        OpenHelp,
        SwitchMode,
        CloseOverlay,
        MenuTab,
        MenuTabBack,
        TextLarger,
        TextSmaller,
        TextReset,
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = desk, no_json)]
pub(crate) struct FocusPane(pub(crate) usize);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = desk, no_json)]
pub(crate) struct SnapPane(pub(crate) Side);

pub(super) const SNAP_CMD: [(&str, Side); 4] = [
    ("cmd-shift-left", Side::Left),
    ("cmd-shift-right", Side::Right),
    ("cmd-shift-up", Side::Top),
    ("cmd-shift-down", Side::Bottom),
];

pub(super) const SNAP_CTRL_ALT: [(&str, Side); 4] = [
    ("ctrl-alt-h", Side::Left),
    ("ctrl-alt-l", Side::Right),
    ("ctrl-alt-k", Side::Top),
    ("ctrl-alt-j", Side::Bottom),
];

pub(super) const FILL_CTRL_ALT: &str = "ctrl-alt-enter";

pub(super) fn snap_keys() -> [(&'static str, Side); 4] {
    match cfg!(target_os = "macos") {
        true => SNAP_CMD,
        false => SNAP_CTRL_ALT,
    }
}

pub(super) const CONTEXT: &str = "Ducktape";

pub(super) const MENU: &str = "menu";

pub(crate) fn bind(cx: &mut gpui_kit::App) {
    const DESK: Option<&str> = Some("Ducktape && desk");
    let mut bindings = vec![
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-n", NewWindow, DESK),
        KeyBinding::new("secondary-/", OpenHelp, DESK),
        KeyBinding::new("secondary-w", CloseWindow, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-enter", FillPane, DESK),
        KeyBinding::new("secondary-shift-m", HoldPane, DESK),
        KeyBinding::new("secondary-`", CycleForward, DESK),
        KeyBinding::new("secondary-~", CycleForward, DESK),
        KeyBinding::new("secondary-shift-`", CycleBack, DESK),
        KeyBinding::new("secondary-shift-~", CycleBack, DESK),
        KeyBinding::new("ctrl-tab", CycleForward, DESK),
        KeyBinding::new("ctrl-shift-tab", CycleBack, DESK),
        KeyBinding::new("secondary-k", ToggleSpotlight, Some("Ducktape && on_desk")),
        KeyBinding::new("secondary-,", OpenSettings, Some(CONTEXT)),
        KeyBinding::new("escape", CloseOverlay, Some("Ducktape && overlay")),
        KeyBinding::new("tab", MenuTab, Some(MENU)),
        KeyBinding::new("shift-tab", MenuTabBack, Some(MENU)),
        KeyBinding::new("secondary-=", TextLarger, None),
        KeyBinding::new("secondary-+", TextLarger, None),
        KeyBinding::new("secondary--", TextSmaller, None),
        KeyBinding::new("secondary-0", TextReset, None),
    ];
    for (stroke, side) in snap_keys() {
        bindings.push(KeyBinding::new(stroke, SnapPane(side), DESK));
    }
    if !cfg!(target_os = "macos") {
        bindings.push(KeyBinding::new(FILL_CTRL_ALT, FillPane, DESK));
    }
    if super::windows::can_keep_above(cx) {
        bindings.push(KeyBinding::new(
            "secondary-shift-t",
            KeepOnTop,
            Some(CONTEXT),
        ));
    }
    if super::layers::CHAT_READY {
        bindings.push(KeyBinding::new(
            "tab",
            SwitchMode,
            Some(super::layers::CONTEXT),
        ));
    }
    for nth in 1..=9 {
        bindings.push(KeyBinding::new(
            &format!("secondary-{nth}"),
            FocusPane(nth - 1),
            DESK,
        ));
    }
    cx.bind_keys(bindings);
}

pub(crate) fn menus(on_top: bool, cx: &mut gpui_kit::App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.set_menus(menu_bar(on_top));
}

fn menu_bar(on_top: bool) -> Vec<Menu> {
    vec![
        Menu::new("Ducktape").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Quit Ducktape", Quit),
        ]),
        Menu::new("View").items([
            MenuItem::action("Search", ToggleSpotlight),
            MenuItem::separator(),
            MenuItem::action("Larger Text", TextLarger),
            MenuItem::action("Smaller Text", TextSmaller),
            MenuItem::action("Default Text Size", TextReset),
        ]),
        Menu::new("Window").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("Close", CloseWindow),
            MenuItem::separator(),
            MenuItem::action("Fill", FillPane),
            MenuItem::action("Left Half", SnapPane(Side::Left)),
            MenuItem::action("Right Half", SnapPane(Side::Right)),
            MenuItem::action("Top Half", SnapPane(Side::Top)),
            MenuItem::action("Bottom Half", SnapPane(Side::Bottom)),
            MenuItem::action("Move or Size", HoldPane),
            MenuItem::action("Keep on Top", KeepOnTop).checked(on_top),
            MenuItem::separator(),
            MenuItem::action("Next Window", CycleForward),
            MenuItem::action("Previous Window", CycleBack),
        ]),
        Menu::new("Help").items([MenuItem::action("Ducktape Help", OpenHelp)]),
    ]
}

impl WindowRoot {
    fn text_size(&self, step: fn(TextSize) -> TextSize, cx: &mut Context<Self>) {
        self.app.prefs.update(cx, |prefs, cx| {
            let size = step(prefs.get().text_size);
            prefs.set_text_size(size, cx);
        });
    }

    pub(super) fn key_context(&self, cx: &gpui_kit::App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add(CONTEXT);
        let console = self.kind == WindowKind::Console;
        let on_desk = console && self.on_desk(cx);
        let overlay = console && self.overlays.read(cx).get().is_some();
        if console {
            context.add("console");
        }
        if on_desk {
            context.add("on_desk");
        }
        if overlay {
            context.add("overlay");
        }
        if on_desk && !overlay {
            context.add("desk");
        }
        context
    }

    pub(super) fn on_keys(
        &self,
        root: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        use gpui_kit::InteractiveElement as _;
        root.key_context(self.key_context(cx))
            .on_action(cx.listener(
                |this, _: &CloseWindow, window, cx| match this.command_w_pane(cx) {
                    Some(index) => this.pane_message(PaneMessage::Close(index), window, cx),
                    None => this.close_by_key(window, cx),
                },
            ))
            .on_action(cx.listener(|this, _: &NewWindow, window, cx| {
                this.pane_message(PaneMessage::Split(layout::EMPTY), window, cx)
            }))
            .on_action(cx.listener(|this, _: &FillPane, window, cx| this.fill_pane(window, cx)))
            .on_action(cx.listener(|this, _: &HoldPane, _, cx| this.hold_pane(cx)))
            .on_action(cx.listener(|this, SnapPane(side): &SnapPane, window, cx| {
                this.snap_pane(*side, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &TextLarger, _, cx| this.text_size(TextSize::larger, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TextSmaller, _, cx| this.text_size(TextSize::smaller, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TextReset, _, cx| {
                    this.text_size(|_| TextSize::default(), cx)
                }),
            )
            .on_action(cx.listener(|this, _: &KeepOnTop, _, cx| {
                let key = this.key;
                this.app
                    .windows
                    .update(cx, |windows, cx| windows.toggle_on_top(key, cx));
            }))
            .on_action(cx.listener(|this, _: &CycleForward, window, cx| {
                this.pane_message(PaneMessage::Cycle { forward: true }, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CycleBack, window, cx| {
                this.pane_message(PaneMessage::Cycle { forward: false }, window, cx)
            }))
            .on_action(
                cx.listener(|this, FocusPane(index): &FocusPane, window, cx| {
                    this.pane_message(PaneMessage::Focus(*index), window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSpotlight, _, cx| {
                let windows = this.app.windows.read(cx);
                let Some(own) = windows.console().and_then(|key| windows.own(key)) else {
                    return;
                };
                own.overlays
                    .clone()
                    .update(cx, |overlays, cx| overlays.toggle(Overlay::Spotlight, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                let windows = this.app.windows.clone();
                cx.defer(move |cx| {
                    windows.update(cx, |windows, cx| windows.open_settings(None, cx))
                });
            }))
            .on_action(cx.listener(|this, _: &OpenHelp, _, cx| {
                this.overlays.entity().update(cx, |overlays, cx| {
                    overlays.close(Overlay::Menu(Popover::Node), cx)
                });
                this.app
                    .windows
                    .update(cx, |windows, cx| windows.help_asked(cx));
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| {
                this.overlays.entity().update(cx, |overlays, cx| {
                    if let Some(open) = *overlays.get() {
                        overlays.close(open, cx);
                    }
                });
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        stroke: &str,
        contexts: &[&str],
        cx: &mut gpui_kit::TestAppContext,
    ) -> Option<String> {
        cx.update(|cx| {
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let stack: Vec<KeyContext> = contexts
                .iter()
                .map(|context| KeyContext::parse(context).unwrap())
                .collect();
            let (bindings, _) =
                keymap.bindings_for_input(&[gpui_kit::Keystroke::parse(stroke).unwrap()], &stack);
            bindings
                .first()
                .map(|binding| binding.action().name().to_owned())
        })
    }

    #[gpui_kit::test]
    fn the_desks_keys_answer_only_where_their_context_says(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        let desk = "Ducktape console on_desk desk";
        assert_eq!(
            resolve("secondary-n", &[desk], cx).as_deref(),
            Some("desk::NewWindow")
        );
        assert_eq!(
            resolve("secondary-n", &["Ducktape console"], cx),
            None,
            "the launcher"
        );
        assert_eq!(
            resolve("secondary-n", &["Ducktape console on_desk overlay"], cx),
            None,
            "an overlay keeps its keys"
        );
        assert_eq!(resolve("secondary-d", &[desk], cx), None);
        assert_eq!(resolve("secondary-shift-d", &[desk], cx), None);
        assert_eq!(
            resolve("secondary-w", &["Ducktape console on_desk overlay"], cx).as_deref(),
            Some("desk::CloseWindow")
        );
        assert_eq!(resolve("3", &[desk], cx), None, "a pane with a view types");
        assert_eq!(
            resolve("escape", &["Ducktape console on_desk overlay"], cx).as_deref(),
            Some("desk::CloseOverlay")
        );
        assert_eq!(resolve("escape", &[desk], cx), None);
    }

    #[gpui_kit::test]
    fn settings_answers_on_every_screen(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        for context in [
            "Ducktape console",
            "Ducktape console on_desk desk",
            "Ducktape console on_desk overlay",
            "Ducktape",
        ] {
            assert_eq!(
                resolve("secondary-,", &[context], cx).as_deref(),
                Some("desk::OpenSettings"),
                "{context}"
            );
        }
    }

    #[gpui_kit::test]
    fn the_pane_chords_answer_on_the_desk_only(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        let desk = "Ducktape console on_desk desk";
        for (stroke, action) in [
            ("secondary-shift-enter", "desk::FillPane"),
            ("secondary-shift-m", "desk::HoldPane"),
        ] {
            assert_eq!(resolve(stroke, &[desk], cx).as_deref(), Some(action));
            assert_eq!(resolve(stroke, &["Ducktape console"], cx), None);
            assert_eq!(
                resolve(stroke, &["Ducktape console on_desk overlay"], cx),
                None,
                "an overlay keeps its keys"
            );
        }
        assert_eq!(resolve("secondary-enter", &[desk], cx), None);
        assert_eq!(resolve("secondary-m", &[desk], cx), None);
        assert_eq!(
            resolve("secondary-shift-m", &[desk, "hold"], cx).as_deref(),
            Some("desk::HoldPane")
        );
    }

    #[gpui_kit::test]
    fn the_snap_chords_answer_on_the_desk_only(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        let desk = "Ducktape console on_desk desk";
        for (stroke, side) in snap_keys() {
            assert_eq!(
                resolve(stroke, &[desk], cx).as_deref(),
                Some("desk::SnapPane"),
                "{stroke}: {side:?}"
            );
            assert_eq!(resolve(stroke, &["Ducktape console"], cx), None);
            assert_eq!(
                resolve(stroke, &["Ducktape console on_desk overlay"], cx),
                None,
                "an overlay keeps its keys"
            );
        }
        if !cfg!(target_os = "macos") {
            assert_eq!(
                resolve(FILL_CTRL_ALT, &[desk], cx).as_deref(),
                Some("desk::FillPane")
            );
        }
    }

    #[gpui_kit::test]
    fn no_desk_chord_takes_a_key_gnome_or_text_editing_owns(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        let desk = "Ducktape console on_desk desk";
        let mut owned = vec![
            "ctrl-alt-tab".to_owned(),
            "ctrl-alt-shift-tab".to_owned(),
            "ctrl-alt-escape".to_owned(),
            "ctrl-alt-shift-escape".to_owned(),
            "ctrl-alt-delete".to_owned(),
            "ctrl-alt-shift-r".to_owned(),
        ];
        for arrow in ["left", "right", "up", "down"] {
            for held in ["ctrl", "ctrl-shift", "ctrl-alt", "ctrl-alt-shift"] {
                owned.push(format!("{held}-{arrow}"));
            }
        }
        for nth in 1..=12 {
            owned.push(format!("ctrl-alt-f{nth}"));
        }
        for nth in 1..=9 {
            owned.push(format!("ctrl-super-{nth}"));
        }
        for stroke in &owned {
            assert_eq!(resolve(stroke, &[desk], cx), None, "{stroke}");
        }
        for (stroke, _) in SNAP_CTRL_ALT {
            assert!(!owned.iter().any(|it| it == stroke), "{stroke}");
        }
    }

    #[gpui_kit::test]
    fn the_text_size_keys_answer_on_every_screen(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        for context in [
            "Ducktape console",
            "Ducktape console on_desk desk",
            "Ducktape console on_desk overlay",
            "Ducktape",
        ] {
            for (stroke, action) in [
                ("secondary-=", "desk::TextLarger"),
                ("secondary-+", "desk::TextLarger"),
                ("secondary--", "desk::TextSmaller"),
                ("secondary-0", "desk::TextReset"),
            ] {
                assert_eq!(
                    resolve(stroke, &[context], cx).as_deref(),
                    Some(action),
                    "{stroke} in {context}"
                );
            }
        }
    }

    #[gpui_kit::test]
    fn keep_on_top_answers_in_every_window(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        for context in [
            "Ducktape console on_desk desk",
            "Ducktape console on_desk overlay",
            "Ducktape",
        ] {
            assert_eq!(
                resolve("secondary-shift-t", &[context], cx).as_deref(),
                Some("desk::KeepOnTop"),
                "{context}"
            );
        }
        assert_eq!(resolve("secondary-t", &["Ducktape"], cx), None);
    }

    #[test]
    fn the_window_menus_keep_on_top_is_checked_as_asked() {
        let checked = |on_top| {
            menu_bar(on_top)
                .into_iter()
                .find(|menu| menu.name.as_ref() == "Window")
                .expect("a Window menu")
                .items
                .into_iter()
                .find_map(|item| match &item {
                    MenuItem::Action { name, .. } if name.as_ref() == "Keep on Top" => {
                        Some(item.is_checked())
                    }
                    _ => None,
                })
        };
        assert_eq!(checked(true), Some(true));
        assert_eq!(checked(false), Some(false));
    }

    #[gpui_kit::test]
    fn tab_in_an_empty_window_moves_focus_until_chat_is_built(cx: &mut gpui_kit::TestAppContext) {
        cx.update(bind);
        let empty = [
            "Ducktape console on_desk desk",
            super::super::layers::CONTEXT,
        ];
        assert_eq!(
            resolve("tab", &empty, cx).is_some(),
            super::super::layers::CHAT_READY
        );
    }
}
