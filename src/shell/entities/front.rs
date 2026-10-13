//! What the chrome lists. `Front`, what one window's desk shows: the
//! focused pane's program, and every window on the desk in layout order
//! (its program, its instance, its title). Never a frame: a drag moves the
//! `Desk` and leaves this still. `Popped`, the pop-outs' windows beside it
//! (`Windows` derives it): the console's sidebar lists both.
use super::{Desk, Slice};
use crate::runtime::WindowKey;
use crate::ui::layout::{Layout, Pane};
use gpui_kit::{App, AppContext as _, Entity};

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Front {
    pub(crate) focused: Option<&'static str>,
    /// The focused window's instance.
    pub(crate) front: Option<u64>,
    /// Every window on the desk, in layout order.
    pub(crate) open: Vec<Listed>,
}

/// A window, as the chrome lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Listed {
    pub(crate) module: &'static str,
    pub(crate) instance: u64,
    pub(crate) title: Option<String>,
    /// The pop-out it is alone in; `None` on the desk the list is read off.
    pub(crate) away: Option<WindowKey>,
}

impl Listed {
    pub(crate) fn of(pane: &Pane, away: Option<WindowKey>) -> Self {
        Self {
            module: pane.module,
            instance: pane.instance,
            title: pane.title.clone(),
            away,
        }
    }
}

/// The pop-outs' windows, as the console's chrome lists them beside its
/// own desk's.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Popped {
    /// Each pop-out's window, in the order the pop-outs opened.
    pub(crate) open: Vec<Listed>,
    /// Its instance, while the OS window in front is a pop-out.
    pub(crate) front: Option<u64>,
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
                .map(|pane| Listed::of(pane, None))
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
