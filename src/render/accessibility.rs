//! What a wire node is to assistive technology. `accessible` maps a node to
//! an `Accessible` (role, name, value, states); `announce` writes one onto
//! the element a renderer built; `guest_aria` writes the guest's own
//! `Interactivity.aria` for the nodes that carry it (Container, Image, Svg,
//! UniformList). `ViewTree::presentation` also lives here
//! for now, though it is not accessibility: it is the native state carried
//! across a guest generation (see its doc).
use super::*;

/// What one wire node is to assistive technology: the role it plays, the
/// name it is called, and the value and states it reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Accessible {
    pub role: Option<gpui_kit::Role>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Text a field holds. Never a secure field's.
    pub value: Option<String>,
    pub numeric: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub toggled: Option<bool>,
    pub expanded: Option<bool>,
    pub selected: Option<bool>,
    /// A heading's level, 1 to 6.
    pub level: Option<usize>,
    /// How a change to text that is not focused is announced.
    pub live: Option<wire::Live>,
    /// A control with no handler: it is drawn, and does nothing.
    pub disabled: bool,
}

fn named(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// Every text a node's descendants carry, depth first, joined by spaces: a
/// button or a clickable drawn with text children but no explicit label is
/// named by them (`#` + `general` is "# general", not "#"), so nothing in the
/// tree is announced with an empty or truncated name.
pub(crate) fn descendant_text(node: &wire::Node) -> Option<String> {
    fn gather<'a>(node: &'a wire::Node, words: &mut Vec<&'a str>) {
        match node {
            wire::Node::Text(view_wire::TextNode { content, .. }) => {
                let content = content.trim();
                if !content.is_empty() {
                    words.push(content);
                }
            }
            _ => node
                .children()
                .iter()
                .for_each(|child| gather(child, words)),
        }
    }
    let mut words = Vec::new();
    gather(node, &mut words);
    named(&words.join(" "))
}

