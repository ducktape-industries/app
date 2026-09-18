//! Shared presentation helpers: icons, popover surfaces, toolbar buttons,
//! and the accessibility helpers the app shares with this crate.

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, ThemeStyled as _};
use gpui_kit::{
    App, Div, ElementId, InteractiveElement, Interactivity, IntoElement, Role, SharedString,
    Stateful, StatefulInteractiveElement, Styled as _, div,
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
}

impl<E: StatefulInteractiveElement> Control for E {}

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
