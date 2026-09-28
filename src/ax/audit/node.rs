//! The per-node rules of phase 1 (AX-001 … AX-017): what one node, and
//! its siblings for AX-016, must be.
use super::*;

const TEXT_INPUT: [&str; 8] = [
    "TextInput",
    "MultilineTextInput",
    "SearchInput",
    "EmailInput",
    "NumberInput",
    "PasswordInput",
    "PhoneNumberInput",
    "UrlInput",
];
const TOGGLE: [&str; 5] = [
    "CheckBox",
    "Switch",
    "RadioButton",
    "MenuItemCheckBox",
    "MenuItemRadio",
];
const CONTROL: [&str; 10] = [
    "Button",
    "Link",
    "Tab",
    "CheckBox",
    "Switch",
    "RadioButton",
    "MenuItem",
    "MenuItemCheckBox",
    "MenuItemRadio",
    "ListBoxOption",
];
const NAMED_CONTAINER: [&str; 14] = [
    "Dialog",
    "AlertDialog",
    "Menu",
    "MenuBar",
    "TabList",
    "RadioGroup",
    "Tree",
    "ListBox",
    "Grid",
    "Table",
    "Toolbar",
    "Log",
    "Feed",
    "Document",
];

/// AX-001 … AX-017 on one node; AX-016 looks at its siblings in `nodes`.
pub(super) fn node_rules(node: &AxNode, nodes: &[AxNode], tally: &mut Tally) {
    let (press, focus) = (offers(node, "press"), offers(node, "focus"));
    let role = node.role.as_str();
    let text_input = TEXT_INPUT.contains(&role);
    if !node.actions.is_empty() {
        tally.check(
            "AX-001",
            node,
            !matches!(role, "Unknown" | "GenericContainer"),
            || format!("actionable node has role {role}"),
        );
    }
    if press || focus {
        tally.check("AX-002", node, named(node), || {
            "press/focus node has no name".to_owned()
        });
    }
    if text_input {
        tally.check("AX-003", node, named(node), || {
            "text input has no name".to_owned()
        });
        if role != "PasswordInput" {
            tally.check("AX-004", node, node.value.is_some(), || {
                "text input has no value key".to_owned()
            });
        }
        if let Some(description) = &node.description {
            tally.check("AX-005", node, node.name != *description, || {
                "text input's name is its description (placeholder)".to_owned()
            });
        }
    }
    if named(node) && (press || focus) {
        let trimmed = node.name.trim();
        let pass =
            trimmed.chars().any(char::is_alphanumeric) && !trimmed.eq_ignore_ascii_case(role);
        tally.check("AX-006", node, pass, || {
            format!("name {trimmed:?} says nothing beyond the role")
        });
    }
    if role == "Image" {
        tally.check("AX-007", node, named(node), || {
            "image has no name".to_owned()
        });
    }
    if NAMED_CONTAINER.contains(&role) {
        tally.check("AX-008", node, named(node), || {
            format!("{role} has no name")
        });
    }
    if TOGGLE.contains(&role) {
        let marks = ["checked", "unchecked", "mixed"]
            .iter()
            .filter(|state| has(node, state))
            .count();
        tally.check("AX-009", node, marks == 1, || {
            format!("{role} has {marks} of checked/unchecked/mixed")
        });
    }
    if role == "ComboBox" {
        let marks = ["expanded", "collapsed"]
            .iter()
            .filter(|state| has(node, state))
            .count();
        tally.check("AX-010", node, marks == 1, || {
            format!("ComboBox has {marks} of expanded/collapsed")
        });
    }
    if CONTROL.contains(&role) && !press {
        tally.check("AX-011", node, has(node, "disabled"), || {
            format!("{role} offers no press and is not disabled")
        });
    }
    if press {
        tally.check("AX-012", node, focus, || {
            "press without focus: a keyboard never reaches it".to_owned()
        });
    }
    if matches!(role, "Status" | "Alert") {
        tally.check("AX-013", node, named(node), || {
            format!("{role} has no name")
        });
    }
    if matches!(role, "Heading" | "Label") {
        tally.check("AX-014", node, named(node), || {
            format!("{role} has no name")
        });
    }
    let suffixed = node.id.rsplit_once('~').is_some_and(|(_, digits)| {
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
    });
    tally.check("AX-015", node, !suffixed, || {
        "two elements resolved to one path".to_owned()
    });
    if press {
        let twin = nodes.iter().find(|other| {
            other.id != node.id
                && offers(other, "press")
                && other.scope == node.scope
                && other.role == node.role
                && other.name == node.name
        });
        tally.check("AX-016", node, twin.is_none(), || {
            format!("same role and name as {}", twin.map_or("", |twin| &twin.id))
        });
        if let Some([x0, y0, x1, y1]) = node.bounds {
            let (w, h) = (x1 - x0, y1 - y0);
            tally.check("AX-017", node, w >= 24 && h >= 24, || {
                format!("press target is {w}x{h} px")
            });
        }
    }
}
