//! The Container node, and the layout helpers the other renderers share:
//! `measure` records an element's bounds under its authored path,
//! `over_padding` floats a bar or a measure over a scroller without counting
//! as its content, `vertical_bar` is a scroller's bar.
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
        let style = self.style(*style);
        let mut element = div();
        *element.style() = (*style).clone();
        crate::fonts::refine_fallbacks(element.style());
        let native_id = id.as_ref().map(native_id).unwrap_or_else(|| {
            let index = self.render_index;
            self.render_index += 1;
            host_id(format!("container-{index}"))
        });
        let mut element = element.id(native_id);
        element = self.guest_aria(element, node, interactivity, cx);
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
                let handle = super::accessibility::tabbed(handle.clone(), interactivity);
                element = element.track_focus(&handle);
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

    /// A zero-paint absolute canvas that records its bounds into
    /// `bounds[path]` and notifies when they change, and, on the first frame
    /// a claim is on `path`, scrolls to it ([`Self::reveal`]). Only a node
    /// that owns `path` may measure there (see `container`).
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
                        this.bounds.insert(route.clone(), bounds);
                        cx.notify();
                    }
                    let moved = this.claiming.contains(&route)
                        && this.revealed.insert(route.clone())
                        && this.reveal(&route, bounds);
                    if moved {
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

impl ViewTree {
    /// Scrolls the scroller nearest above `path` by the least that shows
    /// `row`, the claimed node's bounds this frame (its top, when it is
    /// taller than the scroller); the next frame draws it there. True when
    /// it moved. A row in a virtual list is left to the list, which the
    /// guest scrolls to the row it claims; only a plain scroller is the
    /// host's to move, and only vertically.
    fn reveal(&self, path: &[wire::ElementIdWire], row: Bounds<Pixels>) -> bool {
        let nearest = (0..path.len()).rev().map(|end| &path[..end]).find(|above| {
            self.scrolls.contains_key(*above)
                || self.uniform_lists.contains_key(*above)
                || self.variable_lists.contains_key(*above)
        });
        let Some(scroller) = nearest.and_then(|above| self.scrolls.get(above)) else {
            return false;
        };
        let view = scroller.bounds();
        let by = match row.top() < view.top() || row.size.height > view.size.height {
            true => view.top() - row.top(),
            false => (view.bottom() - row.bottom()).min(px(0.)),
        };
        if by == px(0.) {
            return false;
        }
        let offset = scroller.offset();
        scroller.set_offset(point(offset.x, offset.y + by));
        true
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
        .id(host_id("scrollbar"))
        .mode(gpui_kit::component::scroll::ScrollbarMode::Always)
}
