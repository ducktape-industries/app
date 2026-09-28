//! The desk's panes, drawn: `pane_stage` gives each its title bar and
//! buttons, seats it on the desk, hands the keys to the one in front and
//! asks about a view's notices (the permission bar). Where panes sit, stack
//! and which has the keys is `ui::layout`'s: this file only draws it and
//! sends `PaneMessage`s. Their program views are `mount.rs`'s, the
//! pointer's hold on them `pane_drag.rs`'s, a desk with none `empty_desk.rs`'s.
use super::*;

pub(super) fn label(module: &str) -> String {
    if module == layout::EMPTY {
        return "Empty".to_owned();
    }
    if module == layout::HELP {
        return "Help".to_owned();
    }
    crate::runtime::rail()
        .into_iter()
        .find(|row| row.module == module)
        .map(|row| row.label)
        .unwrap_or_else(|| module.to_owned())
}

/// What an empty window lists: the rail's programs, as the menu bar shows them.
pub(super) fn openable() -> Vec<crate::runtime::RailRow> {
    crate::runtime::rail()
        .into_iter()
        .filter(|row| !row.empty)
        .collect()
}

impl DesktopWindow {
    /// Something done to this window's panes: the model moves them, and
    /// the keys go to the focused one (`pane_stage`).
    pub(super) fn pane_message(
        &mut self,
        message: PaneMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match message {
            // it opens where it sat on the desk
            PaneMessage::PopOut { index, at: None } => {
                let layout = self.layout(cx);
                let pane = layout.panes.get(index);
                PaneMessage::PopOut {
                    index,
                    at: Some(super::windows::unseated(
                        window.bounds(),
                        pane.and_then(|pane| pane.frame),
                        window.display(cx).map(|display| display.bounds()),
                        pane.map_or(super::windows::POPOUT_MIN, |pane| {
                            super::windows::popout_min(pane.module)
                        }),
                    )),
                }
            }
            message => message,
        };
        let key = self.key;
        self.model.update(cx, |model, cx| {
            model.dispatch(Message::Pane(key, message), cx)
        });
        self.panes_moved = true;
        cx.notify();
    }

