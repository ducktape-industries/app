//! One window's panes: which program is where, their frames on the desk,
//! their stacking, which is in front, which the keyboard holds
//! (`ui::layout::Layout`). Its layers observe it; its methods are its one
//! writer, each a compared edit of the layout (`Slice::edit`), so a move
//! that changes nothing notifies nobody. What crosses windows (a pane
//! popped out or back in, a pop-out closing with its pane) is `Windows`'.
use super::Slice;
use crate::ui::layout::{Frame, Layout, Pane, PaneMessage, Side};
use gpui_kit::Context;

pub(crate) struct Desk(Slice<Layout>);

impl Desk {
    pub(crate) fn new(layout: Layout) -> Self {
        Self(Slice::new(layout))
    }
}

impl std::ops::Deref for Desk {
    type Target = Slice<Layout>;

    fn deref(&self) -> &Slice<Layout> {
        &self.0
    }
}

impl std::ops::DerefMut for Desk {
    fn deref_mut(&mut self) -> &mut Slice<Layout> {
        &mut self.0
    }
}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.pane")
}

impl Desk {
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

    pub(crate) fn select(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.select(program), cx);
    }

    pub(crate) fn open(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.open(program), cx);
    }

    pub(crate) fn split(&mut self, program: &'static str, cx: &mut Context<Self>) {
        self.moved(|layout| layout.split(program), cx);
    }

    pub(crate) fn focus(&mut self, index: usize, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.focus(index), cx);
    }

    pub(crate) fn close(&mut self, index: usize, cx: &mut Context<Self>) -> Option<Pane> {
        let mut taken = None;
        self.moved(|layout| taken = layout.close(index), cx);
        taken
    }

    pub(crate) fn popin(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.moved(|layout| layout.popin(pane), cx);
    }

    pub(crate) fn cycle(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.moved(|layout| _ = layout.cycle(forward), cx);
    }

    pub(crate) fn fill(&mut self, cx: &mut Context<Self>) {
        self.moved(Layout::toggle_fill, cx);
    }

    pub(crate) fn snap(&mut self, side: Side, cx: &mut Context<Self>) {
        self.moved(|layout| layout.snap(side), cx);
    }

    pub(crate) fn set_frame(&mut self, index: usize, frame: Frame, cx: &mut Context<Self>) {
        self.moved(
            |layout| {
                let desk = layout.desk();
                layout.set_frame(index, frame, desk);
            },
            cx,
        );
    }

    pub(crate) fn hold(&mut self, index: usize, cx: &mut Context<Self>) {
        self.moved(|layout| layout.hold(index), cx);
    }

    pub(crate) fn release(&mut self, keep: bool, cx: &mut Context<Self>) {
        self.moved(
            |layout| {
                let desk = layout.desk();
                layout.release(keep, desk);
            },
            cx,
        );
    }

    pub(crate) fn drop_hold(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(|layout| layout.held = None, cx);
    }

    pub(crate) fn settle(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(Layout::settle, cx);
    }

    pub(crate) fn set_title(&mut self, instance: u64, title: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(|layout| _ = layout.set_title(instance, Some(title)), cx);
    }

    pub(crate) fn set_showing(&mut self, instance: u64, route: String, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(|layout| _ = layout.set_showing(instance, Some(route)), cx);
    }

    pub(crate) fn take_up(&mut self, kept: Layout, cx: &mut Context<Self>) {
        self.moved(
            |layout| {
                let opened = std::mem::replace(layout, kept);
                if let Some(desk) = opened.desk {
                    layout.measure(desk);
                }
                for pane in opened.panes {
                    layout.popin(pane);
                }
            },
            cx,
        );
    }

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

    pub(crate) fn seed(&mut self, program: &'static str, cx: &mut Context<Self>) {
        if self.get().initialized {
            return;
        }
        self.moved(|layout| _ = layout.select(program), cx);
    }

    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        self.edit(Layout::clear, cx);
    }

    pub(crate) fn moved_by(&mut self, message: PaneMessage, cx: &mut Context<Self>) {
        match message {
            PaneMessage::Select(program) => self.select(program, cx),
            PaneMessage::Open(program) => self.open(program, cx),
            PaneMessage::Split(program) => self.split(program, cx),
            PaneMessage::Focus(index) => self.focus(index, cx),
            PaneMessage::Cycle { forward } => self.cycle(forward, cx),
            PaneMessage::Fill => self.fill(cx),
            PaneMessage::Snap(side) => self.snap(side, cx),
            PaneMessage::Frame(index, frame) => self.set_frame(index, frame, cx),
            PaneMessage::Hold(index) => self.hold(index, cx),
            PaneMessage::Release { keep } => self.release(keep, cx),
            PaneMessage::Close(index) => _ = self.close(index, cx),
            PaneMessage::PopOut { .. } | PaneMessage::PopIn => {}
        }
    }

    pub(crate) fn holds(&self, program: &str) -> bool {
        self.get().panes.iter().any(|pane| pane.program == program)
    }
}
