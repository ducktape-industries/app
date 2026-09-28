//! What a wire node is to assistive technology. `accessible` maps a node to
//! an `Accessible` (role, name, value, states); `announce` writes one onto
//! the element a renderer built; `guest_aria`, the one aria mapper, writes
//! the guest's own `Interactivity.aria` for the nodes that carry it
//! (Container, Image, Svg, UniformList). `ViewTree::presentation` also lives
//! here for now, though it is not accessibility: it is the native state
//! carried across a guest generation (see its doc).
use super::*;

/// What one wire node is to assistive technology: the role it plays, the
/// name it is called, and the value and states it reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Accessible {
    pub role: Option<gpui_kit::Role>,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Text a field holds, or a text's own words. Never a secure field's.
    pub value: Option<String>,
    /// A heading's level, 1 to 6.
    pub level: Option<usize>,
    /// How a change to text that is not focused is announced.
    pub live: Option<gpui_kit::accesskit::Live>,
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
fn descendant_text(node: &wire::Node) -> Option<String> {
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
/// the guest's own `Interactivity.aria` through `guest_aria` instead, which
/// takes only a picture's default role and name from the Image/Svg arm
/// below; RichText has none. The node's accessibility id is its wire key
/// under the module's view, the element id each variant is already built
/// with.
pub(crate) fn accessible(node: &wire::Node) -> Accessible {
    use gpui_kit::Role;
    use wire::Node;
    let labelled = |role: Role, label: &Option<String>| Accessible {
        role: Some(role),
        name: label.as_deref().and_then(named),
        ..Default::default()
    };
    match node {
        // the words are the value too, as gpui's own `Text` has them: the
        // adapters name a Label by its value
        Node::Text(view_wire::TextNode { content, .. }) => Accessible {
            role: Some(Role::Label),
            name: named(content),
            value: named(content),
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
    let Accessible {
        role,
        name,
        description,
        value,
        level,
        // a view says `Aria.live` on a roled container (`guest_aria`)
        live: _,
        disabled,
    } = accessible;
    crate::a11y::aria(element, |mut node| {
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
        if disabled {
            node = node.aria_disabled(true);
        }
        node
    })
}

impl ViewTree {
    /// The one aria mapper: `node`'s `Interactivity` as assistive
    /// technology receives it, on the element a renderer built. Role,
    /// focusable, the author id, every `aria.*` setter, then the guest focus
    /// handle (made once per id, kept in `guest_focus_targets`) and the rest
    /// of the interactivity through `interactivity::apply`. `on_click` stays
    /// with the caller. A picture with no role takes `accessible`'s (a
    /// labelled one is an Image); a roled node with no label is named by the
    /// text it draws. Each setter is a field write gpui reads once at
    /// prepaint, so where the caller puts this among its own setters does
    /// not matter.
    pub(super) fn guest_aria<E: gpui_kit::StatefulInteractiveElement>(
        &mut self,
        mut element: E,
        node: &wire::Node,
        interactivity: &wire::Interactivity,
        cx: &mut Context<Self>,
    ) -> E {
        let aria = &interactivity.aria;
        let own = accessible(node);
        let role = interactivity.role.or(own.role);
        if let Some(role) = role {
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
        } else if let Some(name) = own.name {
            element = element.aria_label(name);
        } else if role.is_some()
            && let Some(text) = descendant_text(node)
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
        // gpui panics (debug) when the node claiming it is the focused one
        if aria.active_descendant && !interactivity.focusable {
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
    /// container or editor, and the decoded image and SVG caches. Plain data
    /// only: no native entity, callback, handler id or IME preedit crosses a
    /// generation, since the new guest's handler ids mean different things.
    /// Document selection remains guest-owned.
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
