//! The phase-2 rules (`docs/ax.md` §1.3): what the door's phase-2 keys say
//! about one node (AX-101, 102, 106, 108 … 114, 116 … 118),
//! where a node sits through `parent` (AX-105, 119), whether the focus is
//! in the dialog that shows (AX-103, 104), and whether the arrows move it
//! through a composite's rows (AX-107).
use super::*;
use std::collections::HashMap;

/// A composite whose rows the arrows pick (AX-107).
const ARROWED: [&str; 5] = ["Tree", "ListBox", "Menu", "Grid", "EditableComboBox"];
/// A row of a composite.
const ITEM: [&str; 8] = [
    "ListBoxOption",
    "MenuItem",
    "MenuItemCheckBox",
    "MenuItemRadio",
    "TreeItem",
    "Row",
    "Cell",
    "GridCell",
];
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

    /// `node` is `ancestor` or sits inside it.
    fn within(&self, node: &'a AxNode, ancestor: &str) -> bool {
        node.id == ancestor || self.ancestors(node).any(|above| above.id == ancestor)
    }
}

/// A focused node of `nodes` is `id` or sits inside it.
fn focus_within(nodes: &[AxNode], snapshot: &Snapshot<'_>, id: &str) -> bool {
    nodes
        .iter()
        .any(|node| has(node, "focused") && snapshot.within(node, id))
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

/// A label that says work is in flight: "Creating…", "Loading".
fn in_flight(name: &str) -> bool {
    let name = name.trim();
    let first = name.split_whitespace().next().unwrap_or_default();
    let first = first.trim_end_matches(['…', '.']);
    name.starts_with("Loading") || (first.ends_with("ing") && name.ends_with(['…', '.']))
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
    if node::CONTROL.contains(&role) && in_flight(&node.name) {
        tally.check("AX-110", node, has(node, "busy"), || {
            format!("{:?} says work is in flight and is not busy", node.name)
        });
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

/// AX-107's own half on one snapshot: focus on a composite that has rows
/// is on one of them, not on the composite.
pub(super) fn snapshot_rules(nodes: &[AxNode], snapshot: &Snapshot<'_>, tally: &mut Tally) {
    for node in nodes.iter().filter(|node| has(node, "focused")) {
        let role = node.role.as_str();
        if ARROWED.contains(&role) && rows(nodes, snapshot, node) > 0 {
            tally.check("AX-107", node, false, || {
                format!("the focus sits on the {role}, and no row is active")
            });
        }
    }
}

/// How many rows of a composite `nodes` has inside `composite`.
fn rows(nodes: &[AxNode], snapshot: &Snapshot<'_>, composite: &AxNode) -> usize {
    nodes
        .iter()
        .filter(|node| ITEM.contains(&node.role.as_str()) && snapshot.within(node, &composite.id))
        .count()
}

/// The composite an arrow press moves in `nodes` (AX-107): the nearest
/// one at or above the focus with two rows or more.
pub(super) fn arrowed(nodes: &[AxNode]) -> Option<&AxNode> {
    let snapshot = Snapshot::of(nodes);
    let focus = nodes.iter().find(|node| has(node, "focused"))?;
    std::iter::once(focus)
        .chain(snapshot.ancestors(focus))
        .find(|node| ARROWED.contains(&node.role.as_str()) && rows(nodes, &snapshot, node) > 1)
}

/// AX-103, AX-104 and AX-107's arrows. A dialog that shows has the focus
/// as the state opens (AX-104); a non-modal one may let the walk out. One
/// the Tab walk never leaves is modal to the keyboard, and says so
/// (AX-103). Each arrow press of the probe moves the active row (AX-107).
pub(super) fn walk_rules(reading: &Reading, tally: &mut Tally) {
    let snapshots = &reading.snapshots;
    let Some(first) = snapshots.first() else {
        return;
    };
    let opened = Snapshot::of(first);
    for dialog in first.iter().filter(|node| node.role == "Dialog") {
        tally.check(
            "AX-104",
            dialog,
            focus_within(first, &opened, &dialog.id),
            || "the dialog shows and the focus is outside it".to_owned(),
        );
        let held = snapshots.len() > 1
            && snapshots
                .iter()
                .all(|nodes| focus_within(nodes, &Snapshot::of(nodes), &dialog.id));
        if held {
            tally.check("AX-103", dialog, dialog.more.modal, || {
                "Tab never leaves the dialog, and it is not modal".to_owned()
            });
        }
    }
    for Arrows { composite, focused } in &reading.arrows {
        let pass = focused[0] != focused[1] && focused[1] != focused[2];
        tally.check("AX-107", composite, pass, || {
            format!("down then up leaves the active row at {focused:?}")
        });
    }
}
