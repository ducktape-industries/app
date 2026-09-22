use super::*;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct VariableListKey {
    pub(super) path: Vec<wire::ElementIdWire>,
    pub(super) state: u64,
}

pub(super) struct VariableList {
    pub(super) state: ListState,
    pub(super) rows: HashMap<usize, wire::Node>,
    item_count: usize,
    revision: u64,
    requested: Option<std::ops::Range<usize>>,
    request_scheduled: bool,
}

impl ViewTree {
    pub(super) fn variable_list(
        &mut self,
        node: &wire::Node,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::List {
            state: state_id,
            path,
            item_count,
            alignment,
            overdraw,
            sizing,
            following_tail,
            revision,
            commands,
            request_handler,
            scroll_handler,
            range_start,
            style,
            children,
        } = node
        else {
            unreachable!()
        };
        let key = VariableListKey {
            path: path.clone(),
            state: *state_id,
        };
        let native_alignment = match alignment {
            wire::ListAlignment::Top => ListAlignment::Top,
            wire::ListAlignment::Bottom => ListAlignment::Bottom,
        };
        let mut created = false;
        let list = self.variable_lists.entry(key.clone()).or_insert_with(|| {
            created = true;
            let state = ListState::new(*item_count, native_alignment, px(*overdraw));
            state.set_follow_mode(if *following_tail {
                FollowMode::Tail
            } else {
                FollowMode::Normal
            });
            VariableList {
                state,
                rows: HashMap::new(),
                item_count: *item_count,
                revision: *revision,
                requested: None,
                request_scheduled: false,
            }
        });
        if created {
            apply_initial_commands(list, commands);
        } else if list.revision != *revision {
            apply_commands(list, commands);
            list.revision = *revision;
        }
        if list.item_count != *item_count {
            list.state.reset(*item_count);
            list.rows.clear();
            list.item_count = *item_count;
        }
        let incoming = *range_start..range_start.saturating_add(children.len());
        let mut changed = Vec::new();
        for (index, row) in incoming.clone().zip(children) {
            if list.rows.get(&index) != Some(row) {
                list.rows.insert(index, row.clone());
                changed.push(index);
            }
        }
        list.rows.retain(|index, _| incoming.contains(index));
        for index in changed {
            list.state.remeasure_items(index..index + 1);
        }

        let request_leading = native_alignment == ListAlignment::Bottom && incoming.start > 0;
        let state = list.state.clone();
        if request_leading {
            request_row(self, &key, incoming.start - 1, *request_handler, cx);
        }
        let weak = cx.entity().downgrade();
        let render_key = key.clone();
        let request_route = *request_handler;
        let native = gpui_kit::list(state.clone(), move |index, window, cx| {
            weak.update(cx, |this, cx| {
                let row = this
                    .variable_lists
                    .get(&render_key)
                    .and_then(|list| list.rows.get(&index))
                    .cloned();
                if let Some(row) = row {
                    let parent =
                        std::mem::replace(&mut this.authored_path, render_key.path.clone());
                    let element = this.node(&row, window, cx);
                    this.authored_path = parent;
                    return element;
                }
                request_row(this, &render_key, index, request_route, cx);
                div().w_full().h(px(44.)).into_any_element()
            })
            .unwrap_or_else(|_| div().w_full().h(px(44.)).into_any_element())
        })
        .with_sizing_behavior(match sizing {
            wire::ListSizingBehavior::Infer => ListSizingBehavior::Infer,
            wire::ListSizingBehavior::Auto => ListSizingBehavior::Auto,
        });
        let mut native = native;
        *native.style() = style.clone();

        if let Some(handler) = *scroll_handler {
            let weak = cx.entity().downgrade();
            let scroll_key = key.clone();
            state.set_scroll_handler(move |event, _, cx| {
                let weak = weak.clone();
                let scroll_key = scroll_key.clone();
                let event = wire::ListScroll {
                    visible_start: event.visible_range.start,
                    visible_end: event.visible_range.end,
                    count: event.count,
                    is_scrolled: event.is_scrolled,
                    is_following_tail: event.is_following_tail,
                    offset: wire::ListOffset::default(),
                };
                cx.defer(move |cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let Some(list) = this.variable_lists.get(&scroll_key) else {
                            return;
                        };
                        let mut event = event;
                        let offset = list.state.logical_scroll_top();
                        event.offset = wire::ListOffset {
                            item_ix: offset.item_ix,
                            offset_in_item: f32::from(offset.offset_in_item),
                        };
                        cx.emit(wire::Event::ListScroll { handler, event });
                    });
                });
            });
        }
        native.into_any_element()
    }
}