/// The mapping from a wire node to what assistive technology hears, for the
/// nodes the presenter announces itself: a view never names those controls'
/// roles, it says `label` and the role follows from the variant. Not every
/// variant passes through here. Container, UniformList, Image and Svg carry
/// the guest's own `Interactivity.aria` through `guest_aria` instead (their
/// renderers never call this; the Image/Svg arm below is reached by tests
/// only), the
/// kit Slider draws its own node, and RichText has none. The node's
/// accessibility id is its wire key under the module's view, the element id
/// each variant is already built with.
pub(crate) fn accessible(node: &wire::Node) -> Accessible {
    use gpui_kit::Role;
    use wire::Node;
    let numeric = |value: f32, min: f32, max: f32| Accessible {
        numeric: Some(value.into()),
        min: Some(min.into()),
        max: Some(max.into()),
        ..Default::default()
    };
    let role_of = |role: &wire::Role| match role {
        wire::Role::Button => Role::Button,
        wire::Role::Link => Role::Link,
        wire::Role::Tab => Role::Tab,
        wire::Role::MenuItem => Role::MenuItem,
        wire::Role::Row => Role::Row,
        wire::Role::Checkbox => Role::CheckBox,
        wire::Role::Switch => Role::Switch,
    };
    let labelled = |role: Role, label: &Option<String>| Accessible {
        role: Some(role),
        name: label.as_deref().and_then(named),
        ..Default::default()
    };
    match node {
        Node::Text(view_wire::TextNode {
            content,
            heading,
            live,
            ..
        }) => Accessible {
            role: Some(match heading {
                Some(_) => Role::Heading,
                None => Role::Label,
            }),
            name: named(content),
            level: heading.map(usize::from),
            live: *live,
            ..Default::default()
        },
        Node::Button {
            content,
            label,
            role,
            checked,
            expanded,
            selected,
            description,
            on_press,
            ..
        } => Accessible {
            role: Some(role.as_ref().map_or(Role::Button, role_of)),
            selected: *selected,
            name: label.as_deref().and_then(named).or_else(|| match content {
                wire::ButtonContent::Label(text) => named(text),
                wire::ButtonContent::Child(_) => descendant_text(node),
            }),
            description: description.as_deref().and_then(named),
            toggled: *checked,
            expanded: *expanded,
            disabled: on_press.is_none(),
            ..Default::default()
        },
        Node::Toggle {
            kind,
            label,
            checked,
            on_toggle,
            ..
        } => Accessible {
            role: Some(match kind {
                wire::ToggleKind::Checkbox => Role::CheckBox,
                wire::ToggleKind::Switch => Role::Switch,
            }),
            name: named(label),
            toggled: Some(*checked),
            disabled: on_toggle.is_none(),
            ..Default::default()
        },
        Node::Radio {
            label, selected, ..
        } => Accessible {
            role: Some(Role::RadioButton),
            name: named(label),
            toggled: Some(*selected),
            ..Default::default()
        },
        Node::Slider {
            label,
            value,
            min,
            max,
            step,
            ..
        } => Accessible {
            role: Some(Role::Slider),
            name: label.as_deref().and_then(named),
            step: Some((*step).into()),
            ..numeric(*value, *min, *max)
        },
        Node::Progress {
            value, min, max, ..
        } => Accessible {
            role: Some(Role::ProgressIndicator),
            ..numeric(*value, *min, *max)
        },
        Node::Input {
            options,
            value,
            secure,
            ..
        } => Accessible {
            role: Some(match secure {
                true => Role::PasswordInput,
                false => Role::TextInput,
            }),
            name: named(&options.label),
            description: options.description.as_deref().and_then(named),
            value: (!secure).then(|| value.clone()),
            disabled: options.disabled,
            ..Default::default()
        },
        // the document is not on the node: the editor mount adds its text as
        // the value (`TextEditor::render`)
        Node::Editor {
            label,
            placeholder,
            editable,
            ..
        } => {
            let field = labelled(Role::MultilineTextInput, label);
            Accessible {
                description: field.name.is_none().then(|| named(placeholder)).flatten(),
                disabled: !editable,
                ..field
            }
        }
        Node::ComboBox {
            options,
            selected,
            label,
            ..
        }
        | Node::PickList {
            options,
            selected,
            label,
            ..
        } => Accessible {
            value: selected.and_then(|index| options.get(index as usize).cloned()),
            ..labelled(Role::ComboBox, label)
        },
        // an area is announced only as what its view says it is
        Node::MouseArea {
            role: Some(role),
            label,
            expanded,
            selected,
            checked,
            on_press,
            on_release,
            ..
        } => Accessible {
            toggled: *checked,
            expanded: *expanded,
            selected: *selected,
            disabled: on_press.is_none() && on_release.is_none(),
            name: label
                .as_deref()
                .and_then(named)
                .or_else(|| descendant_text(node)),
            ..labelled(role_of(role), label)
        },
        // only an open named overlay is a dialog; a closed or unnamed one is layout
        Node::Overlay {
            label, children, ..
        } if named_overlay(label, children) => labelled(Role::Dialog, label),
        // the wire gives a code no label: it is read as what it is
        Node::Qr { .. } => Accessible {
            role: Some(Role::Image),
            name: Some("QR code".into()),
            ..Default::default()
        },
        // an unlabelled picture is decoration: it stays out of the tree
        Node::Image {
            label,
            interactivity,
            ..
        }
        | Node::Svg {
            label,
            interactivity,
            ..
        } => {
            let name = label
                .as_deref()
                .and_then(named)
                .or_else(|| interactivity.aria.label.as_deref().and_then(named));
            if name.is_none() && interactivity.role.is_none() {
                Accessible::default()
            } else {
                Accessible {
                    role: interactivity.role.or(Some(Role::Image)),
                    name,
                    description: interactivity.aria.description.as_deref().and_then(named),
                    disabled: interactivity.aria.disabled.unwrap_or(false),
                    ..Default::default()
                }
            }
        }
        Node::ImageViewer { label, .. } => match label.as_deref().and_then(named) {
            Some(name) => Accessible {
                role: Some(Role::Image),
                name: Some(name),
                ..Default::default()
            },
            None => Accessible::default(),
        },
        _ => Accessible::default(),
    }
}

