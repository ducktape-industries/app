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
    pub(crate) front: Option<u64>,
    pub(crate) open: Vec<Listed>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Listed {
    pub(crate) program: &'static str,
    pub(crate) instance: u64,
    pub(crate) title: Option<String>,
    pub(crate) away: Option<WindowKey>,
}

impl Listed {
    pub(crate) fn of(pane: &Pane, away: Option<WindowKey>) -> Self {
        Self {
            program: pane.program,
            instance: pane.instance,
            title: pane.title.clone(),
            away,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Popped {
    pub(crate) open: Vec<Listed>,
    pub(crate) front: Option<u64>,
}

impl Front {
    fn of(layout: &Layout) -> Self {
        let front = layout.panes.get(layout.focused);
        Self {
            focused: front.map(|pane| pane.program),
            front: front.map(|pane| pane.instance),
            open: layout
                .panes
                .iter()
                .map(|pane| Listed::of(pane, None))
                .collect(),
        }
    }

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
