//! The UniformList node: a gpui `uniform_list` over a host-owned viewport.
//! The guest sends only the rows the host asked for (`UniformListRange`),
//! keyed by index; a row not yet sent draws as a placeholder of
//! `PLACEHOLDER_HEIGHT`.
use super::*;
use crate::render::native_id;
use gpui_kit::UniformListDecoration;
use std::ops::Range;

const PLACEHOLDER_HEIGHT: f32 = 24.;

/// A UniformList's retained state, per wire `path`.
pub(super) struct UniformListHostState {
    /// The guest's list generation: a new route means row nodes and handler
    /// ids from older frames are invalid, so the rows are dropped.
    pub(super) route: u32,
    pub(super) count: usize,
    pub(super) rows: HashMap<usize, wire::Node>,
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
            count,
            rows: HashMap::new(),
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
        _item_height: Pixels,
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
            state.rows.clear();
            state.requested = None;
            state.observed = None;
        }
        state.count = count;
        let measure_index = (*measure_index).min(count.saturating_sub(1));
        // Listener routes are assigned per accepted guest frame. Never keep a
        // row node from an older frame, because its callback IDs may have been
        // reassigned even when this list's route and identity remain stable.
        state.rows.clear();
        for (&index, child) in indices
            .iter()
            .zip(children)
            .take(wire::MAX_UNIFORM_LIST_ROWS)
        {
            let index = index as usize;
            if index < count {
                state.rows.insert(index, child.clone());
            }
        }

        let native_id = native_id(id);
        let scroll = state.scroll.clone();
        if let Some(request) = scroll_request {
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
                scroll,
            });
        *list.style() = style.clone();
        let list = list.id(native_id);
        self.guest_aria(list, node, interactivity, cx)
            .into_any_element()
    }
}
