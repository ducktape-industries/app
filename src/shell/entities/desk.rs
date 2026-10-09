//! One window's panes: which program is where, their frames on the desk,
//! their stacking, which is in front, which the keyboard holds
//! (`ui::layout::Layout`). Its layers observe it; its methods are its one
//! writer, each a compared edit of the layout (`Slice::edit`), so a move
//! that changes nothing notifies nobody. What crosses windows (a pane
//! popped out or back in, a pop-out closing with its pane) is `Windows`'.
use super::Slice;
use crate::ui::layout::{Frame, Layout, Pane, PaneMessage};
use gpui_kit::Context;

pub(crate) type Desk = Slice<Layout>;

/// Every write here is timed as `reducer.pane` (docs/perf.md: a
/// `reducer.*` sample past 250 ms is a hang).
fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.pane")
}

impl Slice<Layout> {
    /// A move of the panes: `edit` on the layout, then frames for what has
    /// none and the hold checked against the front. A desk something was
    /// done to no longer opens the active program on its own.
    fn moved(&mut self, edit: impl FnOnce(&mut Layout), cx: &mut Context<Self>) -> bool {
        let _timed = timed();
        self.edit(
            |layout| {
                layout.initialized = true;
                edit(layout);
                layout.settle();
            },
            cx,
        )
    }

    /// Shift-click on the bar: the window `module` is already in comes to
    /// the front, else it takes the focused window's place.
    pub(crate) fn select(&mut self, module: &'static str, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.select(module), cx);
    }

    /// A bar click, a pick, a link: into an empty focused window, else the
    /// window it is in, else one of its own (`Layout::open`).
    pub(crate) fn open(&mut self, module: &'static str, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.open(module), cx);
    }

    /// Another pane showing `module`.
    pub(crate) fn split(&mut self, module: &'static str, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.split(module), cx);
    }

    pub(crate) fn focus(&mut self, index: usize, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.focus(index), cx);
    }

    /// Pane `index` gone: the pane, for whoever takes it (a pop-out).
    pub(crate) fn close(&mut self, index: usize, cx: &mut Context<Self>) -> Option<Pane> {
        let mut taken = None;
        self.moved(|layout| taken = layout.close(index), cx);
        taken
    }

    /// A pane from another window onto this desk.
    pub(crate) fn popin(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.popin(pane), cx);
    }

    /// ⌘` / ⌘⇧` / ctrl-tab.
    pub(crate) fn cycle(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.cycle(forward), cx);
    }

    /// A title bar's double press, ⌘⇧↩, Restore: the window in front
    /// fills the desk, or every window goes back.
    pub(crate) fn fill(&mut self, cx: &mut Context<Self>) {
        self.moved(Layout::toggle_fill, cx);
    }

    /// A drag moved or sized a window: compared, so a pointer that stays
    /// put notifies nothing.
    pub(crate) fn set_frame(&mut self, index: usize, frame: Frame, cx: &mut Context<Self>) {
        self.moved(
            |layout| {
                let desk = layout.desk();
                layout.set_frame(index, frame, desk);
            },
            cx,
        );
    }

    /// ⌘⇧M: the keyboard holds the window (again, and it lets go), fill
    /// turned off first.
    pub(crate) fn hold(&mut self, index: usize, cx: &mut Context<Self>) {
        self.moved(|layout| layout.hold(index), cx);
    }

    /// The keyboard lets go of the window it holds: `keep` where it is, or
    /// else back where it was.
    pub(crate) fn release(&mut self, keep: bool, cx: &mut Context<Self>) {
        self.moved(
            |layout| {
                let desk = layout.desk();
                layout.release(keep, desk);
            },
            cx,
        );
    }

    /// The keys left the OS window: a hold on one of its panes ends, the
    /// window where it is.
    pub(crate) fn drop_hold(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(|layout| layout.held = None, cx);
    }

    /// A window placed before its view came is widened to it (`Seated`).
    pub(crate) fn settle(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(Layout::settle, cx);
    }

    /// The desk measured this size (the pane layer's frame callback).
    pub(crate) fn resize(&mut self, desk: (f32, f32), cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(
            |layout| {
                layout.measure(desk);
                layout.settle();
            },
            cx,
        );
    }

    /// An untouched desk opens `module` once, on its first draw.
    pub(crate) fn seed(&mut self, module: &'static str, cx: &mut Context<Self>) {
        if self.get().initialized {
            return;
        }
        self.moved(|layout| _ = layout.select(module), cx);
    }

    /// The network in hand was left: every pane goes, the desk keeping
    /// its measure (and opening the active program again once there is one).
    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(Layout::clear, cx);
    }

    /// A `PaneMessage` that stays on this desk: the method it names.
    /// (Close, PopOut and PopIn cross windows: `Windows` takes them.)
    pub(crate) fn moved_by(&mut self, message: PaneMessage, cx: &mut Context<Self>) {
        match message {
            PaneMessage::Select(module) => self.select(module, cx),
            PaneMessage::Open(module) => self.open(module, cx),
            PaneMessage::Split(module) => self.split(module, cx),
            PaneMessage::Focus(index) => self.focus(index, cx),
            PaneMessage::Cycle { forward } => self.cycle(forward, cx),
            PaneMessage::Fill => self.fill(cx),
            PaneMessage::Frame(index, frame) => self.set_frame(index, frame, cx),
            PaneMessage::Hold(index) => self.hold(index, cx),
            PaneMessage::Release { keep } => self.release(keep, cx),
            // desk-only (tests): a pop-out's window stays; panes.rs routes
            // Close to `Windows::close_pane`
            PaneMessage::Close(index) => _ = self.close(index, cx),
            PaneMessage::PopOut { .. } | PaneMessage::PopIn => {}
        }
    }

    /// Whether `module` is on this desk.
    pub(crate) fn holds(&self, module: &str) -> bool {
        self.get().panes.iter().any(|pane| pane.module == module)
    }
}
