//! What a window does to its panes: the messages the bar, the keys and the
//! panes' own controls send (`pane_message`, `open_here`, `open_view`),
//! and a pane's name. The panes themselves are drawn by
//! `layers::PaneLayer` and `layers::PaneView`; where they sit, stack and
//! which has the keys is `ui::layout`'s; their seats are
//! `entities::Seats`'.
use super::*;
use gpui_kit::{Context, Window};

pub(in crate::shell) fn label(rail: &[crate::runtime::RailRow], program: &str) -> String {
    if program == layout::EMPTY {
        return "Empty".to_owned();
    }
    if program == layout::HELP {
        return "Help".to_owned();
    }
    if program == layout::MEMORY {
        return "Memory".to_owned();
    }
    rail.iter()
        .find(|row| row.program == program)
        .map_or_else(|| program.to_owned(), |row| row.label.clone())
}

impl WindowRoot {
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
                let at = at.or_else(|| {
                    let layout = self.layout(cx);
                    let pane = layout.panes.get(index);
                    let inset = super::layers::chrome_inset(
                        self.app.prefs.read(cx).get().layout_or_default(),
                    );
                    Some(super::windows::unseated(
                        window.bounds(),
                        inset,
                        pane.and_then(|pane| pane.frame),
                        window.display(cx).map(|display| display.bounds()),
                        pane.map_or(super::windows::POPOUT_MIN, |pane| {
                            super::windows::popout_min(pane.program)
                        }),
                    ))
                });
                windows.update(cx, |windows, cx| {
                    _ = windows.pop_out(key, index, at, None, cx)
                })
            }
            PaneMessage::PopIn => windows.update(cx, |windows, cx| windows.pop_in(key, cx)),
            PaneMessage::Open(program) => {
                _ = windows.update(cx, |windows, cx| windows.open_program(key, program, cx))
            }
            message => self.desk.update(cx, |desk, cx| desk.moved_by(message, cx)),
        }
        self.panes.update(cx, |panes, _| panes.panes_moved = true);
        cx.notify();
    }

    pub(super) fn open_here(
        &mut self,
        index: usize,
        program: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Focus(index), window, cx);
        self.open_view(program, window, cx);
    }

    pub(super) fn open_view(
        &mut self,
        program: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Open(program), window, cx);
    }
}
