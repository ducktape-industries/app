//! View entities belong to panes, including when a pane changes OS windows.
use super::*;

pub(super) struct MountedPane {
    pub(super) module: &'static str,
    pub(super) view: Entity<crate::runtime::NativeModuleView>,
    pub(super) route: Option<gpui_kit::Subscription>,
}

pub(super) fn label(module: &str) -> String {
    if module == layout::EMPTY {
        return "Empty".to_owned();
    }
    crate::runtime::rail()
        .into_iter()
        .find(|row| row.module == module)
        .map(|row| row.label)
        .unwrap_or_else(|| module.to_owned())
}

/// What an empty pane strip says: the rail lists what the network runs, so
/// an empty pane list beside a non-empty rail means every pane here was
/// closed or popped out, not that the network has nothing to show — that
/// reused the "no program" sentence and read as if Chat/Forge/Settings had
/// vanished from the rail right beside it.
fn empty_panes_message(rail: &[crate::runtime::RailRow]) -> String {
    match rail.iter().any(|row| !row.empty) {
        true => format!("Press {} to open something.", chord_label("K")),
        false => "This network runs no program with a view.".into(),
    }
}

/// What an empty window lists: the rail's programs, as the menu bar shows them.
fn openable() -> Vec<crate::runtime::RailRow> {
    crate::runtime::rail()
        .into_iter()
        .filter(|row| !row.empty)
        .collect()
}

impl DesktopWindow {
    pub(super) fn focused_view(&self) -> Option<Entity<crate::runtime::NativeModuleView>> {
        self.layout
            .panes
            .get(self.layout.focused)
            .and_then(|pane| self.mounted.get(&pane.instance))
            .map(|pane| pane.view.clone())
    }

    pub(super) fn initialize_panes(
        &mut self,
        module: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if !self.initialized {
            let module = match self.kind {
                crate::shell::WindowKind::View { module } => Some(module),
                _ => module,
            };
            if let Some(module) = module {
                self.layout.select(module);
                self.initialized = true;
            }
        }
        self.sync_panes(cx);
    }

    fn sync_panes(&mut self, cx: &mut Context<Self>) {
        let obsolete: Vec<_> = self
            .mounted
            .keys()
            .copied()
            .filter(|id| !self.layout.panes.iter().any(|pane| pane.instance == *id))
            .collect();
        for id in obsolete {
            if let Some(pane) = self.mounted.remove(&id) {
                self.hide_pane(pane, cx);
            }
        }
        for pane in self.layout.panes.iter().filter(|pane| !pane.is_empty()) {
            self.mounted.entry(pane.instance).or_insert_with(|| {
                let module = pane.module;
                let view = cx.new(|_| crate::runtime::NativeModuleView::new(module));
                MountedPane {
                    module,
                    view,
                    route: None,
                }
            });
            let mounted = self.mounted.get_mut(&pane.instance).expect("mounted pane");
            if mounted.route.is_none() {
                let model = self.model.clone();
                let module = pane.module;
                mounted.route = Some(cx.subscribe(&mounted.view, move |_, _, event, cx| {
                    model.update(cx, |model, cx| {
                        model.dispatch(Message::ViewEvent(module, event.clone()), cx)
                    });
                }));
            }
        }
    }

    pub(super) fn hide_pane(&self, pane: MountedPane, cx: &mut gpui_kit::App) {
        let intents = pane.view.update(cx, |view, _| view.hide());
        for intent in intents {
            self.model.update(cx, |model, cx| {
                model.dispatch(Message::ViewEvent(pane.module, intent), cx)
            });
        }
    }

