//! Accessibility helpers for the native screens and the tree presenter: role
//! with name (`Control`), keyboard reach (`keyboard`, `focus_shown`), states
//! set on the element's own node after a kit widget has built it (`aria`),
//! what gpui has no setter for (`Patch`: `modal`, `live`, the view's
//! phase-2 aria, and the class names the AX test door masks, `private`, or
//! leaves untruncated, `whole`), and one AT node for a kit text input
//! (`text_field`), or for it and the list it picks from (`combo_box`).

use gpui_kit::accesskit::{AriaCurrent, CustomAction, HasPopup, Invalid, Live};
use gpui_kit::{
    AccessibleAction, App, Div, ElementId, FocusHandle, InteractiveElement, Interactivity,
    IntoElement, MouseButton, ParentElement as _, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled as _, Window, div,
};

/// A clickable element as assistive technology meets it: in the tree as
/// `role`, called `name`.
pub trait Control: StatefulInteractiveElement {
    fn control(self, role: Role, name: impl Into<SharedString>) -> Self {
        self.role(role).aria_label(name)
    }
}

impl<E: StatefulInteractiveElement> Control for E {}

/// A control a keyboard reaches with Tab and presses with Enter or Space, as
/// a pointer presses it. A pointer's press does not move focus to it, so
/// pressing it leaves the caret where it was. Reached by Tab, it shows it.
pub fn keyboard<E: StatefulInteractiveElement>(element: E) -> E {
    focus_shown(element.focusable().tab_stop(true))
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

/// The focus ring: grey, 2px inside the edge, which reads on the light
/// theme and the dark one alike, over a filled button as over a bare word,
/// and moves nothing.
pub fn ring() -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow {
        color: gpui_kit::hsla(0., 0., 0.5, 1.),
        offset: gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
        blur_radius: gpui_kit::px(0.),
        spread_radius: gpui_kit::px(2.),
        inset: true,
    }
}

/// The mark a control reached by the keyboard wears: the [`ring`].
pub fn focus_shown<E: InteractiveElement>(element: E) -> E {
    element.focus_visible(|style| style.shadow(vec![ring()]))
}

/// The accessibility setters of any interactive element, kit widgets that
/// do not expose them included. A widget still draws its own role and name
/// over these when it renders; the states it leaves alone stay as set here.
pub fn aria<E: InteractiveElement>(mut element: E, set: impl FnOnce(Aria<'_>) -> Aria<'_>) -> E {
    set(Aria(element.interactivity()));
    element
}

/// The node properties gpui has no setter for, written onto the element's
/// own node from its one `a11y_synthetic_children` closure. An element has
/// room for one closure — a second replaces the first — so everything one
/// element needs goes into one `Patch`. gpui runs it only for an element
/// with an id and a role. The view's aria mapper (`guest_aria`) fills the
/// fields a view sets directly.
#[derive(Clone, Default, PartialEq)]
pub struct Patch {
    pub(crate) modal: bool,
    pub(crate) live: Option<Live>,
    pub(crate) busy: bool,
    pub(crate) required: bool,
    pub(crate) read_only: bool,
    pub(crate) invalid: Option<Invalid>,
    pub(crate) has_popup: Option<HasPopup>,
    pub(crate) current: Option<AriaCurrent>,
    /// `(id, description)`, each requested as `Action::CustomAction`.
    pub(crate) custom_actions: Vec<(i32, String)>,
    pub(crate) class_name: Option<&'static str>,
    pub(crate) fallback: bool,
}

impl Patch {
    /// A modal dialog boundary.
    pub fn modal(self) -> Self {
        Self {
            modal: true,
            ..self
        }
    }

    /// A live region: a change to its words is announced, `Polite` after
    /// what is being read, `Assertive` at once.
    pub fn live(self, live: Live) -> Self {
        Self {
            live: Some(live),
            ..self
        }
    }

    /// One of the door's classes, [`AX_PRIVATE`] or [`AX_WHOLE`].
    pub fn class_name(self, class_name: &'static str) -> Self {
        Self {
            class_name: Some(class_name),
            ..self
        }
    }

    /// A box that holds the keys only when nothing inside it does (a
    /// window's root): the app focuses it, Tab never lands on it, so it
    /// offers assistive technology no focus the keyboard cannot reach
    /// either. gpui offers one on every element that tracks a handle.
    pub fn keys_fallback(self) -> Self {
        Self {
            fallback: true,
            ..self
        }
    }

    /// Installs the one closure on `element`.
    pub fn on<E: InteractiveElement>(self, element: E) -> E {
        if self == Self::default() {
            return element;
        }
        aria(element, |node| {
            node.a11y_synthetic_children(move |tree| {
                let node = tree.parent_node();
                if self.modal {
                    node.set_modal();
                }
                if let Some(live) = self.live {
                    node.set_live(live);
                }
                if self.busy {
                    node.set_busy();
                }
                if self.required {
                    node.set_required();
                }
                if self.read_only {
                    node.set_read_only();
                }
                if let Some(invalid) = self.invalid {
                    node.set_invalid(invalid);
                }
                if let Some(popup) = self.has_popup {
                    node.set_has_popup(popup);
                }
                if let Some(current) = self.current {
                    node.set_aria_current(current);
                }
                if !self.custom_actions.is_empty() {
                    node.set_custom_actions(
                        self.custom_actions
                            .into_iter()
                            .map(|(id, description)| CustomAction {
                                id,
                                description: description.into(),
                            })
                            .collect::<Vec<_>>(),
                    );
                }
                if let Some(class_name) = self.class_name {
                    node.set_class_name(class_name);
                }
                if self.fallback {
                    node.remove_action(gpui_kit::accesskit::Action::Focus);
                }
            })
        })
    }
}

/// Marks the element's accessible node as a modal dialog boundary.
pub fn modal<E: InteractiveElement>(element: E) -> E {
    Patch::default().modal().on(element)
}

/// Words assistive technology announces as they appear and change: the
/// element's name and its value (AT-SPI and UIA speak the name, macOS the
/// value), in a live region. The caller draws the words as bare text, so
/// they are read once.
pub fn live<E: StatefulInteractiveElement>(
    element: E,
    live: Live,
    words: impl Into<SharedString>,
) -> E {
    let words = words.into();
    Patch::default()
        .live(live)
        .on(element.aria_label(words.clone()).aria_value(words))
}

/// An element's accessibility properties, reached through its interactivity.
pub struct Aria<'a>(&'a mut Interactivity);

