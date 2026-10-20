//! The List node: variable-height rows in a gpui `ListState`. The view
//! sends a window of rows from `range_start`; a row the list needs and does
//! not have is asked for with one `ListRequest` per frame, and a row it has
//! is rendered under the list's own authored path, which ends in the
//! list's id: a list is a scope of its own (`wire::identity`).
//!
//! Where the list stands (`ListScroll`) is reported once it has settled
//! after any cause — a wheel, a drag, a view's `scroll_to`, a reveal, rows
//! arriving, a resize: the frame after each render compares the list's
//! state with the last report and emits only what changed, never per
//! frame. gpui's own scroll handler fires on a wheel or a drag alone, and
//! with the range from before the move, so it is not used.
use super::*;

pub(super) struct VariableList {
    pub(super) state: ListState,
    pub(super) rows: HashMap<usize, wire::Node>,
    item_count: usize,
    revision: u64,
    requested: Option<std::ops::Range<usize>>,
    request_scheduled: bool,
    alignment: ListAlignment,
    reported: Option<wire::ListScroll>,
    settle_scheduled: bool,
    #[cfg(test)]
    pub(super) remeasured: usize,
}

impl ViewTree {
    pub(super) fn variable_list(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
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
        let scope = native_id(id);
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
                alignment: native_alignment,
                reported: None,
                settle_scheduled: false,
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
        let weak = cx.entity().downgrade();
        let render_key = key.clone();
        let request_route = *request_handler;
        let native = gpui_kit::list(state.clone(), move |index, window, cx| {
            weak.update(cx, |this, cx| {
                let list = this.variable_lists.get(&render_key);
                let row = list.and_then(|list| list.rows.get(&index)).cloned();
                let count = list.map_or(0, |list| list.item_count);
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
        let mut own = Arc::unwrap_or_clone(self.style(*style));
        let room = super::uniform::room(&mut own);
        let mut native = native;
        *native.style() = own;
        let mut native = native.flex_grow(1.).flex_shrink(1.).min_w_0();

        let list = self
            .variable_lists
            .get_mut(&key)
            .expect("the list just drawn");
        if let Some(handler) = *scroll_handler
            && !list.settle_scheduled
        {
            list.settle_scheduled = true;
            let weak = cx.entity().downgrade();
            let settle_key = key.clone();
            window.on_next_frame(move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.settle_list(&settle_key, handler, cx));
            });
        }
        let interactivity = interactivity
            .as_deref()
            .unwrap_or(wire::Interactivity::none());
        let drawn = if *interactivity == wire::Interactivity::default() {
            Scope {
                id: scope.clone(),
                element: native.into_any_element(),
            }
            .into_any_element()
        } else {
            let mut host = div();
            *host.style() = std::mem::take(native.style());
            let host = host.id(scope.clone()).child(native.size_full());
            self.view_aria(host, node, interactivity, cx)
                .into_any_element()
        };
        super::uniform::beside_its_bar(room, drawn, &scope, &state, cx.entity_id())
            .into_any_element()
    }
}

impl ViewTree {
    fn settle_list(&mut self, key: &AuthoredPath, handler: u32, cx: &mut Context<Self>) {
        let Some(list) = self.variable_lists.get_mut(key) else {
            return;
        };
        list.settle_scheduled = false;
        let now = settled(&list.state, list.alignment);
        if list.reported.as_ref() == Some(&now) {
            return;
        }
        list.reported = Some(now.clone());
        cx.emit(wire::Event::ListScroll {
            handler,
            event: now,
        });
    }
}

fn settled(state: &ListState, alignment: ListAlignment) -> wire::ListScroll {
    let count = state.item_count();
    let top = state.logical_scroll_top();
    let visible_start = top.item_ix.min(count);
    let mut visible_end = visible_start;
    while visible_end < count && state.item_is_below_viewport(visible_end) == Some(false) {
        visible_end += 1;
    }
    let resting = match alignment {
        ListAlignment::Top => top.item_ix == 0 && top.offset_in_item == px(0.),
        ListAlignment::Bottom => top.item_ix >= count,
    };
    wire::ListScroll {
        visible_start,
        visible_end,
        count,
        is_scrolled: !resting,
        is_following_tail: state.is_following_tail(),
        offset: wire::ListOffset {
            item_ix: top.item_ix,
            offset_in_item: f32::from(top.offset_in_item),
        },
    }
}

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
        Some(range) => range.start.min(start)..range.end.max(end),
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
    for command in commands {
        match *command {
            wire::ListCommand::Reset { count } => {
                list.state.reset(count);
                list.rows.clear();
                list.item_count = count;
            }
            wire::ListCommand::Splice { start, end, count } => {
                let current = list.state.item_count();
                let start = start.min(current);
                let end = end.clamp(start, current);
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
