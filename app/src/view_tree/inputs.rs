use super::*;

pub(super) struct RangeControl {
    pub(super) state: Entity<SliderState>,
    pub(super) bounds: [f32; 3],
    pub(super) value: f32,
    pub(super) on_change: u32,
    pub(super) on_release: Option<u32>,
    pub(super) _subscription: Subscription,
}

pub(super) struct EditorMount {
    pub(super) view: EditorView,
    pub(super) _subscription: Subscription,
}

/// An editor's `()` is "the store has events": forward them as this tree's.
pub(super) fn drain_editor(store: &crate::editor::wire::EditorStore, cx: &mut Context<ViewTree>) {
    for event in store.drain() {
        cx.emit(event);
    }
}

/// Native editor primitives selected by the guest's explicit projection.
pub(super) enum EditorView {
    Text(Entity<crate::editor::wire::TextEditor>),
}

impl EditorView {
    pub(super) fn sync(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.sync(window, cx)),
        }
    }

    /// Whether this editor takes the box it is given or takes the room its
    /// words need. The field's own element decides its height, so the node's
    /// answer has to reach it — a box told to shrink around an element that
    /// still asks for all of its parent's height shrinks around nothing.
    pub(super) fn fills(&self, fills: bool, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.set_fills(fills, cx)),
        }
    }

    /// The node's mapping, onto the field the editor draws: the node this
    /// mount announces is a wrapper, not the text. A rich editor's blocks are
    /// its own nodes, named by their kind; the label names its page.
    pub(super) fn announce(&self, accessible: Accessible, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.set_accessible(accessible, cx)),
        }
    }

    pub(super) fn widget_command(
        &self,
        command: &wire::WidgetCommand,
        window: &mut Window,
        cx: &mut App,
    ) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| {
                editor.widget_command(command, window, cx);
            }),
        }
    }

    /// The guest echoes which editor held focus when the frame was built, so
    /// the field it named takes the caret back.
    pub(super) fn restore_focus(
        &self,
        path: &[wire::ElementIdWire],
        window: &mut Window,
        cx: &mut App,
    ) {
        let Self::Text(view) = self;
        let focus = wire::WidgetCommand::Focus {
            target: path.to_vec(),
        };
        view.update(cx, |editor, cx| {
            editor.widget_command(&focus, window, cx);
        });
    }

    pub(super) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        match self {
            Self::Text(view) => view.read(cx).is_focused(window, cx),
        }
    }

    pub(super) fn element(&self) -> AnyElement {
        match self {
            Self::Text(view) => view.clone().into_any_element(),
        }
    }
}

pub(super) struct Field {
    pub(super) state: Entity<InputState>,
    pub(super) on_input: u32,
    pub(super) on_submit: Option<u32>,
    pub(super) value: String,
    pub(super) guest_value: String,
    pub(super) placeholder: String,
    pub(super) secure: bool,
    pub(super) ime: Option<crate::module_view::input::ImeState>,
    pub(super) _observer: Subscription,
    pub(super) _subscription: Subscription,
}

