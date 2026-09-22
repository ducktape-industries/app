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

/// The first text a node's descendants carry, depth first: a button or a
/// clickable drawn with a text child but no explicit label is named by it,
/// so nothing in the tree is announced with an empty name while its label
/// sits one level down as a twin node.
pub(crate) fn descendant_text(node: &wire::Node) -> Option<String> {
    if let wire::Node::Text(view_wire::TextNode { content, .. }) = node {
        return named(content);
    }
    node.children().iter().find_map(descendant_text)
}

/// The ONE mapping from a wire node to what assistive technology hears. The
/// presenter builds every variant with it, so a view never names its own
/// controls' roles: it says `label`, and the role follows from the variant.
/// The node's accessibility id is its wire key under the module's view, the
/// element id each variant is already built with.
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
    use crate::a11y as ui;
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
    let element = ui::aria(element, |mut node| {
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
    ui::disabled(element, disabled)
}

impl ViewTree {
    /// Copied presentation only: no native entity, callback, handler id, or IME
    /// preedit crosses a guest generation. Document selection remains guest-owned.
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

    pub(crate) fn with_presentation(mut self, mut presentation: NativePresentation) -> Self {
        self.images = std::mem::take(&mut presentation.images);
        self.vectors = std::mem::take(&mut presentation.vectors);
        self.presentation = presentation;
        self
    }
}
