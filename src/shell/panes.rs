//! View entities belong to panes, including when a pane changes OS windows.
use super::*;

pub(super) struct MountedPane {
    pub(super) module: &'static str,
    pub(super) view: Entity<crate::runtime::NativeModuleView>,
    pub(super) route: Option<gpui_kit::Subscription>,
    context: std::rc::Rc<std::cell::RefCell<String>>,
}

pub(super) fn label(module: &str) -> String {
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
fn empty_panes_message(rail: &[crate::runtime::RailRow]) -> &'static str {
    if rail.iter().any(|row| !row.empty) {
        match cfg!(target_os = "macos") {
            true => "Press ⌘K to open something.",
            false => "Press Ctrl K to open something.",
        }
    } else {
        "This network runs no program with a view."
    }
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
        for pane in &self.layout.panes {
            self.mounted.entry(pane.instance).or_insert_with(|| {
                let module = pane.module;
                let view = cx.new(|_| crate::runtime::NativeModuleView::new(module));
                MountedPane {
                    module,
                    view,
                    route: None,
                    context: Default::default(),
                }
            });
            let mounted = self.mounted.get_mut(&pane.instance).expect("mounted pane");
            if mounted.route.is_none() {
                let model = self.model.clone();
                let context = mounted.context.clone();
                let module = pane.module;
                mounted.route = Some(cx.subscribe(&mounted.view, move |_, _, event, cx| {
                    if event.kind == "context" {
                        *context.borrow_mut() = crate::runtime::event_text(event, "text");
                        cx.notify();
                    }
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
        message: Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.initialized = true;
        match message {
            Message::SelectView(module) => {
                self.layout.select(module);
                self.model.update(cx, |model, cx| {
                    model.dispatch(Message::SelectView(module), cx)
                });
            }
            Message::SplitView(module) => {
                self.layout.split(module);
            }
            Message::ClosePane(index) => {
                self.layout.close(index);
                if matches!(self.kind, crate::shell::WindowKind::View { .. }) {
                    window.remove_window();
                }
            }
            Message::FocusPane(index) => {
                self.layout.focus(index);
            }
            Message::PopOut(index) => {
                if let Some(pane) = self.layout.popout(index)
                    && let Some(mut mounted) = self.mounted.remove(&pane.instance)
                {
                    mounted.route = None;
                    let kind = crate::shell::WindowKind::View {
                        module: pane.module,
                    };
                    let source = cx.weak_entity();
                    let at = super::windows::cascade(
                        window.bounds(),
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
            Message::PopIn(key) if key == self.key => {
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
                    && let Some(pane) = self.layout.popout(0)
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
                    window.remove_window();
                }
            }
            _ => return,
        }
        self.sync_panes(cx);
        for (index, pane) in self.layout.panes.iter().enumerate() {
            self.mounted[&pane.instance].view.update(cx, |view, cx| {
                view.set_focused(index == self.layout.focused, cx)
            });
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn pane_button(
        &self,
        index: usize,
        action: &'static str,
        glyph: gpui_kit::assets::IconName,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::*;
        let ink = super::ink::Ink::of(self.model.read(cx).state.dark());
        let hover = ink.surface;
        // The action drives the element id and the dispatch below (stable
        // for the AX door and tests); the AX name is a phrase a screen
        // reader can announce on its own, not the bare verb.
        let name = match action {
            "split" => "Split pane",
            "popout" => "Open in new window",
            "popin" => "Move to main window",
            _ => "Close pane",
        };
        crate::a11y::disabled(
            crate::a11y::keyboard(
                div()
                    .id(SharedString::from(format!("pane/{index}/{action}")))
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if !enabled {
                            return;
                        }
                        let message = match action {
                            "split" => Message::SplitView(this.layout.panes[index].module),
                            "close" => Message::ClosePane(index),
                            "popout" => Message::PopOut(index),
                            _ => Message::PopIn(this.key),
                        };
                        this.pane_message(message, window, cx);
                    }))
                    .child(gpui_kit::component::Icon::new(glyph).size(px(18.))),
            ),
            !enabled,
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
        let measured = self.measured_widths.clone();
        let mut stage = div()
            .on_children_prepainted(move |bounds, _, _| {
                *measured.borrow_mut() = bounds
                    .into_iter()
                    .step_by(2)
                    .map(|bounds| f32::from(bounds.size.width))
                    .collect();
            })
            .id("panes")
            .size_full()
            .flex()
            .p(px(12.))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    this.resize = None;
                    return;
                }
                if let Some((index, start, left, right)) = this.resize {
                    if left + right < 640. {
                        return;
                    }
                    let delta =
                        (f32::from(event.position.x) - start).clamp(320. - left, right - 320.);
                    this.layout.resize_pair(index, left + delta, right - delta);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resize = None),
            );
        // No title: the program's view is the window (the Pane board: its
        // controls `28px` at `top: 8px; right: 8px`). With two or more,
        // the focused one is drawn in ink and shows its controls; the others
        // show theirs only under the pointer.
        let multi = self.layout.panes.len() > 1;
        for (index, pane) in self.layout.panes.iter().enumerate() {
            let focused = index == self.layout.focused;
            let view = self.mounted[&pane.instance].view.clone();
            view.update(cx, |view, cx| {
                view.set_focused(focused, cx);
                view.set_props(props.clone(), cx);
            });
            use gpui_kit::assets::IconName;
            let group = SharedString::from(format!("pane-{index}"));
            let controls = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .when(multi && !focused, |controls| {
                    controls
                        .opacity(0.)
                        .group_hover(group.clone(), |style| style.opacity(1.))
                })
                .when(self.kind == crate::shell::WindowKind::Console, |strip| {
                    strip
                        .child(self.pane_button(
                            index,
                            "split",
                            IconName::Columns2,
                            self.layout.panes.len() < layout::MAX_PANES,
                            cx,
                        ))
                        .child(self.pane_button(
                            index,
                            "popout",
                            IconName::SquareArrowOutUpRight,
                            true,
                            cx,
                        ))
                })
                .when(self.kind != crate::shell::WindowKind::Console, |strip| {
                    strip.child(self.pane_button(index, "popin", IconName::ArrowDownLeft, true, cx))
                })
                .child(self.pane_button(index, "close", IconName::X, true, cx));
            let strip = div()
                .id(SharedString::from(format!("pane/{index}/strip")))
                .h(px(44.))
                .flex_shrink_0()
                .pl(px(16.))
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    super::ink::mono(400, 12.)
                        .text_color(ink.muted)
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(self.mounted[&pane.instance].context.borrow().clone()),
                )
                .child(controls);
            if index > 0 {
                stage = stage.child(
                    div()
                        .id(SharedString::from(format!("pane/{index}/divider")))
                        .w(px(12.))
                        .h_full()
                        .flex_shrink_0()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                let widths = this.measured_widths.borrow();
                                for (pane, width) in this.layout.panes.iter_mut().zip(widths.iter())
                                {
                                    pane.width = *width;
                                }
                                let left = this.layout.panes[index - 1].width;
                                let right = this.layout.panes[index].width;
                                this.resize =
                                    Some((index - 1, event.position.x.into(), left, right));
                                cx.stop_propagation();
                            }),
                        ),
                );
            }
            stage = stage.child(
                div()
                    .id(SharedString::from(format!("pane/{index}")))
                    .flex()
                    .flex_col()
                    .flex_basis(px(0.))
                    .flex_grow(pane.width)
                    .min_w_0()
                    .h_full()
                    .group(group)
                    .border(px(1.))
                    .border_color(match focused && multi {
                        true => ink.ink,
                        false => ink.line,
                    })
                    .bg(ink.bg)
                    .overflow_hidden()
                    .capture_any_mouse_down(cx.listener(move |this, _, window, cx| {
                        if this.layout.focused != index {
                            this.pane_message(Message::FocusPane(index), window, cx);
                        }
                    }))
                    .child(strip)
                    .child(div().flex_1().min_h_0().w_full().child(view)),
            );
        }
        stage.into_any_element()
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
