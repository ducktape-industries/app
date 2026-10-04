//! The List node: variable-height rows in a gpui `ListState`. The guest
//! sends a window of rows from `range_start`; a row the list needs and does
//! not have is asked for with one `ListRequest` per frame, and a row it has
//! is rendered under the list's own authored path, which ends in the
//! list's id: a list is a scope of its own (`wire::identity`).
use super::*;

/// A List's retained state, per authored path.
pub(super) struct VariableList {
    pub(super) state: ListState,
    pub(super) rows: HashMap<usize, wire::Node>,
    item_count: usize,
    /// The guest's command-batch revision; commands replay only when it
    /// changes.
    revision: u64,
    /// The rows to ask for in the request already scheduled this frame.
    requested: Option<std::ops::Range<usize>>,
    request_scheduled: bool,
    /// Rows remeasured, summed over renders.
    #[cfg(test)]
    pub(super) remeasured: usize,
}

impl ViewTree {
    pub(super) fn variable_list(
        &mut self,
        node: &wire::Node,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wire::Node::List {
            id,
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
            interactivity,
            children,
        } = node
        else {
            unreachable!()
        };
        let key = path.clone();
        // gpui's list has no id, so its rows are drawn under one of the
        // host's own, which no view id is: two lists' rows never share
        // gpui's element state or an accessibility node
        let wire::ElementIdWire::ListState(state_id) = id else {
            unreachable!("the sanitizer holds a list to its state's id")
        };
        let scope = host_id(format!("list-{state_id}"));
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
                #[cfg(test)]
                remeasured: 0,
            }
        });
        if created {
            for command in commands {
                scroll(list, command);
            }
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
        #[cfg(test)]
        {
            list.remeasured += changed.len();
        }
        for index in changed {
            list.state.remeasure_items(index..index + 1);
        }

        let state = list.state.clone();
        let bottom = native_alignment == ListAlignment::Bottom;
        let weak = cx.entity().downgrade();
        let render_key = key.clone();
        let request_route = *request_handler;
        let native = gpui_kit::list(state.clone(), move |index, window, cx| {
            weak.update(cx, |this, cx| {
                let list = this.variable_lists.get(&render_key);
                let row = list.and_then(|list| list.rows.get(&index)).cloned();
                let count = list.map_or(0, |list| list.item_count);
                // The first row the guest sent is on screen: ask for the one
                // above it. Asking whenever rows were missing above walked a
                // bottom-anchored list back one row a frame, off screen too,
                // re-rendering the guest's whole window every step.
                let leading = bottom
                    && index > 0
                    && row.is_some()
                    && list.is_some_and(|list| !list.rows.contains_key(&(index - 1)));
                if leading {
                    request_row(this, &render_key, index - 1, request_route, cx);
                }
                if let Some(row) = row {
                    let parent = std::mem::replace(&mut this.authored_path, render_key.clone());
                    this.next_row = Some((index + 1, count));
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
            state.set_scroll_handler(move |event, _, cx| {
                let weak = weak.clone();
                let scroll_key = key.clone();
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
        if **interactivity == wire::Interactivity::default() {
            return Scope {
                id: scope,
                element: native.into_any_element(),
            }
            .into_any_element();
        }
        // gpui's list is no interactive element: a list the view roled,
        // named or wired is a box in the list's place, holding it whole
        let mut host = div();
        *host.style() = std::mem::take(native.style());
        let host = host.id(scope).child(native.size_full());
        self.guest_aria(host, node, interactivity, cx)
            .into_any_element()
    }
}

/// Moves `list` as a scroll `command` says; a command on its rows moves
/// nothing here.
fn scroll(list: &mut VariableList, command: &wire::ListCommand) {
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

fn request_row(
    tree: &mut ViewTree,
    key: &AuthoredPath,
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
            wire::ListCommand::ScrollTo(_)
            | wire::ListCommand::ScrollToEnd
            | wire::ListCommand::ScrollToRevealItem(_)
            | wire::ListCommand::SetFollowMode { .. }
            | wire::ListCommand::PauseFollowingTail => scroll(list, command),
        }
    }
}
