//! `Node::Input`: a one-line field the kit's `InputState` holds, with the
//! last values the guest gave it so a repeated frame leaves typing alone.

use super::*;
use crate::render::native_id;

/// A mounted `Node::Input`: the kit state and the last values the guest
/// gave, so a frame that repeats them leaves what the user typed alone.
pub(super) struct Field {
    pub(super) state: Entity<InputState>,
    pub(super) on_input: Option<u32>,
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
            let subscription =
                cx.subscribe_in(&state, window, move |this, input, event, window, cx| {
                    let Some(field) = this.fields.get_mut(&input_identity) else {
                        return;
                    };
                    match event {
                        InputEvent::Change => {
                            let mut text = input.read(cx).value().to_string();
                            let typed = text.len();
                            wire::truncate_string(&mut text);
                            if text.len() < typed {
                                // the guest's bound is the field's: it shows what it sent
                                input.update(cx, |state, cx| {
                                    state.set_value(text.clone(), window, cx)
                                });
                            }
                            if text == field.value {
                                return;
                            }
                            field.value = text.clone();
                            if let Some(handler) = field.on_input {
                                cx.emit(wire::Event::Input { handler, text });
                            }
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
        // The wrapper wears the view's id and is the box the view's parent
        // lays out (`placed`); the kit's input inside takes a host name
        // nested under it, where no view child can be.
        let accessible = accessible(node);
        let native_id = native_id(id);
        let focus = field.state.read(cx).focus_handle(cx);
        let (placed, drawn) = placed(style);
        // the kit fixes the line at 20px inside `8px` padding: a face over
        // ~16px is clipped top and bottom. The line follows the face and the
        // kit's height centres it; a view's own style still wins.
        // read-only as disabled is: the kit refuses what the user types.
        // Focused, the kit's box wears the ring on itself (`around_field`);
        // the kit's own focus look is off: a second ring painted round the
        // outside, and a border colour laid over any style given here
        let mut input = Input::new(&field.state)
            .id(host_id("input"))
            .disabled(options.disabled)
            .readonly(options.read_only)
            .focus_bordered(false)
            .line_height(gpui_kit::relative(1.4))
            .py_0()
            .refine_style(&drawn);
        if accessible.role == Some(gpui_kit::Role::PasswordInput) {
            input = input.content_type(InputContentType::Password);
        }
        let input =
            crate::a11y::around_field(input, focus.is_focused(window), crate::a11y::ink(cx));
        let field = crate::a11y::text_field(
            native_id,
            &focus,
            {
                let state = field.state.clone();
                // and what assistive technology sets, which the kit takes
                // as its own programmatic change
                move |value, window, cx| {
                    state.update(cx, |state, cx| {
                        if state.is_editable() {
                            state.replace_all(value, window, cx)
                        }
                    })
                }
            },
            input.role(gpui_kit::component::RoleOverride::Presentational),
        )
        .refine_style(&placed);
        announce(field, accessible).into_any_element()
    }
}

/// A view's `style` for its input, split between the wrapper, the box the
/// parent lays out, and the kit's box drawn inside it. Where the field sits
/// (its margin, its place, its share of a flex line) is the wrapper's; its
/// size is both's, so the kit's box fills the wrapper; the rest is the kit's
/// box. A wrapper left the full width would take a flex line's room from a
/// spacer and its siblings, and a press there would focus the field.
fn placed(
    style: &gpui_kit::StyleRefinement,
) -> (gpui_kit::StyleRefinement, gpui_kit::StyleRefinement) {
    let mut drawn = style.clone();
    let placed = gpui_kit::StyleRefinement {
        position: drawn.position.take(),
        inset: std::mem::take(&mut drawn.inset),
        margin: std::mem::take(&mut drawn.margin),
        align_self: drawn.align_self.take(),
        flex_grow: drawn.flex_grow.take(),
        flex_shrink: drawn.flex_shrink.take(),
        flex_basis: drawn.flex_basis.take(),
        grid_location: drawn.grid_location.take(),
        size: drawn.size.clone(),
        min_size: drawn.min_size.clone(),
        max_size: drawn.max_size.clone(),
        ..Default::default()
    };
    (placed, drawn)
}
