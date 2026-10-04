//! The UniformList node: a gpui `uniform_list` over a host-owned viewport.
//! The guest sends the rows of a window around what the host shows, keyed
//! by index, sized from the viewport and the row height the host measured
//! (`UniformListRange`); a row a far scroll reaches before the guest's
//! next frame draws as a placeholder of `PLACEHOLDER_HEIGHT`. The host draws
//! the list's scroll bar in a gutter it keeps beside the list.
use super::*;
use crate::render::native_id;
use gpui_kit::UniformListDecoration;
use std::ops::Range;

const PLACEHOLDER_HEIGHT: f32 = 24.;

/// A UniformList's retained state, per wire `path`.
pub(super) struct UniformListHostState {
    /// The list's request route: another route in this place is another
    /// list, whose rows these are not.
    pub(super) route: u32,
    /// The scroll request revision last applied: a request is a one-shot
    /// the view made, applied when the revision moves and never again for
    /// the frames that carry it on (the reader's wheel keeps its place).
    pub(super) revision: Option<u64>,
    pub(super) count: usize,
    /// The rows the guest sent for the range it was asked, by index.
    pub(super) rows: HashMap<usize, wire::Node>,
    /// How many rows were taken as sent (new, or not the one held).
    #[cfg(test)]
    pub(super) replaced: usize,
    pub(super) scroll: gpui_kit::UniformListScrollHandle,
    /// The last range asked of the guest; asked again only when it changes.
    pub(super) requested: Option<Range<usize>>,
    /// The last (top index, scrollable, scrolled to end) reported.
    observed: Option<(usize, bool, Option<bool>)>,
}

impl UniformListHostState {
    fn new(route: u32, count: usize) -> Self {
        Self {
            route,
            revision: None,
            count,
            rows: HashMap::new(),
            #[cfg(test)]
            replaced: 0,
            scroll: gpui_kit::UniformListScrollHandle::new(),
            requested: None,
            observed: None,
        }
    }
}

/// Not a visual decoration: `compute` is the one per-layout hook gpui's
/// uniform_list gives with the visible range, so it reports the range and
/// the scroll state to the guest (`UniformListRange`, `UniformListState`)
/// and draws nothing.
struct RangeObserver {
    tree: gpui_kit::WeakEntity<ViewTree>,
    path: Vec<wire::ElementIdWire>,
    route: u32,
    scroll: gpui_kit::UniformListScrollHandle,
}

impl UniformListDecoration for RangeObserver {
    fn compute(
        &self,
        visible: Range<usize>,
        _bounds: Bounds<Pixels>,
        _scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        _window: &mut Window,
        app: &mut App,
    ) -> AnyElement {
        let start = visible.start.min(item_count);
        let end = visible
            .end
            .min(item_count)
            .min(start.saturating_add(wire::MAX_UNIFORM_LIST_ROWS));
        let range = start..end;
        let scrollable = self.scroll.is_scrollable();
        let scrolled_to_end = self.scroll.is_scrolled_to_end();
        let _ = self.tree.update(app, |tree, cx| {
            let Some(state) = tree.uniform_lists.get_mut(&self.path) else {
                return;
            };
            if !range.is_empty() && state.requested.as_ref() != Some(&range) {
                state.requested = Some(range.clone());
                cx.emit(wire::Event::UniformListRange {
                    path: self.path.clone(),
                    route: self.route,
                    start: range.start as u32,
                    end: range.end as u32,
                    item_height: f32::from(item_height),
                });
            }
            // gpui's `logical_scroll_top_index` is test-support-only. Its
            // public native state exposes the same pending target and settled
            // logical offset.
            let native = self.scroll.0.borrow();
            let top_index = native
                .deferred_scroll_to_item
                .as_ref()
                .map(|request| request.item_index)
                .unwrap_or_else(|| native.base_handle.logical_scroll_top().0)
                .min(item_count.saturating_sub(1));
            drop(native);
            let observed = (top_index, scrollable, scrolled_to_end);
            if state.observed != Some(observed) {
                state.observed = Some(observed);
                cx.emit(wire::Event::UniformListState {
                    path: self.path.clone(),
                    route: self.route,
                    top_index: top_index as u32,
                    scrollable,
                    scrolled_to_end,
                });
            }
        });
        div().into_any_element()
    }
}