/// Puts `accessible` on an element the presenter built. A kit widget draws
/// its own role, name and value over these; the states it does not report
/// itself (disabled, expanded, a description) are the ones this adds.
pub(crate) fn announce<E: gpui_kit::InteractiveElement>(element: E, accessible: Accessible) -> E {
    let Accessible {
        role,
        name,
        description,
        value,
        numeric,
        min,
        max,
        step,
        toggled,
        expanded,
        selected,
        level,
        // ponytail: the gpui-pre fork has no live-region setter; `live` is
        // mapped and dropped here until it gains one
        live: _,
        disabled,
    } = accessible;
    let element = crate::a11y::aria(element, |mut node| {
        if let Some(role) = role {
            node = node.role(role);
        }
        if let Some(name) = name {
            node = node.aria_label(name);
        }
        if let Some(description) = description {
            node = node.aria_description(description);
        }
        if let Some(value) = value {
            node = node.aria_value(value);
        }
        if let Some(numeric) = numeric {
            node = node.aria_numeric_value(numeric);
        }
        if let Some(min) = min {
            node = node.aria_min_numeric_value(min);
        }
        if let Some(max) = max {
            node = node.aria_max_numeric_value(max);
        }
        if let Some(step) = step {
            node = node.aria_numeric_value_step(step);
        }
        if let Some(toggled) = toggled {
            node = node.aria_toggled(match toggled {
                true => gpui_kit::Toggled::True,
                false => gpui_kit::Toggled::False,
            });
        }
        if let Some(expanded) = expanded {
            node = node.aria_expanded(expanded);
        }
        if let Some(selected) = selected {
            node = node.aria_selected(selected);
        }
        if let Some(level) = level {
            node = node.aria_level(level);
        }
        node
    });
    crate::a11y::disabled(element, disabled)
}

/// Where the renderers that carry the guest's own `Interactivity.aria`
/// differ. The setter chain was copied into Container, Image/Svg and
/// UniformList and the copies drifted; `guest_aria` keeps each call site's
/// behaviour through these until the owner picks one for all.
pub(super) struct Drift<'a> {
    /// A roled element with no `aria.label` is named by this node's
    /// descendant text. Container only; Image, Svg and UniformList stay
    /// unnamed.
    pub(super) name_from: Option<&'a wire::Node>,
    /// `aria.active_descendant` reaches the element. Container only; the
    /// other copies dropped it.
    pub(super) active_descendant: bool,
}

