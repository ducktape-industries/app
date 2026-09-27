//! Accessibility helpers for the native screens and the tree presenter: role
//! with name (`Control`), keyboard reach (`keyboard`, `focus_shown`), states
//! set on the element's own node after a kit widget has built it (`aria`,
//! `disabled`, `modal`), one AT node for a kit text input (`text_field`),
//! and the class names the AX test door masks (`private`) or leaves
//! untruncated (`whole`).

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

/// The mark a control reached by the keyboard wears: a grey ring inside its
/// edge, which reads on the light theme and the dark one alike, over a
/// filled button as over a bare word, and moves nothing.
pub fn focus_shown<E: InteractiveElement>(element: E) -> E {
    element.focus_visible(|style| {
        style.shadow(vec![gpui_kit::BoxShadow {
            color: gpui_kit::hsla(0., 0., 0.5, 1.),
            offset: gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            blur_radius: gpui_kit::px(0.),
            spread_radius: gpui_kit::px(2.),
            inset: true,
        }])
    })
}

/// The accessibility setters of any interactive element, kit widgets that
/// do not expose them included. A widget still draws its own role and name
/// over these when it renders; the states it leaves alone stay as set here.
pub fn aria<E: InteractiveElement>(mut element: E, set: impl FnOnce(Aria<'_>) -> Aria<'_>) -> E {
    set(Aria(element.interactivity()));
    element
}

/// Marks the element's accessible node as a modal dialog boundary.
pub fn modal<E: InteractiveElement>(element: E) -> E {
    aria(element, |node| {
        node.a11y_synthetic_children(|tree| tree.parent_node().set_modal())
    })
}

/// An element's accessibility properties, reached through its interactivity.
pub struct Aria<'a>(&'a mut Interactivity);

impl InteractiveElement for Aria<'_> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.0
    }
}

impl StatefulInteractiveElement for Aria<'_> {}

/// Reports `disabled` to assistive technology. GPUI has no setter for it:
/// an element's own node is only reachable while its subtree is built.
pub fn disabled<E: InteractiveElement>(element: E, disabled: bool) -> E {
    if !disabled {
        return element;
    }
    aria(element, |node| {
        node.a11y_synthetic_children(|tree| tree.parent_node().set_disabled())
    })
}

/// A text field as assistive technology meets it: one node, `id`, that Tab
/// and the Focus action both land on. The kit's text inputs keep their tab
/// stop on an inner element with no role, so this node tracks the text's own
/// `focus` handle and hands SetValue to `set_value`. The caller draws `field`
/// with no node of its own and sets the role, the name and the states here.
pub fn text_field(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
    field: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .track_focus(focus)
        .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
            if let Some(gpui_kit::accesskit::ActionData::Value(value)) = data {
                set_value(value.to_string(), window, cx);
            }
        })
        .child(field)
}

/// The class of a node whose name and value are private: the app's test
/// door masks them before they leave the process. Assistive technology
/// still reads them — they are on the screen.
pub const AX_PRIVATE: &str = "ax_private";

/// Class name of a node whose name the AX tree gives whole, past its
/// length cap: a URL an agent must read back in full.
pub const AX_WHOLE: &str = "ax_whole";

/// Marks the element's accessible node [`AX_WHOLE`]: the passkey QR's URL.
pub fn whole<E: InteractiveElement>(element: E) -> E {
    aria(element, |node| {
        node.a11y_synthetic_children(|tree| tree.parent_node().set_class_name(AX_WHOLE))
    })
}

/// Marks the element's accessible node [`AX_PRIVATE`]: the recovery phrase.
pub fn private<E: InteractiveElement>(element: E) -> E {
    aria(element, |node| {
        node.a11y_synthetic_children(|tree| tree.parent_node().set_class_name(AX_PRIVATE))
    })
}
