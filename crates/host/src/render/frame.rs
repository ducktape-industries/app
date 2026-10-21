//! A frame's arrival: `replace` adopts the view's new root and retains
//! every native-state map to the paths it still mounts.

use super::commands::walk_authored_paths;
use super::*;

impl ViewTree {
    /// The seat's picture bytes, handed with each root it draws: what a
    /// node that names a picture by hash alone draws.
    pub fn set_pictures(&mut self, pictures: Arc<PictureBytes>) {
        self.pictures = pictures;
    }

    /// Takes what the view built for tooltip routes, each with the table
    /// it came with: a content is its own tree, and names its styles in the
    /// table of the frame that brought it, whatever table the tree has by
    /// now. A response is kept by its route while the tree holds that
    /// route, the newest for a route winning, and a response for a route
    /// the tree does not hold is for nothing.
    pub fn tooltip_responses(
        &mut self,
        responses: Vec<(wire::TooltipResponse, wire::Styles)>,
        cx: &mut Context<Self>,
    ) {
        let routes = tooltip_routes(&self.root);
        let mut changed = false;
        for (response, styles) in responses {
            if !routes.contains(&response.request) {
                continue;
            }
            let Some(content) = response.content else {
                changed |= self.tooltips.remove(&response.request).is_some();
                continue;
            };
            let same = self.tooltips.get(&response.request).is_some_and(|held| {
                held.character_index == response.character_index
                    && *held.content == *content
                    && styles.extends(&held.styles)
            });
            if same {
                continue;
            }
            self.tooltips.insert(
                response.request,
                TooltipContent {
                    character_index: response.character_index,
                    content: Arc::new(*content),
                    styles,
                },
            );
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The view's new root and its style table, with the `Replace` asks
    /// the host still holds for it (target and the revision each read): a
    /// field's edit log is kept from the oldest ask on it.
    pub fn replace(
        &mut self,
        tree: impl Into<Tree>,
        asked: &[(wire::WidgetTarget, u64)],
        cx: &mut Context<Self>,
    ) {
        let Tree { mut root, styles } = tree.into();
        if !styles.extends(&self.styles) {
            for list in self.variable_lists.values_mut() {
                list.rows.clear();
            }
        }
        let mut focusable = HashMap::new();
        let mut view_focus_ids = std::collections::HashSet::new();
        let mut fields = HashMap::new();
        let mut scrolls = std::collections::HashSet::new();
        let mut sideways = std::collections::HashSet::new();
        let mut uniform_lists = std::collections::HashSet::new();
        let mut variable_lists = std::collections::HashSet::new();
        let mut drags = std::collections::HashSet::new();
        let mut dialogs = std::collections::HashSet::new();
        let mut sheets = std::collections::HashSet::new();
        let mut sensors = std::collections::HashSet::new();
        let mut tooltip_routes = std::collections::HashSet::new();
        let mut page = None;
        walk_authored_paths(&root, None, &mut Vec::new(), &mut |node, path| {
            tooltip_routes.extend(tooltip_route(node));
            super::page_motion::first(&mut page, node, path);
            if let wire::Node::Container(view_wire::ContainerNode {
                id: Some(_), style, ..
            }) = node
                && styles[*style].overflow.y == Some(gpui_kit::Overflow::Scroll)
            {
                scrolls.insert(path.clone());
            }
            if let wire::Node::Container(view_wire::ContainerNode {
                id: Some(_), style, ..
            }) = node
                && super::sideways::scrolls_only_sideways(&styles[*style])
            {
                sideways.insert(path.clone());
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
                    if let Some(id) = interactivity.as_ref().and_then(|i| i.focus_handle) {
                        view_focus_ids.insert(id);
                    }
                }
                wire::Node::UniformList {
                    path,
                    interactivity,
                    ..
                } => {
                    if let Some(id) = interactivity.as_ref().and_then(|i| i.focus_handle) {
                        view_focus_ids.insert(id);
                    }
                    uniform_lists.insert(path.clone());
                }
                wire::Node::List {
                    path,
                    interactivity,
                    ..
                } => {
                    if let Some(id) = interactivity.as_ref().and_then(|i| i.focus_handle) {
                        view_focus_ids.insert(id);
                    }
                    variable_lists.insert(path.clone());
                }
                wire::Node::Field { revision, .. } => {
                    fields.insert(path.clone(), *revision);
                }
                wire::Node::ResizeHandle { interactivity, .. } => {
                    if let Some(id) = interactivity.as_ref().and_then(|i| i.focus_handle) {
                        view_focus_ids.insert(id);
                    }
                    drags.insert(path.clone());
                }
                wire::Node::Overlay {
                    label, children, ..
                } if named_overlay(label, children) => {
                    dialogs.insert(path.clone());
                }
                wire::Node::Sheet { label, .. } => {
                    sheets.insert(path.clone());
                    if named_sheet(label) {
                        dialogs.insert(path.clone());
                    }
                }
                wire::Node::Sensor { .. } => {
                    sensors.insert(path.clone());
                }
                _ => {}
            }
        });
        self.turn_page(page, cx);
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
        self.focus_targets
            .retain(|key, (kind, _)| focusable.get(key) == Some(kind));
        self.view_focus_targets
            .retain(|id, _| view_focus_ids.contains(id));
        self.fields.retain(|key, field| {
            let kept = fields.contains_key(key);
            if !kept {
                field.left(cx);
            }
            kept
        });
        for (key, field) in self.fields.iter_mut() {
            let queued = asked
                .iter()
                .filter(|(target, _)| key == target)
                .map(|(_, revision)| *revision)
                .min();
            field.frame(fields[key], queued);
        }
        let dropped: Vec<AuthoredPath> = self
            .sheets
            .keys()
            .filter(|key| !sheets.contains(*key))
            .cloned()
            .collect();
        for key in dropped {
            let Some(state) = self.sheets.remove(&key) else {
                continue;
            };
            if !state.moves() {
                continue;
            }
            let mut leaving = None;
            walk_authored_paths(&self.root, None, &mut Vec::new(), &mut |node, path| {
                if leaving.is_none() && *path == key && matches!(node, wire::Node::Sheet { .. }) {
                    leaving = state.leave(node, &self.styles, path, cx);
                }
            });
            if leaving.is_some() {
                self.leaving = leaving;
            }
        }
        let leaving = self.leaving.as_ref().map(|leaving| &leaving.path);
        let kept = |key: &AuthoredPath| leaving.is_some_and(|path| key.starts_with(path));
        self.scrolls
            .retain(|key, _| scrolls.contains(key) || kept(key));
        self.sideways
            .retain(|key| sideways.contains(key) || kept(key));
        self.uniform_lists
            .retain(|key, _| uniform_lists.contains(key) || kept(key));
        self.variable_lists
            .retain(|key, _| variable_lists.contains(key) || kept(key));
        self.tooltips
            .retain(|request, _| tooltip_routes.contains(request));
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
        self.webs.retain(&root);
        self.media.retain(&root);
        self.calls.retain(&root);
        self.root = root;
        self.styles = styles;
        cx.notify();
    }
}

fn tooltip_route(node: &wire::Node) -> Option<u32> {
    match node {
        wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
        | wire::Node::UniformList { interactivity, .. }
        | wire::Node::List { interactivity, .. }
        | wire::Node::ResizeHandle { interactivity, .. }
        | wire::Node::Image { interactivity, .. }
        | wire::Node::Svg { interactivity, .. } => interactivity
            .as_ref()
            .and_then(|interactivity| interactivity.tooltip.as_ref())
            .map(|tooltip| tooltip.request),
        wire::Node::RichText { tooltip, .. } => *tooltip,
        _ => None,
    }
}

fn tooltip_routes(root: &wire::Node) -> std::collections::HashSet<u32> {
    let mut routes = std::collections::HashSet::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        routes.extend(tooltip_route(node));
        pending.extend(node.children());
    }
    routes
}
