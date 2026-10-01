//! A frame's arrival: `replace` adopts the guest's new root and retains
//! every native-state map to the paths it still mounts, and the runtime
//! hands a new guest instance's editor documents over here.

use super::commands::walk_authored_paths;
use super::*;

impl ViewTree {
    #[cfg(test)]
    pub(crate) fn measured_bounds(&self, path: &[wire::ElementIdWire]) -> Option<Bounds<Pixels>> {
        self.bounds.get(path).copied()
    }

    pub fn set_editor_store(
        &mut self,
        store: crate::editor::wire::EditorStore,
        cx: &mut Context<Self>,
    ) {
        self.editors.clear();
        self.editor_store = Some(store);
        cx.notify();
    }

    pub fn replace(&mut self, mut root: wire::Node, cx: &mut Context<Self>) {
        let mut focusable = HashMap::new();
        let mut guest_focus_ids = std::collections::HashSet::new();
        let mut inputs = std::collections::HashSet::new();
        let mut scrolls = std::collections::HashSet::new();
        let mut uniform_lists = std::collections::HashSet::new();
        let mut variable_lists = std::collections::HashSet::new();
        let mut drags = std::collections::HashSet::new();
        let mut dialogs = std::collections::HashSet::new();
        let mut editors = std::collections::HashSet::new();
        let mut sensors = std::collections::HashSet::new();
        let mut mounted = std::collections::HashSet::new();
        let mut tooltips = std::collections::HashSet::new();
        walk_authored_paths(&root, &mut Vec::new(), &mut |node, path| {
            mounted.insert(path.clone());
            super::tooltip_containment::sources(node, path, &mut tooltips);
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
                wire::Node::Input { .. } => {
                    inputs.insert(path.clone());
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
                wire::Node::Editor { .. } => {
                    editors.insert(path.clone());
                }
                _ => {}
            }
        });
        root.for_each_mut(&mut |node| match node {
            wire::Node::Image {
                hash,
                data: Some(data),
                ..
            } => {
                self.remember_image(*hash, data);
            }
            wire::Node::Svg {
                source:
                    wire::SvgSource::Data {
                        hash,
                        bytes: Some(bytes),
                    },
                ..
            } => {
                self.remember_vector(*hash, bytes);
            }
            _ => {}
        });
        self.bounds.retain(|key, _| mounted.contains(key));
        self.focus_targets
            .retain(|key, (kind, _)| focusable.get(key) == Some(kind));
        self.guest_focus_targets
            .retain(|id, _| guest_focus_ids.contains(id));
        self.fields.retain(|key, _| inputs.contains(key));
        self.scrolls.retain(|key, _| scrolls.contains(key));
        self.uniform_lists.retain(|id, list| {
            list.rows.clear();
            uniform_lists.contains(id)
        });
        self.variable_lists.retain(|id, list| {
            list.rows.clear();
            variable_lists.contains(id)
        });
        self.drags.retain(|key, _| drags.contains(key));
        let opener = &mut self.opener;
        self.dialogs.retain(|key, (_, was)| {
            let open = dialogs.contains(key);
            if !open && was.is_some() {
                *opener = was.take();
            }
            open
        });
        self.editors.retain(|key, _| editors.contains(key));
        self.sensors.retain(|key, _| sensors.contains(key));
        self.retain_tooltip(&tooltips);
        self.root = root;
        cx.notify();
    }
}