fn apply_initial_commands(list: &mut VariableList, commands: &[wire::ListCommand]) {
    for command in commands {
        match *command {
            wire::ListCommand::ScrollTo(offset) => list.state.scroll_to(gpui_kit::ListOffset {
                item_ix: offset.item_ix,
                offset_in_item: px(offset.offset_in_item),
            }),
            wire::ListCommand::ScrollToEnd => list.state.scroll_to_end(),
            wire::ListCommand::ScrollToRevealItem(index) => list.state.scroll_to_reveal_item(index),
            wire::ListCommand::SetFollowMode { tail } => list.state.set_follow_mode(if tail {
                FollowMode::Tail
            } else {
                FollowMode::Normal
            }),
            wire::ListCommand::PauseFollowingTail => list.state.pause_following_tail(),
            wire::ListCommand::Reset { .. }
            | wire::ListCommand::Splice { .. }
            | wire::ListCommand::Remeasure { .. } => {}
        }
    }
}

fn request_row(
    tree: &mut ViewTree,
    key: &VariableListKey,
    index: usize,
    handler: u32,
    cx: &mut Context<ViewTree>,
) {
    let Some(list) = tree.variable_lists.get_mut(key) else {
        return;
    };
    let start = index.min(list.item_count);
    let end = start.saturating_add(1).min(list.item_count);
    list.requested = Some(match list.requested.take() {
        Some(range) => {
            let start = range.start.min(start);
            start
                ..range
                    .end
                    .max(end)
                    .min(start.saturating_add(wire::MAX_LIST_ROWS))
        }
        None => start..end,
    });
    if list.request_scheduled {
        return;
    }
    list.request_scheduled = true;
    let weak = cx.entity().downgrade();
    let key = key.clone();
    cx.defer(move |cx| {
        let _ = weak.update(cx, |this, cx| {
            let Some(list) = this.variable_lists.get_mut(&key) else {
                return;
            };
            list.request_scheduled = false;
            let Some(range) = list.requested.take() else {
                return;
            };
            cx.emit(wire::Event::ListRequest {
                handler,
                request: wire::ListRequest {
                    start: range.start,
                    end: range.end,
                },
            });
        });
    });
}

fn apply_commands(list: &mut VariableList, commands: &[wire::ListCommand]) {
    for command in commands.iter().take(wire::MAX_LIST_COMMANDS) {
        match *command {
            wire::ListCommand::Reset { count } => {
                let count = count.min(wire::MAX_LIST_ITEMS);
                list.state.reset(count);
                list.rows.clear();
                list.item_count = count;
            }
            wire::ListCommand::Splice { start, end, count } => {
                let current = list.state.item_count();
                let start = start.min(current);
                let end = end.clamp(start, current);
                let count = count.min(wire::MAX_LIST_ITEMS.saturating_sub(current - (end - start)));
                list.state.splice(start..end, count);
                let delta = count as isize - (end - start) as isize;
                list.rows = std::mem::take(&mut list.rows)
                    .into_iter()
                    .filter_map(|(index, row)| {
                        if (start..end).contains(&index) {
                            None
                        } else if index >= end {
                            Some(((index as isize + delta) as usize, row))
                        } else {
                            Some((index, row))
                        }
                    })
                    .collect();
                list.item_count = list.state.item_count();
            }
            wire::ListCommand::Remeasure { start, end } => {
                let count = list.state.item_count();
                let start = start.min(count);
                list.state.remeasure_items(start..end.clamp(start, count));
            }
            wire::ListCommand::ScrollTo(offset) => list.state.scroll_to(gpui_kit::ListOffset {
                item_ix: offset.item_ix,
                offset_in_item: px(offset.offset_in_item),
            }),
            wire::ListCommand::ScrollToEnd => list.state.scroll_to_end(),
            wire::ListCommand::ScrollToRevealItem(index) => list.state.scroll_to_reveal_item(index),
            wire::ListCommand::SetFollowMode { tail } => list.state.set_follow_mode(if tail {
                FollowMode::Tail
            } else {
                FollowMode::Normal
            }),
            wire::ListCommand::PauseFollowingTail => list.state.pause_following_tail(),
        }
    }
}
