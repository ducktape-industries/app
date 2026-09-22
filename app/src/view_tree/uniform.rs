use super::*;
use std::ops::Range;

const PLACEHOLDER_HEIGHT: f32 = 24.;

pub(super) struct UniformListHostState {
    pub(super) route: u32,
    pub(super) count: usize,
    pub(super) rows: HashMap<usize, wire::Node>,
    pub(super) scroll: gpui_kit::UniformListScrollHandle,
    pub(super) requested: Option<Range<usize>>,
}

impl UniformListHostState {
    fn new(route: u32, count: usize) -> Self {
        Self {
            route,
            count,
            rows: HashMap::new(),
            scroll: gpui_kit::UniformListScrollHandle::new(),
            requested: None,
        }
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
            route,
            style,
            interactivity,
            count,
            indices,
            children,
        } = node
        else {
            unreachable!()
        };
        let count = (*count).min(wire::MAX_UNIFORM_LIST_COUNT);
        let state = self
            .uniform_lists
            .entry(id.clone())
            .or_insert_with(|| UniformListHostState::new(*route, count));
        if state.route != *route {
            state.route = *route;
            state.rows.retain(|index, _| *index == 0);
            state.requested = None;
        }
        state.count = count;
        state.rows.retain(|index, _| *index < count && *index == 0);
        for (&index, child) in indices.iter().zip(children).take(wire::MAX_UNIFORM_LIST_ROWS) {
            let index = index as usize;
            if index < count {
                state.rows.insert(index, child.clone());
            }
        }

        let native_id = id.to_gpui().unwrap_or_else(|_| {
            let index = self.render_index;
            self.render_index += 1;
            ElementId::NamedInteger("guest-uniform-list".into(), index)
        });
        let scroll = state.scroll.clone();
        let list_id = id.clone();
        let route = *route;
        let row_parent_id = native_id.clone();
        let weak = cx.entity().downgrade();
        let list = gpui_kit::uniform_list(native_id.clone(), count, move |range, window, app| {
            let start = range.start.min(count);
            let end = range.end.min(count).min(start + wire::MAX_UNIFORM_LIST_ROWS);
            let range = start..end;
            weak.update(app, |tree, cx| {
                let request = !range.is_empty() && range != (0..1);
                let should_emit = request && tree.uniform_lists.get_mut(&list_id).is_some_and(|state| {
                    let changed = state.requested.as_ref() != Some(&range);
                    if changed {
                        state.requested = Some(range.clone());
                    }
                    changed
                });
                if should_emit {
                    cx.emit(wire::Event::UniformListRange {
                        id: list_id.clone(),
                        route,
                        start: range.start as u32,
                        end: range.end as u32,
                    });
                }

                let rows = tree
                    .uniform_lists
                    .get(&list_id)
                    .map(|state| {
                        range
                            .clone()
                            .map(|index| state.rows.get(&index).cloned())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                rows.into_iter()
                    .enumerate()
                    .map(|(offset, row)| {
                        let index = range.start + offset;
                        let row_id = ElementId::NamedChild(
                            Arc::new(row_parent_id.clone()),
                            format!("row:{index}").into(),
                        );
                        let content = row
                            .map(|row| tree.node(&row, window, cx))
                            .unwrap_or_else(|| div().h(px(PLACEHOLDER_HEIGHT)).into_any_element());
                        div().id(row_id).child(content).into_any_element()
                    })
                    .collect()
            })
            .unwrap_or_else(|_| {
                range
                    .map(|index| {
                        div()
                            .id(ElementId::NamedChild(
                                Arc::new(row_parent_id.clone()),
                                format!("row:{index}").into(),
                            ))
                            .h(px(PLACEHOLDER_HEIGHT))
                            .into_any_element()
                    })
                    .collect()
            })
        })
        .track_scroll(&scroll);
        let mut list = list;
        *list.style() = style.clone();
        let mut element = div()
            .id(ElementId::NamedChild(Arc::new(native_id), "shell".into()))
            .child(list);
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
        if interactivity.focusable {
            element = element.focusable();
        }
        if let Some(handler) = interactivity.on_click {
            element = element
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(wire::Event::Message(handler))));
        }
        crate::a11y::aria(element, |mut element| {
            if let Some(role) = interactivity.role {
                element = element.role(role);
            }
            let aria = &interactivity.aria;
            if let Some(id) = &aria.author_id {
                element = element.accessibility_id(id.clone());
            }
            if let Some(label) = &aria.label {
                element = element.aria_label(label.clone());
            }
            if let Some(description) = &aria.description {
                element = element.aria_description(description.clone());
            }
            if let Some(value) = &aria.value {
                element = element.aria_value(value.clone());
            }
            if let Some(value) = aria.numeric_value {
                element = element.aria_numeric_value(value);
            }
            if let Some(value) = aria.numeric_value_step {
                element = element.aria_numeric_value_step(value);
            }
            if let Some(value) = aria.min_numeric_value {
                element = element.aria_min_numeric_value(value);
            }
            if let Some(value) = aria.max_numeric_value {
                element = element.aria_max_numeric_value(value);
            }
            if let Some(value) = aria.selected {
                element = element.aria_selected(value);
            }
            if let Some(value) = aria.expanded {
                element = element.aria_expanded(value);
            }
            if let Some(value) = aria.disabled {
                element = element.aria_disabled(value);
            }
            if let Some(value) = aria.toggled {
                element = element.aria_toggled(value);
            }
            if let Some(value) = aria.level {
                element = element.aria_level(value);
            }
            if let Some(value) = aria.position_in_set {
                element = element.aria_position_in_set(value);
            }
            if let Some(value) = aria.size_of_set {
                element = element.aria_size_of_set(value);
            }
            if let Some(value) = aria.row_index {
                element = element.aria_row_index(value);
            }
            if let Some(value) = aria.column_index {
                element = element.aria_column_index(value);
            }
            if let Some(value) = aria.row_count {
                element = element.aria_row_count(value);
            }
            if let Some(value) = aria.column_count {
                element = element.aria_column_count(value);
            }
            element
        })
        .into_any_element()
    }
}
