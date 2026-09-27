//! Container, Responsive and When nodes, and the layout helpers the
//! other renderers share: `measure` records an element's bounds under its
//! authored path, `over_padding` floats a bar or a measure over a scroller
//! without counting as its content, `vertical_bar` is a scroller's bar.
use super::*;
use crate::render::native_id;

impl ViewTree {
    pub(super) fn container(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Container(view_wire::ContainerNode {
            id,
            style,
            interactivity,
            children,
        }) = node
        else {
            unreachable!()
        };
        let mut element = div();
        *element.style() = style.clone();
        crate::fonts::refine_fallbacks(element.style());
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            ElementId::NamedInteger("guest-container".into(), index)
        });
        let mut element = element.id(native_id);
        if let Some(group) = &interactivity.group {
            element = element.group(group.clone());
        }
        if let Some(style) = &interactivity.hover {
            let style = style.clone();
            element = element.hover(move |_| style);
        }
        if let Some(style) = &interactivity.active {
            let style = style.clone();
            element = element.active(move |_| style);
        }
        if let Some(group) = &interactivity.group_hover {
            let style = group.style.clone();
            element = element.group_hover(group.group.clone(), move |_| style);
        }
        if let Some(group) = &interactivity.group_active {
            let style = group.style.clone();
            element = element.group_active(group.group.clone(), move |_| style);
        }
        if let Some(role) = interactivity.role {
            element = element.role(role);
        }
        if interactivity.focusable {
            element = element.focusable();
        }
        if let Some(value) = &interactivity.aria.author_id {
            element = element.accessibility_id(value.clone());
        }
        if let Some(value) = &interactivity.aria.label {
            element = element.aria_label(value.clone());
        } else if interactivity.role.is_some()
            && let Some(text) = descendant_text(node)
        {
            // A role with no explicit label: a view styling its own button
            // out of a container still gets a name, taken from the text it
            // drew inside — not left silent with its label one level down.
            element = element.aria_label(text);
        }
        if let Some(value) = &interactivity.aria.description {
            element = element.aria_description(value.clone());
        }
        if let Some(value) = &interactivity.aria.keyshortcuts {
            element = element.aria_keyshortcuts(value.clone());
        }
        if let Some(value) = &interactivity.aria.value {
            element = element.aria_value(value.clone());
        }
        if let Some(value) = &interactivity.aria.placeholder {
            element = element.aria_placeholder(value.clone());
        }
        if let Some(value) = interactivity.aria.selected {
            element = element.aria_selected(value);
        }
        if let Some(value) = interactivity.aria.expanded {
            element = element.aria_expanded(value);
        }
        if let Some(value) = interactivity.aria.disabled {
            element = element.aria_disabled(value);
        }
        if let Some(value) = interactivity.aria.numeric_value {
            element = element.aria_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.numeric_value_step {
            element = element.aria_numeric_value_step(value);
        }
        if let Some(value) = interactivity.aria.min_numeric_value {
            element = element.aria_min_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.max_numeric_value {
            element = element.aria_max_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.level {
            element = element.aria_level(value);
        }
        if let Some(value) = interactivity.aria.position_in_set {
            element = element.aria_position_in_set(value);
        }
        if let Some(value) = interactivity.aria.size_of_set {
            element = element.aria_size_of_set(value);
        }
        if let Some(value) = interactivity.aria.row_index {
            element = element.aria_row_index(value);
        }
        if let Some(value) = interactivity.aria.column_index {
            element = element.aria_column_index(value);
        }
        if let Some(value) = interactivity.aria.row_count {
            element = element.aria_row_count(value);
        }
        if let Some(value) = interactivity.aria.column_count {
            element = element.aria_column_count(value);
        }
        if let Some(value) = interactivity.aria.toggled {
            element = element.aria_toggled(value);
        }
        if let Some(value) = interactivity.aria.orientation {
            element = element.aria_orientation(value);
        }
        if interactivity.aria.active_descendant {
            element = element.aria_active_descendant();
        }
        let focus_handle = interactivity.focus_handle.as_ref().map(|id| {
            self.guest_focus_targets
                .entry(*id)
                .or_insert_with(|| cx.focus_handle())
                .clone()
        });
        element = super::interactivity::apply(element, interactivity, focus_handle, cx);
        if let Some(handler) = interactivity.on_click {
            element = element.on_click(cx.listener(
                move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    this.user_activation.set(Some(handler));
                    cx.emit(wire::Event::Click {
                        handler,
                        event: event.into(),
                    });
                },
            ));
        }
        // Only a container with an id of its own owns its path. An id-less one
        // sits on its nearest named ancestor's path: measuring there, it and
        // the ancestor overwrite each other's bounds and notify every frame,
        // so the cached tree re-renders whole, every frame, for good.
        if node.identity().is_some() {
            let path = self.authored_path.clone();
            let kind = std::mem::discriminant(node);
            let restore = self
                .presentation
                .focused_container
                .as_ref()
                .is_some_and(|(saved, saved_kind)| saved == &path && *saved_kind == kind);
            if restore {
                self.presentation.focused_container = None;
                let (_, handle) = self
                    .focus_targets
                    .entry(path.clone())
                    .or_insert_with(|| (kind, cx.focus_handle()));
                handle.focus(window, cx);
            }
            if let Some((_, handle)) = self.focus_targets.get(&path) {
                element = element.track_focus(handle);
            }
            element = match style.overflow.y == Some(gpui_kit::Overflow::Scroll) {
                true => element.child(over_padding(&style.padding, self.measure(&path, cx))),
                false => element.child(self.measure(&path, cx)),
            };
        }
        for child in children {
            element = element.child(self.node(child, window, cx));
        }
        // a view's scroller shows its vertical bar: the bar is absolute, over
        // the scroller's bounds, and the handle keeps the offset across frames.
        // The handle is kept at the scroller's own id: an id-less one would
        // share its parent's path, and so its handle, with any sibling there.
        if style.overflow.y == Some(gpui_kit::Overflow::Scroll) && id.is_some() {
            let handle = self
                .scrolls
                .entry(self.authored_path.clone())
                .or_default()
                .clone();
            element = element
                .track_scroll(&handle)
                .child(over_padding(&style.padding, vertical_bar(&handle)));
        }
        #[cfg(test)]
        let element = {
            use gpui_kit::test::TestSupportExt as _;
            element.test_support()
        };
        element.into_any_element()
    }

    pub(super) fn responsive(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::Responsive { content, .. } = node else {
            unreachable!()
        };
        let weak = cx.entity().downgrade();
        let key = self.authored_path.clone();
        let measure = canvas(
            move |bounds, _, cx| {
                let size = [
                    f32::from(bounds.size.width) as f64,
                    f32::from(bounds.size.height) as f64,
                ];
                let _ = weak.update(cx, |this, cx| {
                    let changed = this.containers.get(&key) != Some(&size);
                    if changed {
                        this.containers.insert(key, size);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .relative()
            .child(self.node(content, window, cx))
            .child(measure)
            .into_any_element()
    }

    pub(super) fn when(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::When {
            id,
            condition,
            children,
            ..
        } = node
        else {
            unreachable!()
        };
        let mut element = div().id(native_id(id)).flex().flex_col();
        if condition.matches(&self.containers) {
            for child in children {
                element = element.child(self.node(child, window, cx));
            }
        }
        element.into_any_element()
    }

    /// A zero-paint absolute canvas that records its bounds into
    /// `bounds[path]` and notifies when they change. Only a node that owns
    /// `path` may measure there (see `container`).
    pub(super) fn measure(
        &self,
        path: &[wire::ElementIdWire],
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let route = path.to_vec();
        let weak = cx.entity().downgrade();
        canvas(
            move |bounds, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    let changed = this.bounds.get(&route) != Some(&bounds);
                    if changed {
                        this.bounds.insert(route, bounds);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }
}

/// An absolute overlay on a scroller (its bar, its measure): gpui sizes a
/// scroller's content from its children's bounds plus its padding, so a
/// child spanning the scroller makes a fitting one scroll by its padding,
/// or a long one past its end. The overlay's frame is the content box (the
/// padding as insets), and the overlay inside it reaches back out over the
/// padding (the padding as negative insets): it spans the scroller and
/// counts as no content.
pub(super) fn over_padding(
    padding: &gpui_kit::EdgesRefinement<gpui_kit::DefiniteLength>,
    overlay: impl IntoElement,
) -> impl IntoElement {
    use gpui_kit::{AbsoluteLength, DefiniteLength, Length, Position, Rems};
    let pad = |edge: &Option<DefiniteLength>, sign: f32| -> Option<Length> {
        Some(Length::Definite(match edge.unwrap_or(px(0.).into()) {
            DefiniteLength::Absolute(AbsoluteLength::Pixels(p)) => px(f32::from(p) * sign).into(),
            DefiniteLength::Absolute(AbsoluteLength::Rems(r)) => Rems(r.0 * sign).into(),
            DefiniteLength::Fraction(f) => DefiniteLength::Fraction(f * sign),
        }))
    };
    let frame = |sign: f32| {
        let mut frame = div();
        let style = frame.style();
        style.position = Some(Position::Absolute);
        style.inset.top = pad(&padding.top, sign);
        style.inset.right = pad(&padding.right, sign);
        style.inset.bottom = pad(&padding.bottom, sign);
        style.inset.left = pad(&padding.left, sign);
        frame
    };
    frame(1.).child(frame(-1.).child(overlay))
}

/// A scroller's vertical bar, always shown while it scrolls.
pub(super) fn vertical_bar(handle: &ScrollHandle) -> impl IntoElement {
    gpui_kit::component::scroll::Scrollbar::vertical(handle)
        .id("scrollbar")
        .mode(gpui_kit::component::scroll::ScrollbarMode::Always)
}
