//! A guest's anchored popup, fitted to what its slot can show. GPUI's own
//! `anchored` fits the window, but a view paints inside its pane's mask: a
//! menu opened near a pane's edge fitted the window and was cut off by the
//! pane. This one flips to the other side of its anchor point when that
//! side fits, then shifts and clamps into the mask it is painted in.
use super::*;
use gpui_kit::{Anchor, Axis, Edges};

pub(super) struct Fitted {
    pub(super) children: Vec<AnyElement>,
    pub(super) anchor: Anchor,
    pub(super) position: Option<Point<Pixels>>,
    pub(super) local: bool,
    pub(super) offset: Point<Pixels>,
    pub(super) margin: Edges<Pixels>,
}

impl IntoElement for Fitted {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Fitted {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let children: Vec<_> = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect();
        let style = gpui_kit::Style {
            position: gpui_kit::Position::Absolute,
            display: gpui_kit::Display::Flex,
            ..Default::default()
        };
        (
            window.request_layout(style, children.iter().copied(), cx),
            children,
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        children: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(content) = children
            .iter()
            .map(|id| window.layout_bounds(*id))
            .reduce(|all, one| all.union(&one))
        else {
            return;
        };
        let at = match (self.local, self.position) {
            (false, position) => position.unwrap_or(bounds.origin),
            (true, position) => bounds.origin + position.unwrap_or_default(),
        };
        let viewport = Bounds::new(Point::default(), window.viewport_size());
        let limits = window.content_mask().bounds.intersect(&viewport);
        let origin = fit(
            self.anchor,
            at,
            self.offset,
            content.size,
            limits,
            self.margin,
        );
        let offset = origin - bounds.origin;
        window.with_element_offset(point(offset.x.round(), offset.y.round()), |window| {
            for child in &mut self.children {
                child.prepaint(window, cx);
            }
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.paint(window, cx);
        }
    }
}

/// Where a popup of `size` anchored at `at` (moved by `offset`) sits inside
/// `limits` less `margin`: on its asked side of `at` when that fits, the
/// other side when only that fits, then shifted back inside, and pinned to
/// the top left when it is larger than the room.
pub(super) fn fit(
    anchor: Anchor,
    at: Point<Pixels>,
    offset: Point<Pixels>,
    size: Size<Pixels>,
    limits: Bounds<Pixels>,
    margin: Edges<Pixels>,
) -> Point<Pixels> {
    let room = Bounds::from_corners(
        point(limits.left() + margin.left, limits.top() + margin.top),
        point(
            limits.right() - margin.right,
            limits.bottom() - margin.bottom,
        ),
    );
    let place = |anchor: Anchor, offset: Point<Pixels>| {
        Bounds::from_anchor_and_size(anchor, at + offset, size)
    };
    let mut anchor = anchor;
    let mut offset = offset;
    let mut placed = place(anchor, offset);
    let across =
        |bounds: Bounds<Pixels>| bounds.left() < room.left() || bounds.right() > room.right();
    let down =
        |bounds: Bounds<Pixels>| bounds.top() < room.top() || bounds.bottom() > room.bottom();
    if across(placed) {
        let other = anchor.other_side_along(Axis::Horizontal);
        let mirrored = point(-offset.x, offset.y);
        if !across(place(other, mirrored)) {
            (anchor, offset) = (other, mirrored);
            placed = place(anchor, offset);
        }
    }
    if down(placed) {
        let other = anchor.other_side_along(Axis::Vertical);
        let mirrored = point(offset.x, -offset.y);
        if !down(place(other, mirrored)) {
            placed = place(other, mirrored);
        }
    }
    let mut origin = placed.origin;
    origin.x = origin.x.min(room.right() - size.width).max(room.left());
    origin.y = origin.y.min(room.bottom() - size.height).max(room.top());
    origin
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> Bounds<Pixels> {
        // a pane in the middle of the window, not the window itself
        Bounds::new(point(px(200.), px(100.)), size(px(400.), px(300.)))
    }
    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }
    fn menu() -> Size<Pixels> {
        size(px(220.), px(160.))
    }
    fn fits(origin: Point<Pixels>, size: Size<Pixels>, room: Bounds<Pixels>) -> bool {
        let popup = Bounds::new(origin, size);
        popup.intersect(&room) == popup
    }

    #[test]
    fn a_popup_that_fits_stays_where_it_was_asked() {
        let origin = fit(
            Anchor::TopLeft,
            at(250., 150.),
            Point::default(),
            menu(),
            slot(),
            Edges::default(),
        );
        assert_eq!(origin, at(250., 150.));
    }

    #[test]
    fn near_the_bottom_it_opens_above_the_point() {
        let origin = fit(
            Anchor::TopLeft,
            at(250., 380.),
            Point::default(),
            menu(),
            slot(),
            Edges::default(),
        );
        assert_eq!(origin, at(250., 220.), "its bottom on the point");
    }

    #[test]
    fn near_the_right_it_opens_to_the_left_of_the_point() {
        let origin = fit(
            Anchor::TopLeft,
            at(580., 150.),
            Point::default(),
            menu(),
            slot(),
            Edges::default(),
        );
        assert_eq!(origin, at(360., 150.), "its right edge on the point");
    }

    #[test]
    fn an_offset_is_mirrored_with_the_side() {
        let origin = fit(
            Anchor::TopLeft,
            at(250., 380.),
            at(0., 4.),
            menu(),
            slot(),
            Edges::default(),
        );
        assert_eq!(origin, at(250., 216.), "4px above, as it was 4px below");
    }

    #[test]
    fn when_neither_side_fits_it_is_clamped_inside_less_the_margin() {
        let tall = size(px(220.), px(250.));
        let origin = fit(
            Anchor::TopLeft,
            at(590., 250.),
            Point::default(),
            tall,
            slot(),
            Edges::all(px(8.)),
        );
        assert_eq!(origin, at(370., 142.));
        let room = Bounds::from_corners(at(208., 108.), at(592., 392.));
        assert!(fits(origin, tall, room));
    }

    #[test]
    fn larger_than_the_room_it_keeps_its_top_left_in_view() {
        let huge = size(px(900.), px(900.));
        let origin = fit(
            Anchor::BottomRight,
            at(300., 300.),
            Point::default(),
            huge,
            slot(),
            Edges::default(),
        );
        assert_eq!(origin, at(200., 100.));
    }

    #[test]
    fn the_window_is_not_the_room_a_pane_has() {
        // inside the window (0..800) but past the pane's right edge (600)
        let origin = fit(
            Anchor::TopLeft,
            at(500., 150.),
            Point::default(),
            menu(),
            slot(),
            Edges::default(),
        );
        assert!(origin.x + menu().width <= px(600.), "{origin:?}");
    }
}
