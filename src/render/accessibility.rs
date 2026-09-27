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
/// clickable drawn with text children but no explicit label is named by
/// them (`#` + `general` is "# general", not "#"), so nothing in the
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

/// The ONE mapping from a wire node to what assistive technology hears. The
/// presenter builds every variant with it, so a view never names its own
/// controls' roles: it says `label`, and the role follows from the variant.
/// The node's accessibility id is its wire key under the module's view, the
/// element id each variant is already built with.
pub(crate) fn accessible(node: &wire::Node) -> Accessible {
    use gpui_kit::Role;
    use wire::Node;
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
        // only an open named overlay is a dialog; a closed or unnamed one is layout
        Node::Overlay {
            label, children, ..
        } if named_overlay(label, children) => labelled(Role::Dialog, label),
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
        _ => Accessible::default(),
    }
}

/// Puts `accessible` on an element the presenter built. A kit widget draws
/// its own role, name and value over these; the states it does not report
/// itself (disabled, a description, a level) are the ones this adds.
pub(crate) fn announce<E: gpui_kit::InteractiveElement>(element: E, accessible: Accessible) -> E {
    use crate::a11y as ui;
    let Accessible {
        role,
        name,
        description,
        value,
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
        super::commands::walk_authored_paths(&self.root, &mut Vec::new(), &mut |node, path| {
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
        }
    }

    pub(crate) fn with_presentation(mut self, mut presentation: NativePresentation) -> Self {
        self.images = std::mem::take(&mut presentation.images);
        self.vectors = std::mem::take(&mut presentation.vectors);
        self.presentation = presentation;
        self
    }
}
