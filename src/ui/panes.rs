//! The windows' panes: what each shows, where, and which is in front. The
//! shell draws `layouts` and reports each desk's size; everything that
//! moves a pane is a message here.

use super::layout::{Layout, PaneMessage};
use super::{AppMessage as Message, Ducktape};
use crate::shell::{WindowKey, WindowKind};
use view_wire::Task;

impl Ducktape {
    /// One window's panes, moved.
    pub(super) fn on_pane(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DeskShown { window, desk, seed } => {
                let layout = self.layouts.entry(window).or_default();
                layout.desk = Some(desk);
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
        let mut shows = true;
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
                if layout.panes.get(index).is_some_and(|pane| !pane.is_empty())
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
            PaneMessage::Halve { below } => drop(layout.halve(below, desk)),
            PaneMessage::Cycle { forward } => drop(layout.cycle(forward)),
            PaneMessage::Pick { down, rows } => {
                let last = rows.saturating_sub(1);
                let pick = layout.pick.min(last);
                layout.pick = match down {
                    true => (pick + 1).min(last),
                    false => pick.saturating_sub(1),
                };
                shows = false;
            }
            PaneMessage::Fill(index) => {
                layout.toggle_fill(index, desk);
                shows = false;
            }
            PaneMessage::Frame(index, frame) => {
                layout.set_frame(index, frame, desk);
                shows = false;
            }
        }
        if let Some(layout) = self.layouts.get_mut(&key) {
            layout.settle();
            // the window in front is the active program, however it got there
            if shows && let Some(module) = layout.shown() {
                self.active = Some(module);
                self.badges.remove(module);
            }
        }
        task
    }

    /// The console's desk, made if it has none yet; `None` before the
    /// console window is opened.
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
        assert_eq!(
            state.layouts[&console].panes[0].frame,
            Some(Frame::fill(DESK))
        );
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
        pane(&mut state, console, PaneMessage::Halve { below: false });
        assert_eq!(modules(&state, console), ["calendar", EMPTY, "files"]);
        pane(&mut state, console, PaneMessage::Open("chat"));
        assert_eq!(modules(&state, console), ["calendar", "chat", "files"]);
    }

    /// ⌘D halves, ⌘⇧D halves below, ⌘1–9 focus, ⌘` cycles; closing a
    /// half gives its sibling the space back (#295).
    #[test]
    fn the_desk_keys_move_the_model() {
        let (mut state, console) = desk();
        let whole = state.layouts[&console].panes[0].frame;
        pane(&mut state, console, PaneMessage::Halve { below: false });
        pane(&mut state, console, PaneMessage::Halve { below: true });
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
        assert_eq!(state.layouts[&console].panes[0].frame, whole, "#295");
        // an empty window's list: ↓ past the end stays on the last row
        pane(&mut state, console, PaneMessage::Halve { below: false });
        for _ in 0..5 {
            pane(
                &mut state,
                console,
                PaneMessage::Pick {
                    down: true,
                    rows: 3,
                },
            );
        }
        assert_eq!(state.layouts[&console].pick, 2);
        pane(
            &mut state,
            console,
            PaneMessage::Pick {
                down: false,
                rows: 3,
            },
        );
        assert_eq!(state.layouts[&console].pick, 1);
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
        pane(&mut state, console, PaneMessage::Halve { below: false });
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