    pub(super) fn pane_message(
        &mut self,
        message: PaneMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.initialized = true;
        match message {
            PaneMessage::Select(module) => {
                self.layout.select(module);
            }
            PaneMessage::Split(module) => {
                self.layout.split(module);
            }
            PaneMessage::PopOut(index)
                if self
                    .layout
                    .panes
                    .get(index)
                    .is_none_or(|pane| pane.is_empty()) => {}
            PaneMessage::Close(index) => {
                self.layout.close(index);
                if matches!(self.kind, crate::shell::WindowKind::View { .. }) {
                    super::remove(window.window_handle(), cx);
                }
            }
            PaneMessage::Focus(index) => {
                self.layout.focus(index);
            }
            PaneMessage::PopOut(index) => {
                if let Some(pane) = self.layout.close(index)
                    && let Some(mut mounted) = self.mounted.remove(&pane.instance)
                {
                    mounted.route = None;
                    let kind = crate::shell::WindowKind::View {
                        module: pane.module,
                    };
                    let source = cx.weak_entity();
                    let at = super::windows::unseated(
                        window.bounds(),
                        pane.frame,
                        window.display(cx).map(|display| display.bounds()),
                    );
                    self.model.update(cx, |model, cx| {
                        let (reply, _) = oneshot::channel();
                        model.open_window(
                            WindowKey::unique(),
                            kind,
                            reply,
                            Some((pane, mounted, source)),
                            Some(at),
                            cx,
                        );
                    });
                }
            }
            PaneMessage::PopIn => {
                let destination = self
                    .model
                    .read(cx)
                    .views
                    .values()
                    .filter_map(|view| view.upgrade())
                    .find(|view| {
                        view.entity_id() != cx.entity_id()
                            && view.read(cx).kind == crate::shell::WindowKind::Console
                    });
                if let Some(destination) = destination
                    && let Some(pane) = self.layout.close(0)
                    && let Some(mut mounted) = self.mounted.remove(&pane.instance)
                {
                    mounted.route = None;
                    destination.update(cx, |this, cx| {
                        this.mounted.insert(pane.instance, mounted);
                        this.layout.popin(pane);
                        this.initialized = true;
                        this.sync_panes(cx);
                        cx.notify();
                    });
                    super::remove(window.window_handle(), cx);
                }
            }
        }
        self.settle(window, cx);
    }

