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
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = desk, no_json)]
pub(crate) struct FocusPane(pub(crate) usize);

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
    ];
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
        Menu::new("View").items([MenuItem::action("Search", ToggleSpotlight)]),
        Menu::new("Window").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("Close", CloseWindow),
            MenuItem::separator(),
            MenuItem::action("Fill", FillPane),
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
