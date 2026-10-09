//! What one window's desk shows, as the chrome reads it: the focused pane's
//! program, and every window on the desk in layout order (its program, its
//! instance, its title), which the sidebar lists. Never a frame: a drag
//! moves the `Desk` and leaves this still.
use super::{Desk, Slice};
use crate::ui::layout::Layout;
use gpui_kit::{App, AppContext as _, Entity};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Front {
    pub(crate) focused: Option<&'static str>,
    /// The focused window's instance.
    pub(crate) front: Option<u64>,
    /// Every window on the desk, in layout order.
    pub(crate) open: Vec<Listed>,
}

/// A window on the desk, as the chrome lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Listed {
    pub(crate) module: &'static str,
    pub(crate) instance: u64,
    pub(crate) title: Option<String>,
}

impl Front {
    fn of(layout: &Layout) -> Self {
        let front = layout.panes.get(layout.focused);
        Self {
            focused: front.map(|pane| pane.module),
            front: front.map(|pane| pane.instance),
            open: layout
                .panes
                .iter()
                .map(|pane| Listed {
                    module: pane.module,
                    instance: pane.instance,
                    title: pane.title.clone(),
                })
                .collect(),
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
