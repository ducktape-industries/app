//! Shared presentation helpers: icons, popover surfaces, toolbar buttons,
//! and the accessibility helpers the app shares with this crate.

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, ThemeStyled as _};
use gpui_kit::{
    AccessibleAction, App, Div, ElementId, FocusHandle, InteractiveElement, Interactivity,
    IntoElement, MouseButton, ParentElement as _, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled as _, Window, div,
};

use super::theme::ActiveEditorTheme;

/// An icon from the bundled Lucide set, named the way the Tiptap template
/// names them (`heading-1`, `list-ordered`, …).
#[derive(Clone, Copy)]
pub struct Lucide(pub &'static str);

impl gpui_kit::assets::IconNamed for Lucide {
    fn path(self) -> SharedString {
        format!("icons/{}.svg", self.0).into()
    }
}

pub fn icon(name: &'static str, size: gpui_kit::Pixels, color: gpui_kit::Hsla) -> impl IntoElement {
    gpui_kit::svg()
        .path(Lucide(name).path_string())
        .size(size)
        .flex_none()
        .text_color(color)
}

impl Lucide {
    fn path_string(self) -> SharedString {
        format!("icons/{}.svg", self.0).into()
    }
}

/// The popover surface shared by the slash menu, toolbar and link editor.
///
/// This is the framework's own popup treatment — the one Select, Combobox and
/// the menus use — so the editor's surfaces cannot drift away from the ones
/// beside them.
pub fn popover_surface(cx: &App) -> Div {
    div().popover_style(cx).p(cx.editor_theme().rems(0.25))
}

/// A clickable element as assistive technology meets it: in the tree as
/// `role`, called `name`. The one way the editor and the app give a bare
/// clickable div a role, so a role never arrives without a name.
pub trait Control: StatefulInteractiveElement {
    fn control(self, role: Role, name: impl Into<SharedString>) -> Self {
        self.role(role).aria_label(name)
    }

    /// [`keyboard`], in a builder chain.
    fn keyboard(self) -> Self
    where
        Self: Sized,
    {
        keyboard(self)
    }
}

impl<E: StatefulInteractiveElement> Control for E {}

/// A control a keyboard reaches with Tab and presses with Enter or Space, as
/// a pointer presses it. A pointer's press does not move focus to it, so
/// pressing it leaves the caret where it was.
pub fn keyboard<E: StatefulInteractiveElement>(element: E) -> E {
    element
        .focusable()
        .tab_stop(true)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

/// A button that opens a dropdown menu. The menu opens on the press it
/// listens for, and on Enter or Space; a click listener is what tells
/// assistive technology the button can be pressed at all, and its press
/// lands as that pointer press.
pub fn menu_trigger(button: Button) -> Button {
    button.on_click(|_, _, _| {})
}

/// An icon-only button. `name` is both its tooltip and its accessible name,
/// so a tooltip is never the only thing that says what the button does.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: &'static str,
    name: impl Into<SharedString>,
) -> Button {
    let name = name.into();
    Button::new(id)
        .icon(Lucide(icon))
        .tooltip(name.clone())
        .accessibility_label(name)
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
/// stop on an inner element with no role, so Tab put focus where no node was
/// and a screen reader read out the whole window, while their own node
/// tracked a frame handle Tab never reaches. This node tracks the text's own
/// `focus` handle, so the field stays one tab stop and focus on it is focus
/// here, and hands SetValue to `set_value`. The caller draws `field` with no
/// node of its own (`RoleOverride::Presentational`) and sets the role, the
/// name and the states here.
pub fn text_field(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
    field: impl IntoElement,
) -> Stateful<Div> {
    // full width, as the kit draws the field it holds
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
/// door (#114) masks them before they leave the process. Assistive
/// technology still reads them — they are on the screen.
pub const AX_PRIVATE: &str = "ax_private";

/// Marks `element`'s node [`AX_PRIVATE`].
pub fn ax_private(element: Stateful<Div>) -> Stateful<Div> {
    element.a11y_synthetic_children(|builder| {
        builder.parent_node().set_class_name(AX_PRIVATE);
    })
}

/// A row in a menu: fixed height, hover fill, rounded. In the tree as an
/// option called `name`, reporting whether it is the selected one; `id`
/// must come from the item itself, never its position.
pub fn menu_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    selected: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.editor_theme();
    let row = div()
        .id(id)
        .control(Role::ListBoxOption, name)
        .aria_selected(selected)
        .h(theme.rems(2.))
        .px(theme.rems(0.5))
        .flex()
        .items_center()
        .gap(theme.rems(0.5))
        .rounded(theme.radius)
        .cursor_pointer()
        .text_size(theme.ui_text_size);
    if selected {
        row.bg(cx.theme().accent)
            .text_color(cx.theme().accent_foreground)
    } else {
        row.hover(|this| this.bg(cx.theme().accent.opacity(0.6)))
    }
}

/// A toolbar button: 2rem square, ghost by default, filled when active.
pub fn toolbar_button(
    id: impl Into<gpui_kit::ElementId>,
    active: bool,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let theme = cx.editor_theme();
    let button = div()
        .id(id)
        .size(theme.rems(2.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius)
        .cursor_pointer();
    if active {
        button
            .bg(cx.theme().accent)
            .text_color(cx.theme().accent_foreground)
    } else {
        button.hover(|this| this.bg(cx.theme().accent.opacity(0.6)))
    }
}
