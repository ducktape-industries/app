use super::*;
use gpui_kit::UniformListDecoration;
use std::ops::Range;

const PLACEHOLDER_HEIGHT: f32 = 24.;

pub(super) struct UniformListHostState {
    pub(super) route: u32,
    pub(super) count: usize,
    pub(super) rows: HashMap<usize, wire::Node>,
    pub(super) scroll: gpui_kit::UniformListScrollHandle,
    pub(super) requested: Option<Range<usize>>,
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
            let top_index = self
                .scroll
                .logical_scroll_top_index()
                .min(item_count.saturating_sub(1));
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

        let native_id = id.to_gpui().expect("validated uniform-list identity");
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
                    let elements = rows.into_iter()
                        .map(|row| {
                            row.map(|row| tree.node(&row, window, cx))
                                .unwrap_or_else(|| {
                                    div().h(px(PLACEHOLDER_HEIGHT)).into_any_element()
                                })
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
        let mut list = list.id(native_id);
        if let Some(group) = &interactivity.group {
            list = list.group(group.clone());
        }
        if let Some(hover) = &interactivity.hover {
            let hover = hover.clone();
            list = list.hover(move |_| hover);
        }
        if let Some(active) = &interactivity.active {
            let active = active.clone();
            list = list.active(move |_| active);
        }
        if let Some(group) = &interactivity.group_hover {
            let style = group.style.clone();
            list = list.group_hover(group.group.clone(), move |_| style);
        }
        if let Some(group) = &interactivity.group_active {
            let style = group.style.clone();
            list = list.group_active(group.group.clone(), move |_| style);
        }
        if interactivity.focusable {
            list = list.focusable();
        }
        if let Some(handler) = interactivity.on_click {
            list = list.on_click(
                cx.listener(move |this, event: &gpui_kit::ClickEvent, _, cx| {
                    this.user_activation.set(Some(handler));
                    cx.emit(wire::Event::Click {
                        handler,
                        event: event.into(),
                    })
                }),
            );
        }
        if let Some(role) = interactivity.role {
            list = list.role(role);
        }
        if let Some(value) = &interactivity.aria.author_id {
            list = list.accessibility_id(value.clone());
        }
        if let Some(value) = &interactivity.aria.label {
            list = list.aria_label(value.clone());
        }
        if let Some(value) = &interactivity.aria.description {
            list = list.aria_description(value.clone());
        }
        if let Some(value) = &interactivity.aria.keyshortcuts {
            list = list.aria_keyshortcuts(value.clone());
        }
        if let Some(value) = &interactivity.aria.value {
            list = list.aria_value(value.clone());
        }
        if let Some(value) = &interactivity.aria.placeholder {
            list = list.aria_placeholder(value.clone());
        }
        if let Some(value) = interactivity.aria.selected {
            list = list.aria_selected(value);
        }
        if let Some(value) = interactivity.aria.expanded {
            list = list.aria_expanded(value);
        }
        if let Some(value) = interactivity.aria.disabled {
            list = list.aria_disabled(value);
        }
        if let Some(value) = interactivity.aria.numeric_value {
            list = list.aria_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.numeric_value_step {
            list = list.aria_numeric_value_step(value);
        }
        if let Some(value) = interactivity.aria.min_numeric_value {
            list = list.aria_min_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.max_numeric_value {
            list = list.aria_max_numeric_value(value);
        }
        if let Some(value) = interactivity.aria.level {
            list = list.aria_level(value);
        }
        if let Some(value) = interactivity.aria.position_in_set {
            list = list.aria_position_in_set(value);
        }
        if let Some(value) = interactivity.aria.size_of_set {
            list = list.aria_size_of_set(value);
        }
        if let Some(value) = interactivity.aria.row_index {
            list = list.aria_row_index(value);
        }
        if let Some(value) = interactivity.aria.column_index {
            list = list.aria_column_index(value);
        }
        if let Some(value) = interactivity.aria.row_count {
            list = list.aria_row_count(value);
        }
        if let Some(value) = interactivity.aria.column_count {
            list = list.aria_column_count(value);
        }
        if let Some(value) = interactivity.aria.toggled {
            list = list.aria_toggled(value);
        }
        if let Some(value) = interactivity.aria.orientation {
            list = list.aria_orientation(value);
        }
        list.into_any_element()
    }
}