    /// A press on a row of empty window `index`: that window takes the keys
    /// first, as a pointer's press on it does (`pane_drag::raise`), so the
    /// program opens there, not in the window that had them.
    pub(super) fn open_here(
        &mut self,
        index: usize,
        module: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Focus(index), window, cx);
        self.open_view(module, window, cx);
    }

    /// A menu bar click, or a pick in an empty window: see `Layout::open`.
    pub(super) fn open_view(
        &mut self,
        module: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_message(PaneMessage::Open(module), window, cx);
        self.clear_command(window, cx);
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
        let module = self
            .layout(cx)
            .panes
            .get(index)
            .map_or(layout::EMPTY, |pane| pane.module);
        let (id, name, glyph) = action.parts(module);
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
                        PaneAction::Split => match this.layout(cx).panes.get(index) {
                            Some(pane) => PaneMessage::Split(pane.module),
                            None => return,
                        },
                        PaneAction::Close => PaneMessage::Close(index),
                        PaneAction::PopOut => PaneMessage::PopOut { index, at: None },
                        PaneAction::PopIn => PaneMessage::PopIn,
                    };
                    this.pane_message(message, window, cx);
                }))
                .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
        )
        .aria_disabled(!enabled)
    }

    /// The desk windows sit on: the window below its bar.
    pub(super) fn desk(&self, window: &Window) -> (f32, f32) {
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
        let layout = self.layout(cx);
        let moved = self.keys_move(&layout, cx);
        if layout.panes.is_empty() {
            self.sync_hold(&layout, window, cx);
            return self.empty_desk(moved, &ink, window, cx);
        }
        let props = self.model.read(cx).state.view_props();
        let center = self.model.read(cx).state.center.clone();
        let this = cx.entity();
        let mut stage = div().id("panes").relative().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| pane_drag::raise(this, bounds, window),
            )
            .absolute()
            .size_full(),
        );
        if self.drag.is_some() {
            // window-wide, so a fast pointer can't slip off the window it holds
            let this = cx.entity();
            stage = stage.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| pane_drag::follow(this, window),
                )
                .absolute()
                .size_full(),
            );
        }
        self.pane_keys
            .retain(|instance, _| layout.panes.iter().any(|pane| pane.instance == *instance));
        // before the windows are seated, so a window coming to the front
        // has the keys after the hold gives them back, not before
        self.sync_hold(&layout, window, cx);
        for index in layout.stacking() {
            let pane = &layout.panes[index];
            let focused = index == layout.focused;
            let held = layout
                .held
                .is_some_and(|held| held.instance == pane.instance)
                && self.kind == crate::shell::WindowKind::Console;
            let own = self.pane_focus(pane.instance, focused && moved, window, cx);
            let view = self.pane_body(index, pane, focused, &own, &props, window, cx);
            // it holds the pane's keys when nothing in the view does; Tab
            // never lands on it, so it offers assistive technology no focus
            // either (as the window's root). On the desk it names the chord
            // that hands it the keys (`keys::FocusPane`).
            let view = crate::a11y::Patch::default()
                .keys_fallback()
                .on(div()
                    .id(SharedString::from(format!("pane/{index}/view")))
                    .role(gpui_kit::Role::Group)
                    .aria_label(label(pane.module))
                    .when(self.kind == crate::shell::WindowKind::Console, |view| {
                        view.aria_keyshortcuts(super::chord_label(&(index + 1).to_string()))
                    })
                    // a held window's keys are the hold's (`pane_hold.rs`)
                    .when(held, |view| {
                        view.key_context("hold")
                            .on_key_down(cx.listener(move |this, event, _, cx| {
                                this.held_key(index, event, cx)
                            }))
                            .child(
                                crate::a11y::live(
                                    div().id("hold-say").role(Role::Status),
                                    accesskit::Live::Polite,
                                    pane_hold::hold_words(pane.module),
                                )
                                .absolute()
                                .size(px(1.))
                                .overflow_hidden(),
                            )
                    })
                    .track_focus(&own))
                .flex_1()
                .min_h_0()
                .w_full()
                .child(view);
            // Every window has a title bar, so a view owns all of its
            // rectangle: nothing floats over its corners.
            let title = self.pane_title_bar(index, &layout, &ink, window, cx);
            let asking = (pane.is_view() && center.lock().asking(pane.module))
                .then(|| self.permission_bar(pane.module, cx));
            let contents = [Some(title), asking, Some(view.into_any_element())]
                .into_iter()
                .flatten()
                .collect();
            stage = stage.child(self.place_pane(index, &layout, contents, &ink, cx));
        }
        stage.into_any_element()
    }

    /// Whether the keys go to the window in front on this draw: another
    /// came there, whoever brought it (a key, the bar, the model), or this
    /// window moved its panes. Not while something open over the desk
    /// holds them; once it closes, and then to the new front, not back to
    /// what had them when it opened.
    fn keys_move(&mut self, layout: &layout::Layout, cx: &gpui_kit::App) -> bool {
        let covered = self.kind == crate::shell::WindowKind::Console
            && self.model.read(cx).state.overlay.is_some();
        if covered {
            return false;
        }
        let front = layout.panes.get(layout.focused).map(|pane| pane.instance);
        let turned = std::mem::replace(&mut self.front, front) != front;
        if turned {
            self.refocus = None;
        }
        std::mem::take(&mut self.panes_moved) || turned
    }

    /// The focus handle of the pane with `instance`, remembering what had
    /// the keys inside it. `restore` (the pane just came to the front) puts
    /// the keys back: what had them in it, if it is still there; else its
    /// first control; else the window itself.
    fn pane_focus(
        &mut self,
        instance: u64,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::FocusHandle {
        let holding = self.holding.is_some();
        let (own, last) = self
            .pane_keys
            .entry(instance)
            .or_insert_with(|| (cx.focus_handle(), None));
        // the box of a held window has the keys, and nothing in it does
        if own.contains_focused(window, cx) && !holding {
            *last = window.focused(cx);
        }
        let (own, last) = (own.clone(), last.clone());
        if restore {
            let own = own.clone();
            window.defer(cx, move |window, cx| match last {
                Some(last) if own.contains(&last, window) => last.focus(window, cx),
                _ => {
                    own.focus(window, cx);
                    window.focus_next(cx);
                    if !own.contains_focused(window, cx) {
                        own.focus(window, cx);
                    }
                }
            });
        }
        own
    }

    /// What a pane shows: its program view, told whether it is in front and
    /// given the session's props, or else the app's own Help or the finder.
    #[allow(clippy::too_many_arguments, reason = "one pane, drawn")]
    fn pane_body(
        &mut self,
        index: usize,
        pane: &layout::Pane,
        focused: bool,
        own: &gpui_kit::FocusHandle,
        props: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let mounted = self
            .model
            .read(cx)
            .mounted
            .get(&pane.instance)
            .map(|mounted| mounted.view.clone());
        match mounted {
            Some(view) => {
                view.update(cx, |view, cx| {
                    view.set_focused(focused, cx);
                    view.set_props(props.to_vec(), cx);
                });
                view.into_any_element()
            }
            // Help draws no field: its box keeps the keys
            None if pane.module == layout::HELP => self.help_view(cx),
            None => {
                // the bare window box has the keys: the field takes them
                if focused && own.is_focused(window) && self.holding.is_none() {
                    self.focus_command(window, cx);
                }
                self.command_view(index, focused, window, cx)
            }
        }
    }

    /// A pane's title bar: the handle it is held by on the desk, its name,
    /// and its buttons at the right end.
    fn pane_title_bar(
        &self,
        index: usize,
        layout: &layout::Layout,
        ink: &super::ink::Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let pane = &layout.panes[index];
        let focused = index == layout.focused;
        let console = self.kind == crate::shell::WindowKind::Console;
        let on_desk = console && pane.frame.is_some();
        let controls = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(console, |strip| {
                strip.child(self.pane_button(
                    index,
                    PaneAction::Split,
                    layout.panes.len() < layout::MAX_PANES,
                    cx,
                ))
            })
            // an empty or Help window has no program view to carry out
            .when(console && pane.is_view(), |strip| {
                strip.child(self.pane_button(index, PaneAction::PopOut, true, cx))
            })
            .when(!console, |strip| {
                strip.child(self.pane_button(index, PaneAction::PopIn, true, cx))
            })
            .child(self.pane_button(index, PaneAction::Close, true, cx));
        // A window of its own on macOS draws no title bar of the
        // system's: this bar is its handle, and the traffic lights sit
        // over its left end. Elsewhere the system's bar names it.
        let lights = theme::traffic_lights(window).filter(|_| !on_desk);
        let handle = lights.is_some();
        div()
            .id(SharedString::from(format!("pane/{index}/strip")))
            .h(px(TITLE))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(lights.unwrap_or(12.)))
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
                        (true, 2) => this.pane_message(PaneMessage::Fill(index), window, cx),
                        (true, _) => this.hold(index, pane_drag::Sides::NONE, event.position, cx),
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
            .child(controls)
            .into_any_element()
    }

    /// A pane's box around `contents` (title bar, permission bar, body),
    /// seated: filling the window it is alone in, or at its frame on the
    /// desk with grips around it.
    fn place_pane(
        &self,
        index: usize,
        layout: &layout::Layout,
        contents: Vec<gpui_kit::AnyElement>,
        ink: &super::ink::Ink,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        use gpui_kit::*;
        let pane = &layout.panes[index];
        let focused = index == layout.focused;
        let multi = layout.panes.len() > 1;
        let on_desk = self.kind == crate::shell::WindowKind::Console && pane.frame.is_some();
        let held = on_desk
            && layout
                .held
                .is_some_and(|held| held.instance == pane.instance);
        let body = div()
            .id(SharedString::from(format!("pane/{index}")))
            .flex()
            .flex_col()
            .bg(ink.bg)
            .overflow_hidden()
            .role(gpui_kit::Role::Group);
        match pane.frame.filter(|_| on_desk) {
            None => body.size_full().children(contents).into_any_element(),
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
                    .children(contents)
                    // the keyboard holds it (`pane_hold.rs`): the focus ring
                    // on its border, over the title bar
                    .when(held, |pane| {
                        pane.child(
                            div()
                                .absolute()
                                .size_full()
                                .shadow(vec![crate::a11y::ring()]),
                        )
                    });
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
        }
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
                    .h(px(tall(30.)))
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
        // announced as it appears: nothing else says a view is waiting
        crate::a11y::live(
            div()
                .id(SharedString::from(format!("notify-ask/{module}")))
                .role(Role::Group),
            accesskit::Live::Polite,
            format!("{name} wants to show desktop notifications"),
        )
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
                    "burst",
                    format!("At most {burst} banners a minute; the rest wait in Notifications."),
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
    /// Its element id, its accessible name on the window of `module` (one
    /// window's Close is not another's: "Close Chat window"), its glyph.
    fn parts(self, module: &str) -> (&'static str, String, gpui_kit::assets::IconName) {
        use gpui_kit::assets::IconName;
        let program = label(module);
        let window = match module {
            layout::EMPTY => "empty window".to_owned(),
            _ => format!("{program} window"),
        };
        match self {
            Self::Split => ("split", format!("Open another {window}"), IconName::Plus),
            Self::PopOut => (
                "popout",
                format!("Open {program} in a new window"),
                IconName::SquareArrowOutUpRight,
            ),
            Self::PopIn => (
                "popin",
                format!("Move {program} to the main window"),
                IconName::ArrowDownLeft,
            ),
            Self::Close => ("close", format!("Close {window}"), IconName::X),
        }
    }
}

/// A title bar's height.
const TITLE: f32 = 32.;
