//! The phase-2 rules (`docs/ax.md` §1.3): what the door's phase-2 keys say
//! about one node (AX-101, 102, 106, 108, 109, 111 … 114, 116 … 118), and
//! where a node sits through `parent` (AX-105, 119).
use super::*;
use std::collections::HashMap;

/// A node whose press is the whole of it: nothing pressable inside (AX-119).
const PRESSED_WHOLE: [&str; 9] = [
    "Button",
    "Link",
    "Tab",
    "MenuItem",
    "CheckBox",
    "Switch",
    "RadioButton",
    "MenuItemCheckBox",
    "MenuItemRadio",
];

/// One snapshot's nodes by door id, to follow `parent`.
pub(super) struct Snapshot<'a>(HashMap<&'a str, &'a AxNode>);

impl<'a> Snapshot<'a> {
    pub(super) fn of(nodes: &'a [AxNode]) -> Self {
        Self(nodes.iter().map(|node| (node.id.as_str(), node)).collect())
    }

    /// `node`'s ancestors the snapshot has, nearest first.
    pub(super) fn ancestors(&self, node: &'a AxNode) -> impl Iterator<Item = &'a AxNode> + '_ {
        let up = |node: &AxNode| {
            node.more
                .parent
                .as_deref()
                .and_then(|id| self.0.get(id).copied())
        };
        std::iter::successors(up(node), move |node| up(node))
    }
}

/// The container roles `role` belongs in (AX-105), when it has any.
fn home(role: &str) -> Option<&'static [&'static str]> {
    Some(match role {
        "MenuItem" | "MenuItemCheckBox" | "MenuItemRadio" => &["Menu", "MenuBar"],
        "Tab" => &["TabList"],
        "RadioButton" => &["RadioGroup"],
        "ListBoxOption" => &["ListBox"],
        "TreeItem" => &["Tree"],
        "Row" | "Cell" => &["Table", "Grid"],
        _ => return None,
    })
}

/// The phase-2 rules on one node of `nodes`, `snapshot` their index.
pub(super) fn node_rules(
    node: &AxNode,
    nodes: &[AxNode],
    snapshot: &Snapshot<'_>,
    reading: &Reading,
    tally: &mut Tally,
) {
    let role = node.role.as_str();
    let more = &node.more;
    let press = offers(node, "press");
    if matches!(role, "Tab" | "TreeItem" | "ListBoxOption") {
        let marks = ["selected", "unselected"]
            .iter()
            .filter(|state| has(node, state))
            .count();
        tally.check("AX-101", node, marks == 1, || {
            format!("{role} has {marks} of selected/unselected")
        });
    }
    if let Some(live) = match role {
        "Status" => Some("polite"),
        "Alert" => Some("assertive"),
        _ => None,
    } {
        let pass = more.live == Some(live) && node.value.is_some();
        tally.check("AX-102", node, pass, || {
            format!(
                "{role} is live {:?} with value {:?}; wants {live} and a value",
                more.live, node.value
            )
        });
    }
    if let Some(homes) = home(role) {
        let pass = snapshot
            .ancestors(node)
            .any(|above| homes.contains(&above.role.as_str()));
        tally.check("AX-105", node, pass, || {
            format!("{role} is in no {}", homes.join(" or "))
        });
    }
    if role == "Heading" {
        let pass = more.level.is_some_and(|level| (1..=6).contains(&level));
        tally.check("AX-106", node, pass, || {
            format!("Heading has level {:?}", more.level)
        });
    }
    if node::TEXT_INPUT.contains(&role) {
        if more.invalid.is_some() {
            let described = node
                .description
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty());
            tally.check("AX-108", node, described, || {
                "an invalid field does not say why".to_owned()
            });
            if node.value.as_deref() == Some("") {
                tally.check("AX-109", node, more.required, || {
                    "refused empty, and not required".to_owned()
                });
            }
        }
        if let Some(placeholder) = &more.placeholder {
            tally.check("AX-111", node, node.name != *placeholder, || {
                "its name is its placeholder".to_owned()
            });
        }
    }
    let positioned = more.position_in_set.is_some() || more.size_of_set.is_some();
    let siblings = || {
        nodes.iter().filter(|other| {
            other.scope == node.scope && other.more.parent == more.parent && other.role == node.role
        })
    };
    if !shell(node) && (positioned || siblings().any(|other| other.more.size_of_set.is_some())) {
        let pass = match (more.position_in_set, more.size_of_set) {
            (Some(position), Some(size)) => (1..=size).contains(&position),
            _ => false,
        };
        tally.check("AX-112", node, pass, || {
            format!(
                "row says {:?} of {:?} in its set",
                more.position_in_set, more.size_of_set
            )
        });
    }
    if role == "Button" && (has(node, "expanded") || has(node, "collapsed")) {
        tally.check("AX-113", node, more.has_popup.is_some(), || {
            "opens something and has no has_popup".to_owned()
        });
    }
    if press && let Some((_, chord)) = reading.chords.iter().find(|(name, _)| *name == node.name) {
        let pass = more.keyboard_shortcut.as_deref() == Some(chord.as_str());
        tally.check("AX-114", node, pass, || {
            format!(
                "Help lists {chord}; it reports {:?}",
                more.keyboard_shortcut
            )
        });
    }
    let stepped = offers(node, "increment") || offers(node, "decrement");
    let folds = offers(node, "expand") || offers(node, "collapse");
    if stepped || folds {
        let pass = (!stepped || node.value.is_some())
            && (!folds || has(node, "expanded") || has(node, "collapsed"));
        tally.check("AX-116", node, pass, || {
            "offers a step without a value, or a fold without its state".to_owned()
        });
    }
    if role == "Link" {
        let pass = press && node.name.chars().any(char::is_alphanumeric);
        tally.check("AX-117", node, pass, || {
            "a link offers no press, or no words".to_owned()
        });
    }
    if role == "Splitter" {
        let pass = named(node) && offers(node, "focus");
        tally.check("AX-118", node, pass, || {
            "a splitter is unnamed or the keyboard never reaches it".to_owned()
        });
    }
    if press {
        let host = snapshot
            .ancestors(node)
            .find(|above| PRESSED_WHOLE.contains(&above.role.as_str()));
        tally.check("AX-119", node, host.is_none(), || {
            format!("pressable inside {}", host.map_or("", |host| &host.id))
        });
    }
}