    /// After the windows changed: views mounted for them, the focused one
    /// told so (and the model: it is the active program), keys back at the
    /// desk.
    fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_panes(cx);
        let shown = self
            .layout
            .panes
            .get(self.layout.focused)
            .filter(|pane| !pane.is_empty())
            .map(|pane| pane.module);
        if let Some(module) = shown {
            self.model.update(cx, |model, cx| {
                model.dispatch(Message::ViewShown(module), cx)
            });
        }
        for (index, pane) in self.layout.panes.iter().enumerate() {
            if let Some(mounted) = self.mounted.get(&pane.instance) {
                mounted.view.update(cx, |view, cx| {
                    view.set_focused(index == self.layout.focused, cx)
                });
            }
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// A menu bar click, or a pick in an empty window: see `Layout::open`.
    pub(super) fn open_view(
        &mut self,
        module: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.initialized = true;
        self.layout.open(module);
        self.settle(window, cx);
    }

    /// The desk's own keys, in the console (⌘W is `global_key`'s):
    /// ⌘` / ⌘⇧` (and ctrl-tab) go to the next / previous one, ⌘1…⌘9 to
    /// the Nth, ⌘D / ⌘⇧D halve it; in an empty window ↑↓ pick, Enter and
    /// 1…9 open. True if the key was the desk's.
    pub(super) fn desk_key(
        &mut self,
        key: &KeyPress,
        in_guest_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let command = crate::runtime::command_held(key.modifiers);
        let shift = key.modifiers.shift;
        let name = key.key.to_ascii_lowercase();
        let digit = match name.as_str() {
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => name.parse::<usize>().ok(),
            _ => None,
        };
        let cycle =
            (command && (name == "`" || name == "~")) || (key.modifiers.control && name == "tab");
        if cycle {
            self.layout.cycle(!shift);
        } else if command && name == "d" {
            let desk = self.desk(window);
            self.layout.halve(shift, desk);
        } else if command && let Some(nth) = digit {
            self.layout.focus(nth - 1);
        } else if !in_guest_editor
            && !command
            && !key.modifiers.alt
            && self
                .layout
                .panes
                .get(self.layout.focused)
                .is_some_and(|pane| pane.is_empty())
        {
            let rows = openable();
            let pick = self.layout.pick.min(rows.len().saturating_sub(1));
            let open = match (name.as_str(), digit) {
                ("up", _) => {
                    self.layout.pick = pick.saturating_sub(1);
                    None
                }
                ("down", _) => {
                    self.layout.pick = (pick + 1).min(rows.len().saturating_sub(1));
                    None
                }
                ("enter", _) => rows.get(pick),
                (_, Some(nth)) => rows.get(nth - 1),
                _ => return false,
            };
            match open {
                Some(row) => self.open_view(row.module, window, cx),
                None => cx.notify(),
            }
            return true;
        } else {
            return false;
        }
        self.initialized = true;
        self.settle(window, cx);
        true
    }

    /// An empty window's body (design "A"): what it can open, one row a
    /// program, and the keys that open them.
    fn empty_view(&self, body: f32, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use gpui_kit::*;
        let state = self.model.read(cx).state.clone_facts();
        let ink = Ink::of(state.dark);
        let rows = openable();
        let pick = self.layout.pick.min(rows.len().saturating_sub(1));
        let spacing = empty_spacing(rows.len(), body);
        let (row_pad, outer_pad) = (spacing.row, spacing.outer);
        let list = rows.iter().enumerate().map(|(nth, row)| {
            let module = row.module;
            let picked = nth == pick;
            let name = super::desk::tab_label(row);
            let badge = state.badges.get(module).copied().unwrap_or(0);
            let note = match badge {
                0 => String::new(),
                count => format!("{count} unread"),
            };
            sans(400, 13.)
                .id(SharedString::from(format!("empty/{module}")))
                .control(Role::MenuItem, SharedString::from(name.clone()))
                .flex()
                .items_baseline()
                .gap(px(12.))
                .px(px(12.))
                .py(px(row_pad))
                .cursor_pointer()
                .when(picked, |row| row.bg(ink.surface))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_view(module, window, cx);
                }))
                .child(
                    mono(400, 12.)
                        .w(px(12.))
                        .text_color(ink.strong)
                        .child(match nth < 9 {
                            true => (nth + 1).to_string(),
                            false => String::new(),
                        }),
                )
                .child(sans(400, 15.).text_color(ink.ink).child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.muted)
                        .child(note),
                )
                .child(
                    mono(400, 12.)
                        .text_color(ink.muted)
                        .child(if picked { "↵" } else { "" }),
                )
        });
        // each hint whole, wrapping to the next line on a narrow window
        let hints = [
            format!("1–{} open", rows.len().clamp(1, 9)),
            format!("{} search everything", chord_label("K")),
            format!("{} close", chord_label("W")),
        ]
        .map(|hint| div().whitespace_nowrap().child(hint));
        div()
            .id("empty-window")
            .role(Role::Menu)
            .aria_label("Open in this window")
            // the body's own height, not a share of a parent still waiting on
            // it: the rows scroll and shrink inside it, the footer stays put
            .w_full()
            .h(px(body.max(0.)))
            .flex()
            .flex_col()
            .px(px(16.))
            .py(px(outer_pad))
            .child(
                sans(400, 13.)
                    .text_color(ink.muted)
                    .px(px(12.))
                    .pb(px(8.))
                    .child("Open in this window"),
            )
            .child(
                div()
                    .id("empty-window/rows")
                    .map(|rows| match spacing.shown {
                        Some(shown) => rows.h(px(shown)).flex_none(),
                        None => rows.flex_1().min_h_0(),
                    })
                    .overflow_y_scroll()
                    .children(list),
            )
            .child(
                mono(400, 12.)
                    .flex_shrink_0()
                    .px(px(12.))
                    .pt(px(12.))
                    .flex()
                    .flex_wrap()
                    .gap_x(px(24.))
                    .gap_y(px(4.))
                    .text_color(ink.muted)
                    .children(hints),
            )
            .into_any_element()
    }

    fn pane_button(
        &self,
        index: usize,
        action: PaneAction,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        let hover = ink.surface;
        // The element id is stable for the AX door and tests; the AX name
        // is a phrase a screen reader can announce on its own, not the bare
        // verb.
        let (id, name, glyph) = action.parts();
        crate::a11y::disabled(
            crate::a11y::keyboard(
                div()
                    .id(SharedString::from(format!("pane/{index}/{id}")))
                    .control(Role::Button, name)
                    .size(px(28.))
                    .text_color(ink.muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(enabled, |button| {
                        button.cursor_pointer().hover(move |style| style.bg(hover))
                    })
                    .opacity(if enabled { 1. } else { 0.35 })
                    // a press on a control isn't a hold on the title bar
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if !enabled {
                            return;
                        }
                        let message = match action {
                            PaneAction::Split => {
                                PaneMessage::Split(this.layout.panes[index].module)
                            }
                            PaneAction::Close => PaneMessage::Close(index),
                            PaneAction::PopOut => PaneMessage::PopOut(index),
                            PaneAction::PopIn => PaneMessage::PopIn,
                        };
                        this.pane_message(message, window, cx);
                    }))
                    .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
            ),
            !enabled,
        )
    }

    /// The desk windows sit on: the window below its bar.
    fn desk(&self, window: &Window) -> (f32, f32) {
        let size = window.viewport_size();
        (
            f32::from(size.width),
            f32::from(size.height) - super::desk::BAR,
        )
    }

    pub(super) fn pane_stage(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        if self.layout.panes.is_empty() {
            let moving = self.model.read(cx).state.motion;
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(20.))
                .child(super::spin::drawing(
                    "empty-desk-figure",
                    super::figure::Figure::Node,
                    moving,
                    ink.figure,
                    window,
                    cx,
                ))
                .child(
                    super::ink::mono(400, 12.)
                        .text_color(ink.muted)
                        .child(empty_panes_message(&crate::runtime::rail())),
                )
                .into_any_element();
        }
        let props = self.model.read(cx).state.view_props();
        let console = self.kind == crate::shell::WindowKind::Console;
        let desk = self.desk(window);
        self.layout.place(desk);
        let this = cx.entity();
        let mut stage = div().id("panes").relative().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| raise(this, bounds, window),
            )
            .absolute()
            .size_full(),
        );
        if self.drag.is_some() {
            // window-wide, so a fast pointer can't slip off the window it holds
            let this = cx.entity();
            stage = stage.child(
                canvas(|_, _, _| {}, move |_, _, window, _| follow(this, window))
                    .absolute()
                    .size_full(),
            );
        }
        let multi = self.layout.panes.len() > 1;
        for index in self.layout.stacking() {
            let pane = &self.layout.panes[index];
            let focused = index == self.layout.focused;
            let empty = pane.is_empty();
            let view = match self.mounted.get(&pane.instance) {
                Some(mounted) => {
                    let view = mounted.view.clone();
                    view.update(cx, |view, cx| {
                        view.set_focused(focused, cx);
                        view.set_props(props.clone(), cx);
                    });
                    view.into_any_element()
                }
                None => {
                    let pane = &self.layout.panes[index];
                    let tall = pane
                        .frame
                        .map_or(f32::from(window.viewport_size().height), |frame| frame.h);
                    self.empty_view(tall - TITLE, cx)
                }
            };
            let pane = &self.layout.panes[index];
            let controls = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .when(console, |strip| {
                    strip.child(self.pane_button(
                        index,
                        PaneAction::Split,
                        self.layout.panes.len() < layout::MAX_PANES,
                        cx,
                    ))
                })
                // an empty window has no view to carry out
                .when(console && !empty, |strip| {
                    strip.child(self.pane_button(index, PaneAction::PopOut, true, cx))
                })
                .when(!console, |strip| {
                    strip.child(self.pane_button(index, PaneAction::PopIn, true, cx))
                })
                .child(self.pane_button(index, PaneAction::Close, true, cx));
            let body = div()
                .id(SharedString::from(format!("pane/{index}")))
                .flex()
                .flex_col()
                .bg(ink.bg)
                .overflow_hidden()
                .role(gpui_kit::Role::Group);
            // Every window has a title bar, so a view owns all of its
            // rectangle: nothing floats over its corners.
            let on_desk = console && pane.frame.is_some();
            // A window of its own on macOS draws no title bar of the
            // system's: this bar is its handle, and the traffic lights sit
            // over its left end. Elsewhere the system's bar names it.
            let handle = !on_desk && cfg!(target_os = "macos") && !window.is_fullscreen();
            let title = div()
                .id(SharedString::from(format!("pane/{index}/strip")))
                .h(px(TITLE))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(if handle { 78. } else { 12. }))
                .pr(px(2.))
                .border_b_1()
                .border_color(ink.line)
                .bg(match focused {
                    true => ink.surface,
                    false => ink.bg,
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        match (on_desk, event.click_count) {
                            (true, 2) => {
                                let desk = this.desk(window);
                                this.layout.toggle_fill(index, desk);
                                cx.notify();
                            }
                            (true, _) => this.hold(index, [false; 4], event.position),
                            (false, 2) if handle => window.titlebar_double_click(),
                            (false, _) if handle => window.start_window_move(),
                            (false, _) => {}
                        }
                    }),
                )
                .when(on_desk || handle, |title| {
                    title.child(
                        super::ink::mono(500, 12.)
                            .flex_shrink_0()
                            .text_color(ink.ink)
                            .child(label(pane.module)),
                    )
                })
                // pushes the controls to the bar's right end
                .child(div().flex_1().min_w_0())
                .child(controls);
            let asking = (!empty && crate::runtime::notify::center().asking(pane.module))
                .then(|| self.permission_bar(pane.module, cx));
            let seated = match pane.frame.filter(|_| on_desk) {
                None => body
                    .size_full()
                    .child(title)
                    .children(asking)
                    .child(div().flex_1().min_h_0().w_full().child(view))
                    .into_any_element(),
                // on the desk: a title bar to hold it by, and edges to size
                // it by that reach past its border, so the grips sit in a
                // frame `GRAB` wider than the window (the body clips)
                Some(frame) => {
                    let grab = layout::GRAB;
                    let inner = body
                        // what's behind a window doesn't hear presses on it
                        .occlude()
                        .absolute()
                        .left(px(grab))
                        .top(px(grab))
                        .w(px(frame.w))
                        .h(px(frame.h))
                        .border_1()
                        .border_color(match focused && multi {
                            true => ink.ink,
                            false => ink.strong,
                        })
                        .when(focused, |pane| pane.shadow_lg())
                        .when(!focused, |pane| pane.shadow_sm())
                        .child(title)
                        .children(asking)
                        .child(div().flex_1().min_h_0().w_full().child(view));
                    div()
                        .absolute()
                        .left(px(frame.x - grab))
                        .top(px(frame.y - grab))
                        .w(px(frame.w + 2. * grab))
                        .h(px(frame.h + 2. * grab))
                        .child(inner)
                        .children(self.grips(index, cx))
                        .into_any_element()
                }
            };
            stage = stage.child(seated);
        }
        stage.into_any_element()
    }

    /// The NotifPermission board: a view posted before the person said
    /// anything about its notices, so its window asks, at the top. Until
    /// they answer its notices wait in Notifications, silently.
    fn permission_bar(&self, module: &'static str, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use super::ink::*;
        use crate::runtime::notify::{self, Permission};
        use gpui_kit::*;
        let ink = Ink::of(self.model.read(cx).state.dark());
        let name = super::panes::label(module);
        let burst = notify::Settings::load().burst;
        let button = |id: &'static str, text: &'static str, filled: bool, message: Message| {
            let model = self.model.clone();
            let message = std::cell::RefCell::new(Some(message));
            crate::a11y::keyboard(
                sans(500, 14.)
                    .id(SharedString::from(format!("notify-ask/{module}/{id}")))
                    .control(Role::Button, text)
                    .h(px(30.))
                    .px(px(12.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .cursor_pointer()
                    .border(px(1.5))
                    .border_color(ink.ink)
                    .when(filled, |button| button.bg(ink.ink).text_color(ink.bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        if let Some(message) = message.borrow_mut().take() {
                            model.update(cx, |model, cx| model.dispatch(message, cx));
                        }
                    })
                    .child(text),
            )
        };
        div()
            .id(SharedString::from(format!("notify-ask/{module}")))
            .role(Role::Group)
            .aria_label(SharedString::from(format!(
                "{name} wants to show desktop notifications"
            )))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(10.))
            .bg(ink.surface)
            .border_b_1()
            .border_color(ink.line)
            .child(
                gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Bell)
                    .size(px(14.))
                    .text_color(ink.ink),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        sans(500, 14.)
                            .text_color(ink.ink)
                            .child(format!("{name} wants to show desktop notifications")),
                    )
                    .child(note(
                        format!(
                            "At most {burst} banners a minute; the rest wait in Notifications."
                        ),
                        ink.muted,
                    )),
            )
            .child(button(
                "allow",
                "Allow",
                true,
                Message::NotifyPermission(module, Permission::Allow),
            ))
            .child(button(
                "not-now",
                "Not now",
                false,
                Message::NotifyNotNow(module),
            ))
            .into_any_element()
    }

    /// Takes hold of window `index` at `at`: `sides` (left, top, right,
    /// bottom) follow the pointer; none, and the whole window does.
    fn hold(&mut self, index: usize, sides: [bool; 4], at: gpui_kit::Point<gpui_kit::Pixels>) {
        if let Some(start) = self.layout.panes.get(index).and_then(|pane| pane.frame) {
            self.drag = Some(Drag {
                index,
                sides,
                from: (at.x.into(), at.y.into()),
                start,
            });
        }
    }

    /// The edges and corners a window is sized by: each reaches `GRAB`
    /// out past the border and `IN` over it, a corner further in.
    fn grips(
        &self,
        index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<gpui_kit::Stateful<gpui_kit::Div>> {
        use gpui_kit::*;
        // measured from the outside of the grips' frame, `GRAB` past the border
        const IN: f32 = 4.;
        const EDGE: f32 = layout::GRAB + IN;
        const CORNER: f32 = layout::GRAB + 10.;
        let grips: [(&str, [bool; 4], CursorStyle); 8] = [
            (
                "left",
                [true, false, false, false],
                CursorStyle::ResizeLeftRight,
            ),
            (
                "right",
                [false, false, true, false],
                CursorStyle::ResizeLeftRight,
            ),
            (
                "top",
                [false, true, false, false],
                CursorStyle::ResizeUpDown,
            ),
            (
                "bottom",
                [false, false, false, true],
                CursorStyle::ResizeUpDown,
            ),
            (
                "top-left",
                [true, true, false, false],
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "bottom-right",
                [false, false, true, true],
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "top-right",
                [false, true, true, false],
                CursorStyle::ResizeUpRightDownLeft,
            ),
            (
                "bottom-left",
                [true, false, false, true],
                CursorStyle::ResizeUpRightDownLeft,
            ),
        ];
        grips
            .into_iter()
            .map(|(name, sides, cursor)| {
                let [left, top, right, bottom] = sides;
                let corner = (left || right) && (top || bottom);
                let grip = div()
                    .id(SharedString::from(format!("pane/{index}/grip/{name}")))
                    .absolute()
                    .cursor(cursor)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.hold(index, sides, event.position);
                        }),
                    );
                let grip = match corner {
                    true => grip.size(px(CORNER)),
                    false if left || right => grip.w(px(EDGE)).top(px(CORNER)).bottom(px(CORNER)),
                    false => grip.h(px(EDGE)).left(px(CORNER)).right(px(CORNER)),
                };
                let grip = if left {
                    grip.left_0()
                } else if right {
                    grip.right_0()
                } else {
                    grip
                };
                if top {
                    grip.top_0()
                } else if bottom {
                    grip.bottom_0()
                } else {
                    grip
                }
            })
            .collect()
    }
}

