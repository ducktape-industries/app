use super::*;
use crate::render::native_id;

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
    pub(super) fn fills(&self, fills: bool, cap: Option<usize>, cx: &mut App) {
        match self {
            Self::Text(view) => view.update(cx, |editor, cx| editor.set_fills(fills, cap, cx)),
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
        let native_id = native_id(id);
        let field_id = ElementId::NamedChild(Arc::new(native_id.clone()), "field".into());
        // the kit fixes the line at 20px inside `8px` padding: a face over
        // ~16px is clipped top and bottom. The line follows the face and the
        // kit's height centres it; a view's own style still wins.
        let mut input = Input::new(&field.state)
            .id(native_id)
            .disabled(options.disabled)
            .line_height(gpui_kit::relative(1.4))
            .py_0()
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
        // A height the guest gave is a box to fill; none (a composer's
        // `min_h`..`max_h`) is a field as tall as its lines, which the
        // field's own auto-grow reports up to the parent that waits on it.
        let cap = field_rows(style);
        editor.view.fills(style.size.height.is_some(), cap, cx);
        editor.view.announce(accessible(node), cx);
        editor.view.sync(window, cx);
        if self.presentation.editors.remove(&path).as_ref() == Some(document) {
            editor.view.restore_focus(&path, window, cx);
        }
        let view = editor.view.element();
        let mut element = div().relative().id(native_id(id));
        *element.style() = style.clone();
        element
            .child(view)
            .child(self.measure(&path, cx))
            .into_any_element()
    }
}

/// How many lines a field that grows with its words shows before it scrolls:
/// what its node's `max_h` holds, less its vertical padding, at the field's
/// line height (gpui's default, the golden ratio of the text size).
fn field_rows(style: &gpui_kit::StyleRefinement) -> Option<usize> {
    use gpui_kit::{AbsoluteLength, DefiniteLength, Length};
    let pixels = |length: Option<DefiniteLength>| match length {
        Some(DefiniteLength::Absolute(AbsoluteLength::Pixels(pixels))) => f32::from(pixels),
        _ => 0.,
    };
    let Some(Length::Definite(max)) = style.max_size.height else {
        return None;
    };
    let pad = pixels(style.padding.top) + pixels(style.padding.bottom);
    let size = match style.text.font_size {
        Some(AbsoluteLength::Pixels(size)) => f32::from(size),
        _ => 14.,
    };
    let rows = (pixels(Some(max)) - pad) / (size * 1.618);
    Some((rows.floor() as usize).max(1))
}
