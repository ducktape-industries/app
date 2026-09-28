//! The windows' panes: what each shows, where, and which is in front. The
//! shell draws `layouts` and reports each desk's size; everything that
//! moves a pane is a message here.

use super::layout::{Layout, PaneMessage};
use super::{AppMessage as Message, Ducktape};
use crate::shell::{WindowKey, WindowKind};
use crate::ui::task::Task;

impl Ducktape {
    /// One window's panes, moved.
    pub(super) fn on_pane(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DeskShown { window, desk, seed } => {
                let layout = self.layouts.entry(window).or_default();
                layout.measure(desk);
                if !layout.initialized
                    && let Some(module) = seed
                {
                    layout.select(module);
                    layout.initialized = true;
                }
                layout.settle();
                Task::none()
            }
            Message::Pane(window, pane) => self.pane(window, pane),
            _ => unreachable!("routed by `update`"),
        }
    }

    fn pane(&mut self, key: WindowKey, message: PaneMessage) -> Task<Message> {
        let console = self.console_win;
        let Some(layout) = self.layouts.get_mut(&key) else {
            return Task::none();
        };
        layout.initialized = true;
        let desk = layout.desk();
        let mut task = Task::none();
        // filling or dragging a window does not change the active program
        let mut sets_active = true;
        match message {
            PaneMessage::Select(module) => drop(layout.select(module)),
            PaneMessage::Open(module) => drop(layout.open(module)),
            PaneMessage::Split(module) => drop(layout.split(module)),
            PaneMessage::Focus(index) => drop(layout.focus(index)),
            PaneMessage::Close(index) => {
                layout.close(index);
                // a pop-out is its one pane: it goes with it
                if console != Some(key) {
                    task = crate::shell::close(key);
                }
            }
            PaneMessage::PopOut { index, at } => {
                if layout.panes.get(index).is_some_and(|pane| pane.is_view())
                    && let Some(pane) = layout.close(index)
                {
                    let kind = WindowKind::View {
                        module: pane.module,
                    };
                    let mut own = Layout::default();
                    own.popin(pane);
                    own.initialized = true;
                    let (window, opened) = crate::shell::open_at(kind, at);
                    self.layouts.insert(window, own);
                    task = opened.discard();
                }
            }
            PaneMessage::PopIn => {
                if let Some(console) = console.filter(|console| *console != key)
                    && let Some(pane) = layout.close(0)
                {
                    let desk = self.layouts.entry(console).or_default();
                    desk.popin(pane);
                    desk.initialized = true;
                    desk.settle();
                    task = crate::shell::close(key);
                }
            }
            PaneMessage::Cycle { forward } => drop(layout.cycle(forward)),
            PaneMessage::Fill(index) => {
                layout.toggle_fill(index, desk);
                sets_active = false;
            }
            PaneMessage::Frame(index, frame) => {
                layout.set_frame(index, frame, desk);
                sets_active = false;
            }
        }
        if let Some(layout) = self.layouts.get_mut(&key) {
            layout.settle();
            // the window in front is the active program, however it got there
            if sets_active && let Some(module) = layout.shown() {
                self.active = Some(module);
            }
        }
        task
    }

    /// Help on the desk: into the focused window if it is empty, else
    /// where it already is, else a window of its own. A new account starts
    /// here, greeted (`welcome`).
    pub(super) fn open_help(&mut self, welcome: bool) {
        self.welcome = welcome;
        if let Some(desk) = self.desk_layout() {
            desk.open(super::layout::HELP);
            desk.settle();
        }
    }

    /// The console's desk, made if it has none yet; `None` before the
    /// console window is opened. Marks it initialized: a desk something was
    /// put on no longer opens the active program on its own.
    pub(super) fn desk_layout(&mut self) -> Option<&mut Layout> {
        let layout = self.layouts.entry(self.console_win?).or_default();
        layout.initialized = true;
        Some(layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout::{EMPTY, Frame};

    const DESK: (f32, f32) = (1400., 860.);

    /// A desk with `chat` open, measured, as the console first draws it.
    fn desk() -> (Ducktape, WindowKey) {
        let (mut state, _) = Ducktape::boot();
        let console = WindowKey::unique();
        state.console_win = Some(console);
        let _ = state.update(Message::DeskShown {
            window: console,
            desk: DESK,
            seed: Some("chat"),
        });
        (state, console)
    }

    fn pane(state: &mut Ducktape, window: WindowKey, message: PaneMessage) {
        let _ = state.update(Message::Pane(window, message));
    }

    fn modules(state: &Ducktape, window: WindowKey) -> Vec<&'static str> {
        state.layouts[&window]
            .panes
            .iter()
            .map(|pane| pane.module)
            .collect()
    }

    #[test]
    fn an_untouched_desk_opens_its_seed_once_and_every_window_has_a_frame() {
        let (mut state, console) = desk();
        assert_eq!(modules(&state, console), ["chat"]);
        assert!(state.layouts[&console].panes[0].frame.is_some());
        pane(&mut state, console, PaneMessage::Close(0));
        let _ = state.update(Message::DeskShown {
            window: console,
            desk: DESK,
            seed: Some("files"),
        });
        assert!(
            modules(&state, console).is_empty(),
            "a closed desk reopened"
        );
    }

    /// The bar: a click fills an empty focused window, focuses the window a
    /// program is already in, else opens one of its own; shift puts it in
    /// the focused window in place of what it showed.
    #[test]
    fn the_bar_fills_focuses_opens_or_replaces() {
        let (mut state, console) = desk();
        pane(&mut state, console, PaneMessage::Open("files"));
        assert_eq!(modules(&state, console), ["chat", "files"]);
        assert_eq!(state.active, Some("files"));
        pane(&mut state, console, PaneMessage::Open("chat"));
        assert_eq!(state.layouts[&console].focused, 0, "focused where it was");
        assert_eq!(state.active, Some("chat"));
        pane(&mut state, console, PaneMessage::Select("calendar"));
        assert_eq!(modules(&state, console), ["calendar", "files"]);
        pane(&mut state, console, PaneMessage::Split(EMPTY));
        assert_eq!(modules(&state, console), ["calendar", EMPTY, "files"]);
        pane(&mut state, console, PaneMessage::Open("chat"));
        assert_eq!(modules(&state, console), ["calendar", "chat", "files"]);
    }

    /// ⌘N opens an empty window, ⌘1–9 focus, ⌘` cycles, ⌘W closes.
    #[test]
    fn the_desk_keys_move_the_model() {
        let (mut state, console) = desk();
        let whole = state.layouts[&console].panes[0].frame;
        pane(&mut state, console, PaneMessage::Split(EMPTY));
        pane(&mut state, console, PaneMessage::Split(EMPTY));
        assert_eq!(state.layouts[&console].panes.len(), 3);
        pane(&mut state, console, PaneMessage::Focus(0));
        assert_eq!(
            (state.layouts[&console].focused, state.active),
            (0, Some("chat"))
        );
        pane(&mut state, console, PaneMessage::Cycle { forward: true });
        assert_ne!(state.layouts[&console].focused, 0);
        pane(&mut state, console, PaneMessage::Close(2));
        pane(&mut state, console, PaneMessage::Close(1));
        assert_eq!(state.layouts[&console].panes[0].frame, whole, "untouched");
    }

    #[test]
    fn a_drag_and_a_double_press_move_frames_on_the_measured_desk() {
        let (mut state, console) = desk();
        let small = Frame {
            x: 100.,
            y: 80.,
            w: 500.,
            h: 400.,
        };
        pane(&mut state, console, PaneMessage::Frame(0, small));
        assert_eq!(state.layouts[&console].panes[0].frame, Some(small));
        pane(&mut state, console, PaneMessage::Fill(0));
        assert_eq!(
            state.layouts[&console].panes[0].frame,
            Some(Frame::fill(DESK))
        );
        pane(&mut state, console, PaneMessage::Fill(0));
        assert_eq!(state.layouts[&console].panes[0].frame, Some(small));
    }

    /// A pane popped out keeps its instance (its view) in a window of its
    /// own; popped back in, it lands on the desk and its window goes.
    #[test]
    fn pop_out_and_back_in_keep_the_pane() {
        let (mut state, console) = desk();
        pane(&mut state, console, PaneMessage::Split("files"));
        let files = state.layouts[&console].panes[1].instance;
        pane(
            &mut state,
            console,
            PaneMessage::PopOut { index: 1, at: None },
        );
        assert_eq!(modules(&state, console), ["chat"]);
        let (&popped, own) = state
            .layouts
            .iter()
            .find(|(key, _)| **key != console)
            .expect("a window of its own");
        assert_eq!(own.panes[0].instance, files);
        pane(&mut state, popped, PaneMessage::PopIn);
        assert!(state.layouts[&popped].panes.is_empty());
        assert_eq!(state.layouts[&console].panes[1].instance, files);
        let _ = state.update(Message::WindowWasClosed(popped));
        assert!(!state.layouts.contains_key(&popped));
        // an empty window has no view to carry out
        pane(&mut state, console, PaneMessage::Split(EMPTY));
        let before = state.layouts.len();
        let focused = state.layouts[&console].focused;
        pane(
            &mut state,
            console,
            PaneMessage::PopOut {
                index: focused,
                at: None,
            },
        );
        assert_eq!(state.layouts.len(), before);
    }

    /// A window is placed before its view comes (60% of the desk, centred);
    /// once the view is seated, it widens to the view's minimum and its
    /// border, pulled left as far as it takes to stay on the desk, and on a
    /// desk narrower than that it is the desk.
    #[test]
    fn a_window_widens_to_its_view_once_the_view_is_seated() {
        for (module, desk, min_width, placed, widened) in [
            ("seated-wide-view", 1000., 680, (200., 600.), (200., 682.)),
            // the console at its smallest, forge opening in it
            ("seated-console-view", 720., 640, (144., 432.), (78., 642.)),
            ("seated-cramped-view", 600., 680, (120., 360.), (0., 600.)),
        ] {
            let (mut state, _) = Ducktape::boot();
            let console = WindowKey::unique();
            state.console_win = Some(console);
            let _ = state.update(Message::DeskShown {
                window: console,
                desk: (desk, 700.),
                seed: Some(module),
            });
            let frame = state.layouts[&console].panes[0].frame.unwrap();
            assert_eq!((frame.x, frame.w), placed, "{module}");
            crate::runtime::seat_for_test(module, min_width);
            let seated = crate::runtime::Intent::Seated;
            let _ = state.update(Message::ViewEvent(module, seated));
            let frame = state.layouts[&console].panes[0].frame.unwrap();
            assert_eq!((frame.x, frame.w), widened, "{module}");
            assert!(frame.x + frame.w <= desk, "{module}: {frame:?}");
        }
    }

    /// The model's own asks land in the console's layout at once, each of
    /// them: two in a row no longer overwrite each other.
    #[test]
    fn spotlight_and_links_both_land_in_the_desk() {
        let (mut state, console) = desk();
        crate::runtime::list_for_test("pane-link");
        let _ = state.update(Message::SelectView("forge"));
        let _ = state.update(Message::OpenLink("duck://pane-link/x".into()));
        assert_eq!(modules(&state, console), ["forge", "pane-link"]);
        assert_eq!(state.active, Some("pane-link"));
        // leaving the network empties every window, keeping its measure
        let _ = state.update(Message::Disconnect);
        assert!(modules(&state, console).is_empty());
        assert_eq!(state.layouts[&console].desk, Some(DESK));
        assert!(!state.layouts[&console].initialized);
    }
}