/// The room the view gave the list, taken out of `list`: how the node sits
/// in its parent (shown or not, its place, its size, its margin, its share
/// of a flex or grid parent). The row that holds the list and the bar's
/// gutter takes it; `list` keeps its own box (padding, border, fill, text).
fn room(list: &mut gpui_kit::StyleRefinement) -> gpui_kit::StyleRefinement {
    use std::mem::take;
    gpui_kit::StyleRefinement {
        display: list
            .display
            .take_if(|display| *display == gpui_kit::Display::None),
        visibility: list.visibility.take(),
        opacity: list.opacity.take(),
        position: list.position.take(),
        inset: take(&mut list.inset),
        size: take(&mut list.size),
        min_size: take(&mut list.min_size),
        max_size: take(&mut list.max_size),
        aspect_ratio: list.aspect_ratio.take(),
        margin: take(&mut list.margin),
        align_self: list.align_self.take(),
        flex_basis: list.flex_basis.take(),
        flex_grow: list.flex_grow.take(),
        flex_shrink: list.flex_shrink.take(),
        grid_location: list.grid_location.take(),
        ..Default::default()
    }
}

/// The list and, beside it, the gutter the host keeps for the list's bar:
/// the row takes the list's `room`, the list fills what the gutter leaves,
/// so the list's own box (its rows, its fill, its ring) ends where the
/// gutter begins and no view writes padding for a bar it did not draw. The
/// gutter is kept whether or not the list scrolls, as a view's scroller
/// keeps its own (the SDK's `bar_gutter`), and the wheel over it scrolls
/// the list, as it does over a scroller's bar.
fn beside_its_bar(
    room: gpui_kit::StyleRefinement,
    list: impl IntoElement,
    scroll: &gpui_kit::UniformListScrollHandle,
    tree: gpui_kit::EntityId,
) -> impl IntoElement {
    use gpui_base::ScrollbarHandle as _;
    let wheel = scroll.clone();
    let gutter = div()
        .w(gpui_kit::component::scroll::Scrollbar::width())
        .flex_none()
        .relative()
        .on_scroll_wheel(move |event, window, cx| {
            let delta = event.delta.pixel_delta(window.line_height());
            let offset = wheel.offset();
            // the list keeps the offset inside its rows when it next draws
            wheel.set_offset(point(offset.x, offset.y + delta.y));
            cx.notify(tree);
            cx.stop_propagation();
        })
        .child(super::layout::vertical_bar(scroll).viewport_from_layout());
    let mut row = div();
    *row.style() = room;
    // a list the view hid stays hidden; any other is a row of two
    row.style().display.get_or_insert(gpui_kit::Display::Flex);
    row.child(list).child(gutter)
}

