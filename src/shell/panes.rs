//! What a window does to its panes: the messages the bar, the keys and the
//! panes' own controls send (`pane_message`, `open_here`, `open_view`),
//! and a pane's name. The panes themselves are drawn by
//! `layers::PaneLayer` and `layers::PaneView`; where they sit, stack and
//! which has the keys is `ui::layout`'s; their seats are
//! `entities::Seats`'.
use super::*;
use gpui_kit::Window;

/// A pane's name: its program's, as the rail lists it (`Rail`).
pub(in crate::shell) fn label(rail: &[crate::runtime::RailRow], module: &str) -> String {
    if module == layout::EMPTY {
        return "Empty".to_owned();
    }
    if module == layout::HELP {
        return "Help".to_owned();
    }
    rail.iter()
        .find(|row| row.module == module)
        .map_or_else(|| module.to_owned(), |row| row.label.clone())
}

impl WindowRoot {
    /// Something done to this window's panes: its `Desk` moves them (or
    /// `Windows`, for what crosses windows: a close that takes a pop-out
    /// with it, a pop-out, a pop-in), and the keys go to the focused one
    /// (`PaneLayer::drawn`).
    pub(super) fn pane_message(
        &mut self,
        message: PaneMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_switch();
        let key = self.key;
        let windows = self.app.windows.clone();
        match message {
            PaneMessage::Close(index) => {
                windows.update(cx, |windows, cx| windows.close_pane(key, index, cx))
            }
            PaneMessage::PopOut { index, at } => {
                // it opens where it sat on the desk
                let at = at.or_else(|| {
                    let layout = self.layout(cx);
                    let pane = layout.panes.get(index);
                    Some(super::windows::unseated(
                        window.bounds(),
                        pane.and_then(|pane| pane.frame),
                        window.display(cx).map(|display| display.bounds()),
                        pane.map_or(super::windows::POPOUT_MIN, |pane| {
                            super::windows::popout_min(pane.module)
                        }),
                    ))
                });
                windows.update(cx, |windows, cx| windows.pop_out(key, index, at, cx))
            }
            PaneMessage::PopIn => windows.update(cx, |windows, cx| windows.pop_in(key, cx)),
            message => self.desk.update(cx, |desk, cx| desk.moved_by(message, cx)),
        }
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