/// What is done to this window's panes: the desk's own business, not the
/// model's.
#[derive(Clone, Copy, Debug)]
pub(super) enum PaneMessage {
    /// `module` in the focused pane (shift-click on the bar).
    Select(&'static str),
    /// Another pane showing `module`.
    Split(&'static str),
    Close(usize),
    Focus(usize),
    /// The pane into a window of its own.
    PopOut(usize),
    /// This pop-out's pane back onto the console's desk.
    PopIn,
}

/// A button on a window's title bar.
#[derive(Clone, Copy)]
enum PaneAction {
    Split,
    PopOut,
    PopIn,
    Close,
}

impl PaneAction {
    /// Its element id, its accessible name, its glyph.
    fn parts(self) -> (&'static str, &'static str, gpui_kit::assets::IconName) {
        use gpui_kit::assets::IconName;
        match self {
            Self::Split => ("split", "Open another window", IconName::Plus),
            Self::PopOut => (
                "popout",
                "Open in new window",
                IconName::SquareArrowOutUpRight,
            ),
            Self::PopIn => ("popin", "Move to main window", IconName::ArrowDownLeft),
            Self::Close => ("close", "Close pane", IconName::X),
        }
    }
}

/// The empty window's spacing for `rows` rows in a body `body` tall: row
/// and outer padding, the design's 10 and 28 while every row fits and
/// tighter as the window gets short; and when even the tightest leaves some
/// rows out, the height of the whole rows that fit, so the list scrolls by
/// whole rows and none is cut above the footer.
pub(super) fn empty_spacing(rows: usize, body: f32) -> EmptySpacing {
    // the label over the rows, and a footer wrapped to two lines
    const FIXED: f32 = 28. + 12. + 2. * 18.;
    const LINE: f32 = 24.;
    let room = |outer: f32| body - 2. * outer - FIXED;
    let fits = |(row, outer): (f32, f32)| rows as f32 * (LINE + 2. * row) <= room(outer);
    let spacings = [(10., 28.), (6., 16.), (3., 10.)];
    if let Some((row, outer)) = spacings.into_iter().find(|spacing| fits(*spacing)) {
        return EmptySpacing {
            row,
            outer,
            shown: None,
        };
    }
    let (row, outer) = spacings[2];
    let shown = (room(outer) / (LINE + 2. * row)).floor().max(1.);
    EmptySpacing {
        row,
        outer,
        shown: Some(shown * (LINE + 2. * row)),
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct EmptySpacing {
    pub(super) row: f32,
    pub(super) outer: f32,
    /// the rows' box when not all of them fit: whole rows only
    pub(super) shown: Option<f32>,
}

/// A title bar's height.
const TITLE: f32 = 32.;

/// A window held by the pointer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Drag {
    index: usize,
    sides: [bool; 4],
    from: (f32, f32),
    start: layout::Frame,
}

impl Drag {
    /// The frame with the pointer at `to`. A side held past the smallest
    /// window stops; the side across from it stays put.
    fn frame(&self, to: (f32, f32)) -> layout::Frame {
        let (dx, dy) = (to.0 - self.from.0, to.1 - self.from.1);
        let start = self.start;
        let [left, top, right, bottom] = self.sides;
        let mut frame = start;
        if self.sides == [false; 4] {
            frame.x += dx;
            frame.y += dy;
        }
        if right {
            frame.w = (start.w + dx).max(layout::MIN_WIDTH);
        }
        if bottom {
            frame.h = (start.h + dy).max(layout::MIN_HEIGHT);
        }
        if left {
            frame.w = (start.w - dx).max(layout::MIN_WIDTH);
            frame.x = start.x + start.w - frame.w;
        }
        if top {
            frame.h = (start.h - dy).max(layout::MIN_HEIGHT);
            frame.y = start.y + start.h - frame.h;
        }
        frame
    }
}

/// A press anywhere on a window brings it to the front, unless something
/// is open over the desk. Window-wide and
/// before anything under the pointer sees it: a program's view may block
/// the pointer from the window it sits in.
fn raise(
    this: gpui_kit::Entity<DesktopWindow>,
    desk: gpui_kit::Bounds<gpui_kit::Pixels>,
    window: &mut Window,
) {
    use gpui_kit::*;
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture {
            return;
        }
        let at = (
            f32::from(event.position.x - desk.origin.x),
            f32::from(event.position.y - desk.origin.y),
        );
        this.update(cx, |this, cx| {
            // a press on something open over the desk is not on a window
            let covered = this.kind == crate::shell::WindowKind::Console
                && this.model.read(cx).state.overlay.is_some();
            let Some(index) = this.layout.under(at).filter(|_| !covered) else {
                return;
            };
            let on_top = this.layout.stacking().last() == Some(&index);
            if index != this.layout.focused || !on_top {
                this.pane_message(PaneMessage::Focus(index), window, cx);
            }
        });
    });
}

/// The pointer, window-wide, while a window is held: a move carries it, a
/// release lets it go.
fn follow(this: gpui_kit::Entity<DesktopWindow>, window: &mut Window) {
    use gpui_kit::*;
    let held = this.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        held.update(cx, |this, cx| {
            let Some(drag) = this.drag else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                this.drag = None;
            } else {
                let desk = this.desk(window);
                let to = (event.position.x.into(), event.position.y.into());
                this.layout.set_frame(drag.index, drag.frame(to), desk);
            }
            cx.notify();
        });
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            this.update(cx, |this, cx| {
                if this.drag.take().is_some() {
                    cx.notify();
                }
            });
        }
    });
}