impl ViewTree {
    pub(super) fn uniform_list(
        &mut self,
        node: &wire::Node,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::UniformList {
            id,
            path,
            route,
            style,
            interactivity,
            count,
            measure_index,
            sizing,
            horizontal_sizing,
            y_flipped,
            scroll_request,
            revision,
            indices,
            children,
        } = node
        else {
            unreachable!()
        };
        let count = (*count).min(wire::MAX_UNIFORM_LIST_COUNT);
        let state = self
            .uniform_lists
            .entry(path.clone())
            .or_insert_with(|| UniformListHostState::new(*route, count));
        if state.route != *route {
            state.route = *route;
            state.revision = None;
            state.rows.clear();
            state.requested = None;
            state.observed = None;
        }
        state.count = count;
        let measure_index = (*measure_index).min(count.saturating_sub(1));
        // the rows are the ones sent: a row held as sent stays as it is
        let sent = indices
            .iter()
            .zip(children)
            .take(wire::MAX_UNIFORM_LIST_ROWS);
        for (&index, child) in sent.clone() {
            let index = index as usize;
            if index < count && state.rows.get(&index) != Some(child) {
                state.rows.insert(index, child.clone());
                #[cfg(test)]
                {
                    state.replaced += 1;
                }
            }
        }
        state
            .rows
            .retain(|index, _| sent.clone().any(|(&sent, _)| sent as usize == *index));

        let native_id = native_id(id);
        let scroll = state.scroll.clone();
        let asked = state.revision != Some(*revision);
        state.revision = Some(*revision);
        if let Some(request) = scroll_request.filter(|_| asked) {
            let strategy = match request.strategy {
                wire::list::UniformListScrollStrategy::Top => gpui_kit::ScrollStrategy::Top,
                wire::list::UniformListScrollStrategy::Center => gpui_kit::ScrollStrategy::Center,
                wire::list::UniformListScrollStrategy::Bottom => gpui_kit::ScrollStrategy::Bottom,
                wire::list::UniformListScrollStrategy::Nearest => gpui_kit::ScrollStrategy::Nearest,
            };
            if request.strict {
                scroll.scroll_to_item_strict_with_offset(request.index, strategy, request.offset);
            } else {
                scroll.scroll_to_item_with_offset(request.index, strategy, request.offset);
            }
        }

        let list_path = path.clone();
        let weak = cx.entity().downgrade();
        let mut list =
            gpui_kit::uniform_list(native_id.clone(), count, move |range, window, app| {
                let start = range.start.min(count);
                let end = range
                    .end
                    .min(count)
                    .min(start.saturating_add(wire::MAX_UNIFORM_LIST_ROWS));
                let range = start..end;
                weak.update(app, |tree, cx| {
                    let rows = tree
                        .uniform_lists
                        .get(&list_path)
                        .map(|state| {
                            range
                                .clone()
                                .map(|index| state.rows.get(&index).cloned())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let parent = std::mem::replace(&mut tree.authored_path, list_path.clone());
                    let elements = range
                        .clone()
                        .zip(rows)
                        .map(|(index, row)| {
                            row.map(|row| {
                                tree.next_row = Some((index + 1, count));
                                tree.node(&row, window, cx)
                            })
                            .unwrap_or_else(|| div().h(px(PLACEHOLDER_HEIGHT)).into_any_element())
                        })
                        .collect();
                    tree.authored_path = parent;
                    elements
                })
                .unwrap_or_else(|_| {
                    range
                        .map(|_| div().h(px(PLACEHOLDER_HEIGHT)).into_any_element())
                        .collect()
                })
            })
            .with_width_from_item((count > 0).then_some(measure_index))
            .with_sizing_behavior(match sizing {
                wire::list::UniformListSizing::Infer => ListSizingBehavior::Infer,
                wire::list::UniformListSizing::Auto => ListSizingBehavior::Auto,
            })
            .with_horizontal_sizing_behavior(match horizontal_sizing {
                wire::list::UniformListHorizontalSizing::FitList => {
                    gpui_kit::ListHorizontalSizingBehavior::FitList
                }
                wire::list::UniformListHorizontalSizing::Unconstrained => {
                    gpui_kit::ListHorizontalSizingBehavior::Unconstrained
                }
            })
            .track_scroll(&scroll)
            .y_flipped(*y_flipped)
            .with_decoration(RangeObserver {
                tree: cx.entity().downgrade(),
                path: path.clone(),
                route: *route,
                scroll: scroll.clone(),
            });
        let mut style = self.styles[*style].clone();
        let room = room(&mut style);
        *list.style() = style;
        let list = list.flex_grow(1.).flex_shrink(1.).min_w_0().id(native_id);
        let list = self.guest_aria(list, node, interactivity, cx);
        beside_its_bar(room, list, &scroll, cx.entity_id()).into_any_element()
    }
}
