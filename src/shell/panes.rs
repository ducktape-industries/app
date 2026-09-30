//! What a window does to its panes: the messages the bar, the keys and the
//! panes' own controls send (`pane_message`, `open_here`, `open_view`),
//! and a pane's name. The panes themselves are drawn by
//! `layers::PaneLayer` and `layers::PaneView`; where they sit, stack and
//! which has the keys is `ui::layout`'s; their seats are
//! `entities::Seats`'.
use super::*;

pub(super) fn label(roster: &crate::runtime::Roster, module: &str) -> String {
    if module == layout::EMPTY {
        return "Empty".to_owned();
    }
    if module == layout::HELP {
        return "Help".to_owned();
    }
    roster
        .rail()
        .into_iter()
        .find(|row| row.module == module)
        .map(|row| row.label)
        .unwrap_or_else(|| module.to_owned())
}

impl DesktopWindow {
    /// Something done to this window's panes: the model moves them, and
    /// the keys go to the focused one (`PaneLayer::drawn`).
    pub(super) fn pane_message(
        &mut self,
        message: PaneMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_switch();
        let message = match message {
            // it opens where it sat on the desk
            PaneMessage::PopOut { index, at: None } => {
                let layout = self.layout(cx);
                let pane = layout.panes.get(index);
                PaneMessage::PopOut {
                    index,
                    at: Some(super::windows::unseated(
                        window.bounds(),
                        pane.and_then(|pane| pane.frame),
                        window.display(cx).map(|display| display.bounds()),
                        pane.map_or(super::windows::POPOUT_MIN, |pane| {
                            super::windows::popout_min(pane.module)
                        }),
                    )),
                }
            }
            message => message,
        };
        let key = self.key;
        self.model.update(cx, |model, cx| {
            model.dispatch(Message::Pane(key, message), cx)
        });
        // never from inside the layer's own update: its listeners send
        // through here, so a nested update would hold it twice
        self.panes.update(cx, |panes, _| panes.panes_moved = true);
        cx.notify();
    }

    /// A press on a row of empty window `index`: that window takes the keys
    /// first, as a pointer's press on it does (`pane_drag::raise`), so the
    /// program opens there, not in the window that had them.
    pub(super) fn open_here(
        &mut self,
        index: usize,
        module: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Focus(index), window, cx);
        self.open_view(module, window, cx);
    }

    /// A menu bar click, or a pick in an empty window: see `Layout::open`.
    pub(super) fn open_view(
        &mut self,
        module: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Open(module), window, cx);
    }
}