impl ViewTree {
    pub(super) fn input(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Input {
            id,
            value,
            placeholder,
            secure,
            on_input,
            on_submit,
            options,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        let identity = self.authored_path.clone();
        debug_assert_eq!(identity.last(), Some(id));
        if !self.fields.contains_key(&identity) {
            let presentation = self
                .presentation
                .inputs
                .remove(&identity)
                .filter(|saved| saved.value == *value && saved.secure == *secure);
            let state = cx.new(|cx| {
                let mut state = InputState::new(window, cx)
                    .placeholder(placeholder.clone())
                    .masked(*secure);
                state.set_value(value.clone(), window, cx);
                if let Some(saved) = presentation {
                    state.set_selected_range(saved.selection, cx);
                    if saved.focused {
                        state.focus(window, cx);
                    }
                }
                state
            });
            let input_identity = identity.clone();
            let observed_identity = identity.clone();
            let observer = cx.observe_in(&state, window, move |this, input, window, cx| {
                let Some(field) = this.fields.get_mut(&observed_identity) else {
                    return;
                };
                let (text, marked, cursor, selection) = input.update(cx, |input, cx| {
                    let marked = input.marked_text_range(window, cx);
                    (
                        input.value().to_string(),
                        marked,
                        input.cursor(),
                        input.selected_range(),
                    )
                });
                for event in crate::module_view::input::ime_events(
                    &mut field.ime,
                    &text,
                    marked,
                    cursor,
                    selection,
                ) {
                    cx.emit(event);
                }
            });
            let subscription = cx.subscribe_in(&state, window, move |this, input, event, _, cx| {
                let Some(field) = this.fields.get_mut(&input_identity) else {
                    return;
                };
                match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        if text == field.value {
                            return;
                        }
                        field.value = text.clone();
                        cx.emit(wire::Event::Input {
                            handler: field.on_input,
                            text,
                        });
                    }
                    InputEvent::PressEnter { .. } => {
                        if let Some(message) = field.on_submit {
                            cx.emit(wire::Event::Message(message));
                        }
                    }
                    InputEvent::Focus | InputEvent::Blur => {}
                }
            });
            self.fields.insert(
                identity.clone(),
                Field {
                    state,
                    on_input: *on_input,
                    on_submit: *on_submit,
                    value: value.clone(),
                    guest_value: value.clone(),
                    placeholder: placeholder.clone(),
                    secure: *secure,
                    ime: None,
                    _observer: observer,
                    _subscription: subscription,
                },
            );
        }
        let field = self.fields.get_mut(&identity).expect("field inserted");
        field.on_input = *on_input;
        field.on_submit = *on_submit;
        if field.guest_value != *value {
            field.guest_value = value.clone();
            if field.value != *value {
                field.value = value.clone();
                field
                    .state
                    .update(cx, |state, cx| state.set_value(value.clone(), window, cx));
            }
        }
        if field.placeholder != *placeholder {
            field.placeholder = placeholder.clone();
            field.state.update(cx, |state, cx| {
                state.set_placeholder(placeholder.clone(), window, cx)
            });
        }
        if field.secure != *secure {
            field.secure = *secure;
            field
                .state
                .update(cx, |state, cx| state.set_masked(*secure, window, cx));
        }
        // The field's one node is `ui::text_field`, carrying the mapping: an
        // empty label is no name, left unset so it reads as missing; a secure
        // field is a password, which keeps its value out of the tree.
        let accessible = accessible(node);
        let native_id = id.to_gpui().expect("validated input ID must lower to GPUI");
        let field_id = ElementId::NamedChild(Arc::new(native_id.clone()), "field".into());
        let mut input = Input::new(&field.state)
            .id(native_id)
            .disabled(options.disabled)
            .refine_style(style);
        if accessible.role == Some(gpui_kit::Role::PasswordInput) {
            input = input.content_type(InputContentType::Password);
        }
        let field = crate::a11y::text_field(
            field_id,
            &field.state.read(cx).focus_handle(cx),
            {
                let state = field.state.clone();
                move |value, window, cx| {
                    state.update(cx, |state, cx| state.replace_all(value, window, cx))
                }
            },
            input.role(gpui_kit::component::RoleOverride::Presentational),
        );
        announce(field, accessible).into_any_element()
    }

    pub(super) fn button(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Button {
            id,
            content,
            label,
            checked,
            on_press,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut button = Button::new(id.to_gpui().expect("sanitized button identity"))
            .refine_style(style)
            .disabled(on_press.is_none())
            .selected(checked.unwrap_or(false));
        button = match content {
            wire::ButtonContent::Label(text) => button.label(text.clone()),
            wire::ButtonContent::Child(child) => {
                button.h_auto().child(self.node(child, window, cx))
            }
        };
        if let Some(label) = label {
            button = button.accessibility_label(label.clone());
        }
        if let Some(message) = on_press {
            let message = *message;
            button = button.on_click(cx.listener(move |this, _, _, cx| {
                this.user_activation.set(Some(message));
                cx.emit(wire::Event::Message(message));
                cx.stop_propagation();
            }));
        }
        // the kit button resolves its own role at render, over the one
        // `announce` sets: a Tab is told so here or it is drawn a Button
        let accessible = accessible(node);
        if let Some(role) = accessible.role {
            button = button.role(role);
        }
        // GPUI's generic Click action falls back to a centre-point pointer
        // click. Register the semantic action on the button itself so an
        // accessibility press does not depend on overlapping hit-test layers.
        let button = if let Some(message) = *on_press {
            let view = cx.entity().downgrade();
            crate::a11y::aria(button, |node| {
                node.on_a11y_action(gpui_kit::accesskit::Action::Click, move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.user_activation.set(Some(message));
                        cx.emit(wire::Event::Message(message));
                    });
                })
            })
        } else {
            button
        };
        announce(button, accessible).into_any_element()
    }

    pub(super) fn toggle(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Toggle {
            id,
            kind,
            label,
            checked,
            on_toggle,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        if *kind == wire::ToggleKind::Switch {
            let mut toggle = gpui_kit::component::switch::Switch::new(
                id.to_gpui().expect("sanitized toggle identity"),
            )
                .refine_style(style)
                .label(label.clone())
                .checked(*checked)
                .disabled(on_toggle.is_none());
            if let Some(handler) = on_toggle {
                let handler = *handler;
                toggle = toggle.on_click(cx.listener(move |_, on, _, cx| {
                    cx.emit(wire::Event::Toggle { handler, on: *on })
                }));
            }
            return toggle.into_any_element();
        }
        let mut checkbox = Checkbox::new(id.to_gpui().expect("sanitized toggle identity"))
            .refine_style(style)
            .label(label.clone())
            .checked(*checked)
            .disabled(on_toggle.is_none());
        if let Some(handler) = on_toggle {
            let handler = *handler;
            checkbox =
                checkbox.on_click(cx.listener(move |_, on, _, cx| {
                    cx.emit(wire::Event::Toggle { handler, on: *on })
                }));
        }
        announce(checkbox, accessible(node)).into_any_element()
    }

    pub(super) fn progress(&mut self, node: &wire::Node, cx: &mut Context<Self>) -> AnyElement {
        let wire::Node::Progress {
            id,
            value,
            min,
            max,
            axis,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        let span = max - min;
        let valid = span.is_finite() && span > 0.0;
        let fraction = match valid {
            true => ((value - min) / span).clamp(0.0, 1.0),
            false => 0.0,
        };
        let fill = div().bg(gpui_kit::component::Theme::global(cx)
            .color_tokens()
            .primary);
        let track = match axis {
            wire::Axis::Row => div().child(fill.w(relative(fraction)).h_full()),
            wire::Axis::Column => div()
                .flex()
                .flex_col()
                .justify_end()
                .child(fill.h(relative(fraction)).w_full()),
        };
        announce(
            track
                .id(id.to_gpui().expect("sanitized progress identity"))
                .refine_style(style),
            accessible(node),
        )
        .into_any_element()
    }

    pub(super) fn editor(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Editor {
            id,
            style,
            document,
            ..
        } = node
        else {
            unreachable!()
        };
        let Some(store) = self.editor_store.clone() else {
            return div().child("Editor host is unavailable").into_any_element();
        };
        let path = self.authored_path.clone();
        debug_assert_eq!(path.last(), Some(id));
        if !self.editors.contains_key(&path) {
            let events = store.clone();
            let (view, subscription) = {
                let view = cx.new(|cx| {
                    crate::editor::wire::TextEditor::new(path.clone(), store, window, cx)
                });
                let subscription =
                    cx.subscribe(&view, move |_, _, _: &(), cx| drain_editor(&events, cx));
                (EditorView::Text(view), subscription)
            };
            self.editors.insert(
                path.clone(),
                EditorMount {
                    view,
                    _subscription: subscription,
                },
            );
        }
        let editor = self.editors.get(&path).expect("editor inserted");
        editor.view.fills(true, cx);
        editor.view.announce(accessible(node), cx);
        editor.view.sync(window, cx);
        if self.presentation.editors.remove(&path).as_ref() == Some(document) {
            editor.view.restore_focus(&path, window, cx);
        }
        let view = editor.view.element();
        let mut element = div()
            .relative()
            .id(id.to_gpui().expect("sanitized editor identity"));
        *element.style() = style.clone();
        element
            .child(view)
            .child(self.measure(&path, cx))
            .into_any_element()
    }

    pub(super) fn slider(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Slider {
            id,
            value,
            min,
            max,
            step,
            on_change,
            on_release,
            axis,
            style,
            ..
        } = node
        else {
            unreachable!()
        };
        let path = self.authored_path.clone();
        let bounds = [*min, *max, *step];
        let rebuild = self
            .ranges
            .get(&path)
            .is_none_or(|control| control.bounds != bounds);
        if rebuild {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(*min)
                    .max(*max)
                    .step(*step)
                    .default_value(*value)
            });
            let route = path.clone();
            let subscription = cx.subscribe_in(&state, window, move |this, _, event, _, cx| {
                let Some(control) = this.ranges.get_mut(&route) else {
                    return;
                };
                match event {
                    SliderEvent::Change(value) => {
                        control.value = value.start();
                        cx.emit(wire::Event::Slide {
                            handler: control.on_change,
                            value: control.value,
                        });
                    }
                    SliderEvent::Release(_) => {
                        if let Some(message) = control.on_release {
                            cx.emit(wire::Event::Message(message));
                        }
                    }
                }
            });
            self.ranges.insert(
                path.clone(),
                RangeControl {
                    state,
                    bounds,
                    value: *value,
                    on_change: *on_change,
                    on_release: *on_release,
                    _subscription: subscription,
                },
            );
        }
        let control = self.ranges.get_mut(&path).expect("range inserted");
        control.on_change = *on_change;
        control.on_release = *on_release;
        if control.value != *value {
            control.value = *value;
            control
                .state
                .update(cx, |state, cx| state.set_value(*value, window, cx));
        }
        let slider = match axis {
            wire::Axis::Row => Slider::new(&control.state).horizontal(),
            wire::Axis::Column => Slider::new(&control.state).vertical(),
        };
        div()
            .id(id.to_gpui().expect("sanitized slider identity"))
            .refine_style(style)
            .child(slider)
            .into_any_element()
    }
}
