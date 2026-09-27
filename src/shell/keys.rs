//! The app's own keys: gpui actions, bound in one table under key contexts
//! the windows set, and the same actions in the macOS menu bar. `secondary`
//! is ⌘ on a Mac and Ctrl elsewhere.
//!
//! What decides where a key goes is the key context of the focused element
//! and its parents: a window's root says what it is (`Ducktape`, `console`),
//! whether the desk's keys reach it (`desk`: on the desk, nothing open over
//! it), and whether something is open over it (`overlay`). A guest editor
//! (`GuestEditor`) sits deeper, so its own keys come first. An empty
//! window's ↑↓ and Enter are its field's (`command.rs` takes them); Tab
//! there is `SwitchMode`, under the field's own context.

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
        /// ⌘D: the focused window halved, left | right.
        Halve,
        /// ⌘⇧D: halved, top / bottom.
        HalveBelow,
        /// ⌘` / ctrl-tab: the next window to the front.
        CycleForward,
        /// ⌘⇧` / ctrl-shift-tab: the previous one.
        CycleBack,
        /// ⌘K.
        ToggleSpotlight,
        /// Tab in an empty window's field: what it searches (Module | Chat).
        SwitchMode,
        /// Escape: whatever is open over the desk.
        CloseOverlay,
    ]
);

/// ⌘1…⌘9: the Nth window.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = desk, no_json)]
pub(crate) struct FocusPane(pub(crate) usize);

/// The desk's context, on every window's root.
pub(super) const CONTEXT: &str = "Ducktape";

/// Every key the app answers, with the context it answers in.
pub(crate) fn bind(cx: &mut gpui_kit::App) {
    const DESK: Option<&str> = Some("Ducktape && desk");
    let mut bindings = vec![
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-n", NewWindow, DESK),
        KeyBinding::new("secondary-w", CloseWindow, Some(CONTEXT)),
        KeyBinding::new("secondary-d", Halve, DESK),
        KeyBinding::new("secondary-shift-d", HalveBelow, DESK),
        KeyBinding::new("secondary-`", CycleForward, DESK),
        KeyBinding::new("secondary-~", CycleForward, DESK),
        KeyBinding::new("secondary-shift-`", CycleBack, DESK),
        KeyBinding::new("secondary-shift-~", CycleBack, DESK),
        KeyBinding::new("ctrl-tab", CycleForward, DESK),
        KeyBinding::new("ctrl-shift-tab", CycleBack, DESK),
        KeyBinding::new("secondary-k", ToggleSpotlight, Some("Ducktape && on_desk")),
        KeyBinding::new("escape", CloseOverlay, Some("Ducktape && overlay")),
        KeyBinding::new("tab", SwitchMode, Some(super::command::CONTEXT)),
    ];
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
            MenuItem::action("Split", Halve),
            MenuItem::action("Split Below", HalveBelow),
            MenuItem::separator(),
            MenuItem::action("Next Window", CycleForward),
            MenuItem::action("Previous Window", CycleBack),
        ]),
    ]);
}

impl DesktopWindow {
    /// What this window's root tells the keymap about it.
    pub(super) fn key_context(&self, cx: &gpui_kit::App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add(CONTEXT);
        let console = self.kind == WindowKind::Console;
        let on_desk = console && self.on_desk(cx);
        let overlay = console && self.model.read(cx).state.overlay.is_some();
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
            .on_action(cx.listener(|this, _: &Halve, window, cx| {
                this.pane_message(PaneMessage::Halve { below: false }, window, cx)
            }))
            .on_action(cx.listener(|this, _: &HalveBelow, window, cx| {
                this.pane_message(PaneMessage::Halve { below: true }, window, cx)
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
                let message = match this.model.read(cx).state.overlay {
                    Some(crate::Overlay::Spotlight) => {
                        Message::CloseOverlay(crate::Overlay::Spotlight)
                    }
                    _ => Message::OpenSpotlight,
                };
                this.model
                    .update(cx, |model, cx| model.dispatch(message, cx));
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
            resolve("secondary-d", &[desk], cx).as_deref(),
            Some("desk::Halve")
        );
        assert_eq!(
            resolve("secondary-d", &["Ducktape console"], cx),
            None,
            "the launcher"
        );
        assert_eq!(
            resolve("secondary-d", &["Ducktape console on_desk overlay"], cx),
            None,
            "an overlay keeps its keys"
        );
        assert_eq!(
            resolve("secondary-w", &["Ducktape console on_desk overlay"], cx).as_deref(),
            Some("desk::CloseWindow")
        );
        assert_eq!(
            resolve("secondary-n", &[desk], cx).as_deref(),
            Some("desk::NewWindow")
        );
        assert_eq!(resolve("3", &[desk], cx), None, "a pane with a view types");
        assert_eq!(
            resolve("escape", &["Ducktape console on_desk overlay"], cx).as_deref(),
            Some("desk::CloseOverlay")
        );
        assert_eq!(resolve("escape", &[desk], cx), None);
    }
}
