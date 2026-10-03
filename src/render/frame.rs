//! A frame's arrival: `replace` adopts the guest's new root and retains
//! every native-state map to the paths it still mounts.

use super::commands::walk_authored_paths;
use super::*;

impl ViewTree {
    #[cfg(test)]
    pub(crate) fn measured_bounds(&self, path: &[wire::ElementIdWire]) -> Option<Bounds<Pixels>> {
        self.bounds.get(path).copied()
    }

    /// The seat's picture bytes, handed with each root it draws: what a
    /// node that names a picture by hash alone draws.
    pub(crate) fn set_pictures(&mut self, pictures: Arc<PictureBytes>) {
        self.pictures = pictures;
    }

    /// Takes what the guest built for tooltip routes. A response is kept by
    /// its route while the tree holds that route, the newest for a route
    /// winning, and a response for a route the tree does not hold is for
    /// nothing. Together the contents are held to the frame's node budget,
    /// as the frames that brought them were: one that would take them past
    /// it starts the store over, so the tooltip the pointer rests on shows.
    pub fn tooltip_responses(
        &mut self,
        responses: Vec<wire::TooltipResponse>,
        cx: &mut Context<Self>,
    ) {
        let routes = tooltip_routes(&self.root);
        let mut changed = false;
        for response in responses {
            if !routes.contains(&response.request) {
                continue;
            }
            let Some(content) = response.content else {
                changed |= self.tooltips.remove(&response.request).is_some();
                continue;
            };
            let same = self.tooltips.get(&response.request).is_some_and(|held| {
                held.character_index == response.character_index && *held.content == *content
            });
            if same {
                continue;
            }
            let held: usize = (self.tooltips.iter())
                .filter(|(request, _)| **request != response.request)
                .map(|(_, held)| held.content.count())
                .sum();
            if held + content.count() > wire::MAX_NODES {
                self.tooltips.clear();
            }
            self.tooltips.insert(
                response.request,
                TooltipContent {
                    character_index: response.character_index,
                    content: Arc::new(*content),
                },
            );
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The guest's new root, with the `Replace` asks the host still holds
    /// for it (target and the revision each read): a field's edit log is kept
    /// from the oldest ask on it.
    pub fn replace(
        &mut self,
        mut root: wire::Node,
        asked: &[(wire::WidgetTarget, u64)],
        cx: &mut Context<Self>,
    ) {
        let mut focusable = HashMap::new();
        let mut guest_focus_ids = std::collections::HashSet::new();
        // every field, with the guest's revision this frame carries
        let mut fields = HashMap::new();
        let mut scrolls = std::collections::HashSet::new();
        let mut uniform_lists = std::collections::HashSet::new();
        let mut variable_lists = std::collections::HashSet::new();
        let mut drags = std::collections::HashSet::new();
        let mut dialogs = std::collections::HashSet::new();
        let mut sensors = std::collections::HashSet::new();
        let mut mounted = std::collections::HashSet::new();
        let mut tooltip_routes = std::collections::HashSet::new();
        walk_authored_paths(&root, None, &mut Vec::new(), &mut |node, path| {
            mounted.insert(path.clone());
            tooltip_routes.extend(tooltip_route(node));
            // a scrolling container with an id keeps its handle; an id-less
            // one has no path of its own to keep it at
            if let wire::Node::Container(view_wire::ContainerNode {
                id: Some(_), style, ..
            }) = node
                && style.overflow.y == Some(gpui_kit::Overflow::Scroll)
            {
                scrolls.insert(path.clone());
            }
            if matches!(
                node,
                wire::Node::Container(view_wire::ContainerNode { id: Some(_), .. })
            ) {
                focusable.insert(path.clone(), std::mem::discriminant(node));
            }
            match node {
                wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
                | wire::Node::Image { interactivity, .. }
                | wire::Node::Svg { interactivity, .. } => {
                    if let Some(id) = interactivity.focus_handle {
                        guest_focus_ids.insert(id);
                    }
                }
                wire::Node::UniformList {
                    path,
                    interactivity,
                    ..
                } => {
                    if let Some(id) = interactivity.focus_handle {
                        guest_focus_ids.insert(id);
                    }
                    uniform_lists.insert(path.clone());
                }
                wire::Node::List {
                    path,
                    state,
                    interactivity,
                    ..
                } => {
                    if let Some(id) = interactivity.focus_handle {
                        guest_focus_ids.insert(id);
                    }
                    variable_lists.insert(VariableListKey {
                        path: path.clone(),
                        state: *state,
                    });
                }
                wire::Node::Field { revision, .. } => {
                    fields.insert(path.clone(), *revision);
                }
                wire::Node::ResizeHandle { interactivity, .. } => {
                    if let Some(id) = interactivity.focus_handle {
                        guest_focus_ids.insert(id);
                    }
                    drags.insert(path.clone());
                }
                wire::Node::Overlay {
                    label, children, ..
                } if named_overlay(label, children) => {
                    dialogs.insert(path.clone());
                }
                wire::Node::Sensor { .. } => {
                    sensors.insert(path.clone());
                }
                _ => {}
            }
        });
        self.images.borrow_mut().next_frame();
        root.for_each_mut(&mut |node| {
            if let wire::Node::Image {
                hash,
                data: Some(data),
                ..
            } = node
            {
                self.remember_image(*hash, data, None, cx);
            }
        });
        self.bounds.retain(|key, _| mounted.contains(key));
        self.focus_targets
            .retain(|key, (kind, _)| focusable.get(key) == Some(kind));
        self.guest_focus_targets
            .retain(|id, _| guest_focus_ids.contains(id));
        self.fields.retain(|key, _| fields.contains_key(key));
        for (key, field) in self.fields.iter_mut() {
            // an ask names its target by the whole authored path, as the
            // guest SDK sends it and `execute_widget_command` reads it
            let queued = asked
                .iter()
                .filter(|(target, _)| key == target)
                .map(|(_, revision)| *revision)
                .min();
            field.frame(fields[key], queued);
        }
        self.scrolls.retain(|key, _| scrolls.contains(key));
        self.uniform_lists
            .retain(|id, _| uniform_lists.contains(id));
        self.tooltips
            .retain(|request, _| tooltip_routes.contains(request));
        self.variable_lists
            .retain(|id, _| variable_lists.contains(id));
        self.drags.retain(|key, _| drags.contains(key));
        let opener = &mut self.opener;
        self.dialogs.retain(|key, (entry, was)| {
            let open = dialogs.contains(key);
            if !open && let Some(was) = was.take() {
                *opener = Some((entry.clone(), was));
            }
            open
        });
        self.sensors.retain(|key, _| sensors.contains(key));
        self.root = root;
        cx.notify();
    }
}

/// The tooltip route a node holds, if it holds one.
fn tooltip_route(node: &wire::Node) -> Option<u32> {
    match node {
        wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
        | wire::Node::UniformList { interactivity, .. }
        | wire::Node::List { interactivity, .. }
        | wire::Node::ResizeHandle { interactivity, .. }
        | wire::Node::Image { interactivity, .. }
        | wire::Node::Svg { interactivity, .. } => interactivity
            .tooltip
            .as_ref()
            .map(|tooltip| tooltip.request),
        wire::Node::RichText { tooltip, .. } => *tooltip,
        _ => None,
    }
}

/// Every tooltip route the tree holds: the requests a response may answer.
fn tooltip_routes(root: &wire::Node) -> std::collections::HashSet<u32> {
    let mut routes = std::collections::HashSet::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        routes.extend(tooltip_route(node));
        pending.extend(node.children());
    }
    routes
}