impl InteractiveElement for Aria<'_> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.0
    }
}

impl StatefulInteractiveElement for Aria<'_> {}

/// A text field as assistive technology meets it: one node, `id`, that Tab
/// and the Focus action both land on. The kit's text inputs keep their tab
/// stop on an inner element with no role, so this node tracks the text's own
/// `focus` handle and hands SetValue to `set_value`. The caller draws `field`
/// with no node of its own and sets the role, the name and the states here.
/// Focused, it wears the [`ring`] however it got there: a caret alone is too
/// faint a mark for where typing goes.
pub fn text_field(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
    field: impl IntoElement,
) -> Stateful<Div> {
    typed(
        div()
            .id(id)
            .w_full()
            .track_focus(focus)
            .focus(|style| style.shadow(vec![ring()])),
        set_value,
    )
    .child(field)
}

/// An editable combo box as assistive technology meets it (a field and the
/// list it picks from): one node, `id`, around the field and its list, that
/// holds the field's `focus`, so the list's picked row can claim to be its
/// active descendant — gpui honours the claim only under the focused node.
/// The field inside is a [`text_field`] with no role: it keeps the ring and
/// Tab, and has no node of its own. The caller sets the name, the value and
/// whether the list shows (`aria_expanded`).
pub fn combo_box(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    typed(
        div().id(id).role(Role::EditableComboBox).track_focus(focus),
        set_value,
    )
}

/// `element` hands SetValue's text to `set_value`.
fn typed(
    element: Stateful<Div>,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    element.on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
        if let Some(gpui_kit::accesskit::ActionData::Value(value)) = data {
            set_value(value.to_string(), window, cx);
        }
    })
}

/// The class of a node whose value is private, and its name too unless it
/// is a text field (a field's name is its label): the app's test door masks
/// them before they leave the process. Assistive technology still reads
/// them — they are on the screen.
pub const AX_PRIVATE: &str = "ax_private";

/// Class name of a node whose name the AX tree gives whole, past its
/// length cap: a URL an agent must read back in full.
pub const AX_WHOLE: &str = "ax_whole";

/// Marks the element's accessible node [`AX_WHOLE`]: the passkey QR's URL.
pub fn whole<E: InteractiveElement>(element: E) -> E {
    Patch::default().class_name(AX_WHOLE).on(element)
}

/// Marks the element's accessible node [`AX_PRIVATE`]: the recovery phrase.
pub fn private<E: InteractiveElement>(element: E) -> E {
    Patch::default().class_name(AX_PRIVATE).on(element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Context, Render, TestAppContext, VisualTestContext, px, size};

    struct Patched(FocusHandle);

    impl Render for Patched {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Patch::default()
                .modal()
                .live(Live::Polite)
                .class_name(AX_PRIVATE)
                .keys_fallback()
                .on(div()
                    .id("patched")
                    .role(Role::Dialog)
                    .aria_label("Patched")
                    .track_focus(&self.0))
        }
    }

    /// Everything one element needs lands on its node: a live region set
    /// beside a modal does not replace it.
    #[gpui_kit::test]
    fn one_patch_writes_every_property_it_collects(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| Patched(cx.focus_handle()));
        let mut native = VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            let update = window.a11y_tree().unwrap();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Patched"))
                .unwrap();
            assert!(node.is_modal());
            assert_eq!(node.live(), Some(Live::Polite));
            assert_eq!(node.class_name(), Some(AX_PRIVATE));
            assert!(!node.supports_action(gpui_kit::accesskit::Action::Focus));
        });
    }

    /// Draws a text field and keeps the shadow it computed.
    struct Probe(
        FocusHandle,
        std::rc::Rc<std::cell::RefCell<Vec<gpui_kit::BoxShadow>>>,
    );

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (focus, seen) = (self.0.clone(), self.1.clone());
            gpui_kit::canvas(
                move |_, window, cx| {
                    let mut field = text_field("field", &focus, |_, _, _| {}, div());
                    *seen.borrow_mut() = field
                        .interactivity()
                        .compute_style(None, None, window, cx)
                        .box_shadow;
                },
                |_, _, _, _| {},
            )
            .size_full()
        }
    }

    /// A focused field wears the buttons' ring, pointer-focused too; an
    /// unfocused one wears nothing.
    #[gpui_kit::test]
    fn a_focused_text_field_wears_the_ring(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let seen = std::rc::Rc::default();
        let kept = std::rc::Rc::clone(&seen);
        let focus = cx.update(|cx| cx.focus_handle());
        let handle = focus.clone();
        let window = cx.open_window(size(px(200.), px(200.)), move |_, _| Probe(handle, kept));
        let mut native = VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| window.render_frame(cx));
        assert!(seen.borrow().is_empty());
        native.update(|window, cx| {
            focus.focus(window, cx);
            window.render_frame(cx);
        });
        assert_eq!(*seen.borrow(), vec![ring()]);
    }
}
