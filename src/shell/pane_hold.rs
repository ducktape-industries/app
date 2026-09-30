//! A window held by the keyboard (⌘⇧M, or "Move or size window" in Search):
//! the arrows move it and, with Alt (Option on a Mac), size it from its
//! right and bottom edges; Return or Space keeps it where it is, Escape
//! puts it back as it was, and any other key, a press, something opening
//! over the desk or the OS window losing the keys keeps it. Whether a
//! window is held is the model's (`Layout::held`); this file gives it the
//! keys while it is, and hands them back to what had them. ⌘⇧↩ fills the
//! desk with the window in front, as a double press on its title bar does.
//! The chords are the window's (`DesktopWindow`); the hold on the keys is
//! the `PaneLayer`'s, brought in line with the model after every draw.
use super::layers::PaneLayer;
use super::*;

/// Pixels an arrow moves or sizes by; with Shift, [`FAR`].
const STEP: f32 = 8.;
const FAR: f32 = 32.;

/// The held window's box has the keys.
pub(super) struct Holding {
    /// The held window's box.
    own: gpui_kit::FocusHandle,
    /// What had the keys when the hold took them.
    previous: Option<gpui_kit::FocusHandle>,
}

/// What a screen reader is told when a window is taken: how to move it and
/// how to let go. The chords are the platform's.
pub(super) fn hold_words(roster: &crate::runtime::Roster, module: &str) -> String {
    let (size, keep) = match cfg!(target_os = "macos") {
        true => ("Option", "Return"),
        false => ("Alt", "Enter"),
    };
    format!(
        "Moving {} window. Arrows move, Shift further, {size} sizes, {keep} keeps, Escape puts back",
        panes::label(roster, module),
    )
}

/// The key that keeps a window on a tab or in a hold: Return on a Mac.
pub(super) fn keep_key() -> &'static str {
    match cfg!(target_os = "macos") {
        true => "Return",
        false => "Enter",
    }
}

impl DesktopWindow {
    /// The window the desk's chords act on: the one in front, when the
    /// desk's keys reach it and it has a frame.
    pub(super) fn desk_pane(&self, cx: &gpui_kit::App) -> Option<usize> {
        let layout = self.layout(cx);
        let pane = layout.panes.get(layout.focused)?;
        (self.desk_keys(cx) && pane.frame.is_some()).then_some(layout.focused)
    }

    /// ⌘⇧↩. A window held lets go where it is first.
    pub(super) fn fill_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.desk_pane(cx) else {
            return;
        };
        if self.layout(cx).held.is_some() {
            self.hold_message(PaneMessage::Release { keep: true }, cx);
        }
        self.pane_message(PaneMessage::Fill(index), window, cx);
    }

    /// ⌘⇧M.
    pub(super) fn hold_pane(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.desk_pane(cx) {
            self.hold_message(PaneMessage::Hold(index), cx);
        }
    }

    /// A hold's message to the model. Not `pane_message`: that hands the
    /// keys to the window in front, and the hold has them.
    pub(super) fn hold_message(&mut self, message: PaneMessage, cx: &mut Context<Self>) {
        let key = self.key;
        self.model.update(cx, |model, cx| {
            model.dispatch(Message::Pane(key, message), cx)
        });
        cx.notify();
    }
}

impl PaneLayer {
    /// A hold's message to the model, through the window.
    fn hold_message(&self, message: PaneMessage, cx: &mut gpui_kit::App) {
        let _ = self
            .window
            .update(cx, |desk, cx| desk.hold_message(message, cx));
    }

    /// Brings the keys in line with the model: to the held window's box
    /// when a window is taken, and back to what had them when it is let go.
    /// After a draw (`PaneLayer::drawn`): Search giving the keys back to
    /// what had them (`desk.rs`, deferred from the draw) has gone first,
    /// and that is what is kept.
    pub(super) fn sync_hold(
        &mut self,
        layout: &layout::Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let held = layout
            .held
            .filter(|_| self.kind == crate::shell::WindowKind::Console);
        let own = held
            .and_then(|held| self.pane_keys.get(&held.instance))
            .map(|(own, _)| own.clone());
        match (own, &self.holding) {
            (None, None) => {}
            (None, Some(_)) => self.give_back(window, cx),
            (Some(own), None) => {
                self.holding = Some(Holding {
                    own: own.clone(),
                    previous: window.focused(cx),
                });
                own.focus(window, cx);
            }
            (Some(own), Some(_)) => {
                if !own.is_focused(window) {
                    // the keys went elsewhere (a Tab, a press, assistive technology)
                    self.hold_message(PaneMessage::Release { keep: true }, cx);
                }
            }
        }
    }

    /// The hold is let go of: the keys go back to what had them, where its
    /// box still has them. A press, a Tab or assistive technology that took
    /// them left them where they went.
    fn give_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Holding { own, previous }) = self.holding.take() else {
            return;
        };
        let Some(previous) = previous else {
            return;
        };
        if own.is_focused(window) {
            previous.focus(window, cx);
        }
        // something that opened over the desk as the hold ended took the box
        // for what had them (its own keys came first, `desk.rs`): they go
        // back to `previous` when it closes
        let _ = self.window.update(cx, |desk, _| {
            if desk.refocus.as_ref() == Some(&own) {
                desk.refocus = Some(previous);
            }
        });
    }

    /// A key while window `index` is held.
    pub(super) fn held_key(
        &mut self,
        index: usize,
        event: &gpui_kit::KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let stroke = &event.keystroke;
        let modifiers = stroke.modifiers;
        let step = match modifiers.shift {
            true => FAR,
            false => STEP,
        };
        let by = match stroke.key.as_str() {
            "left" => Some((-step, 0.)),
            "right" => Some((step, 0.)),
            "up" => Some((0., -step)),
            "down" => Some((0., step)),
            _ => None,
        };
        match (stroke.key.as_str(), by) {
            (_, Some(by)) if !modifiers.control && !modifiers.platform => {
                cx.stop_propagation();
                let sides = match (modifiers.alt, by.0 != 0.) {
                    (false, _) => pane_drag::Sides::NONE,
                    (true, true) => pane_drag::Sides::RIGHT,
                    (true, false) => pane_drag::Sides::BOTTOM,
                };
                let layout = self.layout(cx);
                if let Some(drag) = pane_drag::Drag::of(&layout, index, sides, (0., 0.)) {
                    let frame = drag.frame(by);
                    self.hold_message(PaneMessage::Frame(index, frame), cx);
                }
            }
            ("enter" | "space", _) => {
                cx.stop_propagation();
                self.hold_message(PaneMessage::Release { keep: true }, cx);
            }
            ("escape", _) => {
                cx.stop_propagation();
                self.hold_message(PaneMessage::Release { keep: false }, cx);
            }
            // it goes on to what it means outside a hold
            _ => self.hold_message(PaneMessage::Release { keep: true }, cx),
        }
    }
}
