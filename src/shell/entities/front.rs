//! What one window's desk shows, as the bar reads it: the focused pane's
//! program and every program on the desk, in layout order. Never a frame:
//! a drag moves the `Desk` and leaves this still.
use super::{Desk, Slice};
use crate::ui::layout::Layout;
use gpui_kit::{App, AppContext as _, Entity};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Front {
    pub(crate) focused: Option<&'static str>,
    pub(crate) open: Vec<&'static str>,
}

impl Front {
    fn of(layout: &Layout) -> Self {
        Self {
            focused: layout.panes.get(layout.focused).map(|pane| pane.module),
            open: layout.panes.iter().map(|pane| pane.module).collect(),
        }
    }

    /// The `Front` of `desk`, derived: the observer made here is its one
    /// writer, and it notifies only when the value moved.
    pub(crate) fn of_desk(desk: &Entity<Desk>, cx: &mut App) -> Entity<Slice<Front>> {
        let first = Self::of(desk.read(cx).get());
        cx.new(|cx| {
            cx.observe(desk, |front: &mut Slice<Front>, desk, cx| {
                front.set(Self::of(desk.read(cx).get()), cx);
            })
            .detach();
            Slice::new(first)
        })
    }
}
