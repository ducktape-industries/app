//! The app's own keys: gpui actions, bound in one table under key contexts
//! the windows set, and the same actions in the macOS menu bar. `secondary`
//! is ⌘ on a Mac and Ctrl elsewhere.
//!
//! What decides where a key goes is the key context of the focused element
//! and its parents: a window's root says what it is (`Ducktape`, `console`),
//! whether it is past the launcher (`on_desk`: ⌘K works here even with
//! something open), whether the desk's keys reach it (`desk`: on the desk,
//! nothing open over it), and whether something is open over it
//! (`overlay`). A guest editor (`GuestEditor`) sits deeper, so its own keys
//! come first. An empty window's ↑↓ and Enter are its field's
//! (`layers::EmptyPane` takes them); Tab there is `SwitchMode` once agent chat is built
//! (`layers::CHAT_READY`), under the empty window's own context
//! (`layers::CONTEXT`), and until then moves focus. A desk window is a
//! pane; an OS window's root is a `WindowRoot`.

use super::*;
use gpui_kit::{Action, KeyBinding, KeyContext, Menu, MenuItem};

gpui_kit::actions!(
    desk,
    [
        /// ⌘Q.
        Quit,
        /// ⌘N: an empty window on the desk.
        NewWindow,
        /// ⌘W: the focused desk window, else this window.
        CloseWindow,
        /// ⌘` / ctrl-tab: the next window to the front.
        CycleForward,
        /// ⌘⇧` / ctrl-shift-tab: the previous one.
        CycleBack,
        /// ⌘⇧↩: the window in front fills the desk, or goes back.
        FillPane,
        /// ⌘⇧M: the arrows move the window in front (⌥ sizes it).
        HoldPane,
        /// ⌘K.
        ToggleSpotlight,
        /// ⌘/: the app's help, in a window on the desk.
        OpenHelp,
        /// Tab in an empty window's field: what it searches (Module | Chat).
        SwitchMode,
        /// Escape: whatever is open over the desk.
        CloseOverlay,
        /// Tab in a menu hanging from the bar: on, and past its end out of
        /// it, which closes it (`layers::Chrome`).
        MenuTab,
        /// Shift+Tab in a menu: back, and past its start out of it.
        MenuTabBack,
    ]
);

/// ⌘1…⌘9: the Nth window.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = desk, no_json)]
pub(crate) struct FocusPane(pub(crate) usize);

/// The app's context, on every window's root.
pub(super) const CONTEXT: &str = "Ducktape";

/// The context of a menu hanging from the bar, on the box that holds its
/// keys (`layers::Chrome`).
pub(super) const MENU: &str = "menu";

/// Every key the app answers, with the context it answers in.
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
        KeyBinding::new("escape", CloseOverlay, Some("Ducktape && overlay")),
        // over the kit's own Tab, which would move the keys and leave the
        // menu open behind them
        KeyBinding::new("tab", MenuTab, Some(MENU)),
        KeyBinding::new("shift-tab", MenuTabBack, Some(MENU)),
    ];
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

/// The macOS menu bar: the app's keys where a Mac user looks for them.
pub(crate) fn menus(cx: &mut gpui_kit::App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.set_menus(vec![
        Menu::new("Ducktape").items([MenuItem::action("Quit Ducktape", Quit)]),
        Menu::new("View").items([MenuItem::action("Search", ToggleSpotlight)]),
        Menu::new("Window").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("Close", CloseWindow),
            MenuItem::separator(),
            MenuItem::action("Fill", FillPane),
            MenuItem::action("Move or Size", HoldPane),
            MenuItem::separator(),
            MenuItem::action("Next Window", CycleForward),
            MenuItem::action("Previous Window", CycleBack),
        ]),
        Menu::new("Help").items([MenuItem::action("Ducktape Help", OpenHelp)]),
    ]);
}

impl WindowRoot {
    /// What this window's root tells the keymap about it.
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

    /// The actions a window answers, on its root.
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
                let message = match this.model.read(cx).state.overlay {
                    Some(crate::Overlay::Spotlight) => {
                        Message::CloseOverlay(crate::Overlay::Spotlight)
                    }
                    _ => Message::OpenSpotlight,
                };
                this.model
                    .update(cx, |model, cx| model.dispatch(message, cx));
            }))
            .on_action(cx.listener(|this, _: &OpenHelp, _, cx| {
                this.model
                    .update(cx, |model, cx| model.dispatch(Message::OpenHelp, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| {
                if let Some(overlay) = this.model.read(cx).state.overlay {
                    this.model.update(cx, |model, cx| {
                        model.dispatch(Message::CloseOverlay(overlay), cx)
                    });
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a keystroke does with `contexts` (outermost first) focused.
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
        // no key halves a window
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

    /// ⌘⇧↩ fills and ⌘⇧M holds, on the desk only, and neither takes the
    /// plain keys of the chords they sit beside.
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

    /// Tab in an empty window switches to agent chat only once chat is
    /// built; until then it moves focus, as everywhere (AX-022).
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