#[cfg(test)]
mod drag_tests {
    use super::{Drag, layout::*};

    #[test]
    fn a_title_bar_moves_and_an_edge_sizes_from_its_own_side() {
        let start = Frame {
            x: 100.,
            y: 100.,
            w: 500.,
            h: 400.,
        };
        let drag = |sides| Drag {
            index: 0,
            sides,
            from: (0., 0.),
            start,
        };
        let moved = drag([false; 4]).frame((30., -20.));
        assert_eq!(
            moved,
            Frame {
                x: 130.,
                y: 80.,
                ..start
            }
        );
        let corner = drag([false, false, true, true]).frame((40., 50.));
        assert_eq!(
            corner,
            Frame {
                w: 540.,
                h: 450.,
                ..start
            }
        );
        // the left edge past the smallest window: the right edge stays put
        let left = drag([true, false, false, false]).frame((1000., 0.));
        assert_eq!((left.w, left.x + left.w), (MIN_WIDTH, 600.));
        let top = drag([false, true, false, false]).frame((0., -60.));
        assert_eq!((top.y, top.h), (40., 460.));
    }
}

#[cfg(test)]
mod empty_panes_message_tests {
    use super::empty_panes_message;
    use crate::runtime::RailRow;

    fn row(empty: bool) -> RailRow {
        RailRow {
            module: "chat",
            label: "Chat".into(),
            note: None,
            empty,
        }
    }

    #[test]
    fn names_the_network_only_when_the_rail_itself_is_empty() {
        assert_eq!(
            empty_panes_message(&[]),
            "This network runs no program with a view."
        );
        assert_eq!(
            empty_panes_message(&[row(true)]),
            "This network runs no program with a view.",
            "a rail of empty-slot rows still has nothing to open"
        );
        assert!(empty_panes_message(&[row(true), row(false)]).starts_with("Press "));
    }
}