impl ViewTree {
    /// The guest's `Interactivity` as assistive technology receives it, on
    /// the element a renderer built: role, focusable, the author id, every
    /// `aria.*` setter, then the guest focus handle (made once per id, kept
    /// in `guest_focus_targets`) and the rest of the interactivity through
    /// `interactivity::apply`. `on_click` stays with the caller. Each setter
    /// is a field write gpui reads once at prepaint, so where the caller
    /// puts this among its own setters does not matter.
    pub(super) fn guest_aria<E: gpui_kit::StatefulInteractiveElement>(
        &mut self,
        mut element: E,
        interactivity: &wire::Interactivity,
        drift: Drift<'_>,
        cx: &mut Context<Self>,
    ) -> E {
        let aria = &interactivity.aria;
        if let Some(role) = interactivity.role {
            element = element.role(role);
        }
        if interactivity.focusable {
            element = element.focusable();
        }
        if let Some(value) = &aria.author_id {
            element = element.accessibility_id(value.clone());
        }
        if let Some(value) = &aria.label {
            element = element.aria_label(value.clone());
        } else if interactivity.role.is_some()
            && let Some(text) = drift.name_from.and_then(super::descendant_text)
        {
            // A role with no explicit label: a view styling its own button
            // out of a container still gets a name, taken from the text it
            // drew inside — not left silent with its label one level down.
            element = element.aria_label(text);
        }
        if let Some(value) = &aria.description {
            element = element.aria_description(value.clone());
        }
        if let Some(value) = &aria.keyshortcuts {
            element = element.aria_keyshortcuts(value.clone());
        }
        if let Some(value) = &aria.value {
            element = element.aria_value(value.clone());
        }
        if let Some(value) = &aria.placeholder {
            element = element.aria_placeholder(value.clone());
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
        if let Some(value) = aria.toggled {
            element = element.aria_toggled(value);
        }
        if let Some(value) = aria.orientation {
            element = element.aria_orientation(value);
        }
        if drift.active_descendant && aria.active_descendant {
            element = element.aria_active_descendant();
        }
        let focus_handle = interactivity.focus_handle.map(|id| {
            self.guest_focus_targets
                .entry(id)
                .or_insert_with(|| cx.focus_handle())
                .clone()
        });
        super::interactivity::apply(element, interactivity, focus_handle, cx)
    }
}

impl ViewTree {
    /// The native state worth keeping when the view's guest is re-instantiated
    /// (a new generation): each field's text, selection and focus, the focused
    /// container or editor, scroll offsets, and the decoded image and SVG
    /// caches. Plain data only: no native entity, callback, handler id or IME
    /// preedit crosses a generation, since the new guest's handler ids mean
    /// different things. Document selection remains guest-owned.
    pub(crate) fn presentation(&self, window: &Window, cx: &App) -> NativePresentation {
        let inputs = self
            .fields
            .iter()
            .map(|(key, field)| {
                let input = field.state.read(cx);
                let range = input.selected_range();
                let selection = if input.cursor() == range.start {
                    range.end..range.start
                } else {
                    range
                };
                (
                    key.clone(),
                    InputPresentation {
                        value: input.value().to_string(),
                        secure: field.secure,
                        selection,
                        focused: input.focus_handle(cx).is_focused(window),
                    },
                )
            })
            .collect();
        let mut editors = HashMap::new();
        let mut scrolls = HashMap::new();
        super::commands::walk_authored_paths(&self.root, &mut Vec::new(), &mut |node, path| {
            if let wire::Node::Scroll {
                direction,
                anchor_x,
                anchor_y,
                ..
            } = node
            {
                let offset = self
                    .lists
                    .get(path)
                    .map(|list| list.state.scroll_px_offset_for_scrollbar())
                    .or_else(|| self.scrolls.get(path).map(ScrollHandle::offset));
                if let Some(offset) = offset {
                    scrolls.insert(
                        path.clone(),
                        ScrollPresentation {
                            direction: *direction,
                            anchors: (*anchor_x, *anchor_y),
                            offset,
                            rows: self
                                .lists
                                .get(path)
                                .map(|list| list.rows.iter().map(|row| row.key.clone()).collect()),
                        },
                    );
                }
            }
            if let wire::Node::Editor { document, .. } = node {
                let focused = self
                    .editors
                    .get(path)
                    .is_some_and(|editor| editor.view.is_focused(window, cx));
                if focused {
                    editors.insert(path.clone(), document.clone());
                }
            }
        });
        NativePresentation {
            images: self.images.clone(),
            vectors: self.vectors.clone(),
            focused_container: self.focus_targets.iter().find_map(|(key, (kind, handle))| {
                handle.is_focused(window).then(|| (key.clone(), *kind))
            }),
            inputs,
            editors,
            scrolls,
        }
    }

    /// A fresh tree that starts from the old one's `presentation`: the caches
    /// are taken now, the rest is claimed by each node's first render and
    /// dropped after it (`Render for ViewTree`).
    pub(crate) fn with_presentation(mut self, mut presentation: NativePresentation) -> Self {
        self.images = std::mem::take(&mut presentation.images);
        self.vectors = std::mem::take(&mut presentation.vectors);
        self.presentation = presentation;
        self
    }
}

#[cfg(test)]
mod guest_aria_tests;
