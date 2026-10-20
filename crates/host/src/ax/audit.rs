//! The audit (`docs/ax.md` §1.2, §1.3, §2): the rule table applied to what
//! the door saw. Per-node rules run over every node of every snapshot; walk
//! rules over the sequence a Tab walk yields. [`audit`] is pure: the
//! [`Observer`] is the one thing here that touches a window, and it only
//! takes the snapshots and presses the keys. What the door cannot see it
//! is told:
//! whether a modal scopes the snapshot, whether `escape` was bound at each
//! step, and — the caller's word, never a guess from the tree — whether the
//! shell screen is a launcher screen (AX-018), and which controls the
//! app's Help lists with a chord (AX-114).
use super::actions::{press_keys, shortcuts};
use super::{AxNode, tree};
use gpui_kit::accesskit::NodeId;
use gpui_kit::{App, FocusHandle, GlobalElementId, Window};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(test)]
pub mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
}

/// One row of the table, as `docs/ax.md` §7 lists it.
#[derive(Debug, Serialize)]
pub struct Rule {
    pub id: &'static str,
    pub severity: Severity,
    pub predicate: &'static str,
}

const fn rule(id: &'static str, severity: Severity, predicate: &'static str) -> Rule {
    Rule {
        id,
        severity,
        predicate,
    }
}

use Severity::{Error, Warn};

pub const RULES: [Rule; 43] = [
    rule(
        "AX-001",
        Error,
        "An actionable node's role is neither Unknown nor GenericContainer.",
    ),
    rule(
        "AX-002",
        Error,
        "A node whose actions contain press or focus is named.",
    ),
    rule(
        "AX-003",
        Error,
        "A node with a text-input role is named; description does not count.",
    ),
    rule(
        "AX-004",
        Error,
        "A node with a text-input role other than PasswordInput has a value key.",
    ),
    rule(
        "AX-005",
        Warn,
        "A text-input node with a description has name != description.",
    ),
    rule(
        "AX-006",
        Error,
        "A named press/focus node's name has an alphanumeric and is not its role.",
    ),
    rule("AX-007", Error, "An Image is named."),
    rule("AX-008", Error, "A named-container role is named."),
    rule(
        "AX-009",
        Error,
        "A toggle role has exactly one of checked, unchecked, mixed.",
    ),
    rule(
        "AX-010",
        Error,
        "A ComboBox has exactly one of expanded, collapsed.",
    ),
    rule("AX-011", Error, "A control role without press is disabled."),
    rule(
        "AX-012",
        Error,
        "A node with press also offers focus, or holds the keys now, or sits in a composite that does, or (a node no element draws) under a node that does, or (shell) in a box without the keys whose chord hands them to it.",
    ),
    rule("AX-013", Error, "A Status or Alert is named."),
    rule("AX-014", Error, "A Heading or Label is named."),
    rule("AX-015", Error, "No id ends in ~ followed by digits."),
    rule(
        "AX-016",
        Warn,
        "No two press nodes in one scope share role and name.",
    ),
    rule(
        "AX-017",
        Warn,
        "A press node's bounds are at least 24 by 24 logical px.",
    ),
    rule(
        "AX-018",
        Error,
        "A launcher screen (no modal) has exactly one shell Heading.",
    ),
    rule(
        "AX-020",
        Error,
        "After every tab press exactly one node is focused.",
    ),
    rule(
        "AX-021",
        Error,
        "Every node offering focus in the first snapshot is focused once in the walk, or is the nearest node offering focus above a focused node that offers none (the keys are in it), or had the keys as a dialog or menu the walk closed opened.",
    ),
    rule(
        "AX-022",
        Warn,
        "Every tab press changes which id is focused.",
    ),
    rule(
        "AX-023",
        Error,
        "Under a modal, every snapshot of the walk has a focused node.",
    ),
    rule(
        "AX-024",
        Error,
        "While a shell Dialog shows, escape is bound.",
    ),
    rule(
        "AX-025",
        Error,
        "While a Dialog shows, some node is focused.",
    ),
    rule(
        "AX-101",
        Error,
        "A Tab, TreeItem or ListBoxOption has exactly one of selected, unselected.",
    ),
    rule(
        "AX-102",
        Error,
        "A Status is live polite, an Alert live assertive, and both have a value.",
    ),
    rule(
        "AX-103",
        Error,
        "A Dialog the Tab walk never leaves is modal.",
    ),
    rule("AX-104", Error, "A Dialog in a snapshot holds the focus."),
    rule(
        "AX-105",
        Error,
        "A menu item, tab, radio button, option, tree item, row or cell is in its container.",
    ),
    rule("AX-106", Error, "A Heading has a level in 1..=6."),
    rule(
        "AX-107",
        Error,
        "Focus in a composite is on a row, and its arrows move it, there and back.",
    ),
    rule(
        "AX-108",
        Error,
        "An invalid text field has a description that says why.",
    ),
    rule(
        "AX-109",
        Warn,
        "A text field refused empty (invalid, with no value) is required.",
    ),
    rule(
        "AX-110",
        Error,
        "A control whose name says work is in flight is busy.",
    ),
    rule(
        "AX-111",
        Warn,
        "A text field's name is not its placeholder.",
    ),
    rule(
        "AX-112",
        Error,
        "A row of a view set that says where its rows are has position and size.",
    ),
    rule(
        "AX-113",
        Warn,
        "A Button that is expanded or collapsed reports has_popup.",
    ),
    rule(
        "AX-114",
        Warn,
        "A control Help lists with a chord reports it as keyboard_shortcut.",
    ),
    rule(
        "AX-116",
        Error,
        "A step action comes with a value, a fold action with its state.",
    ),
    rule(
        "AX-117",
        Error,
        "A Link offers press and is named by words.",
    ),
    rule("AX-118", Error, "A Splitter is named and offers focus."),
    rule(
        "AX-119",
        Error,
        "Nothing pressable sits inside a button, link, tab, menu item or toggle.",
    ),
    rule(
        "AX-124",
        Error,
        "No element is refused for sharing its accessibility id with an earlier one.",
    ),
];

fn severity(rule: &str) -> Severity {
    RULES
        .iter()
        .find(|row| row.id == rule)
        .unwrap_or_else(|| panic!("{rule} is not in the rule table"))
        .severity
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Violation {
    pub rule: &'static str,
    pub severity: Severity,
    /// the node's door id; a walk step's `step N` (N presses in); a refused
    /// element's `<kept door id> <- <its GlobalElementId>` (AX-124)
    pub id: String,
    pub role: String,
    pub name: String,
    pub message: String,
}

/// `docs/ax.md` §2: `actionable` = actionable nodes with no error-severity
/// violation over actionable nodes; `rule` = 1 − failed over applicable,
/// every rule counted. Both 1 over an empty denominator.
#[derive(Debug, Serialize, PartialEq)]
pub struct Coverage {
    pub actionable: f64,
    pub rule: f64,
}

#[derive(Debug, Serialize)]
pub struct Report {
    /// distinct ids over every snapshot
    pub nodes: usize,
    pub actionable: usize,
    pub modal: bool,
    /// walk presses taken (`tab`, or `escape tab` after a stay); 0 when
    /// the walk was not asked for
    pub presses: usize,
    /// AX-018 was evaluated: the caller said the screen is a launcher screen
    pub launcher: bool,
    pub violations: Vec<Violation>,
    /// per rule: distinct nodes (or steps) its antecedent selected
    pub applicable: BTreeMap<&'static str, usize>,
    pub coverage: Coverage,
}

/// What one screen state showed the door: the first snapshot, then one per
/// tab press; `escape`, one per snapshot, whether `escape` was bound then;
/// `outside`, one per snapshot, whether its focus was outside `keep`;
/// `arrows`, the arrow probe of each composite the focus sat in; and,
/// the caller's word, `chords`: `(control name, chord)` for each control
/// the app's Help lists with a chord (AX-114); and `refused`, what the fork
/// dropped from the tree on the frame of each snapshot (AX-124).
#[derive(Debug, Default)]
pub struct Reading {
    pub snapshots: Vec<Vec<AxNode>>,
    pub escape: Vec<bool>,
    pub modal: bool,
    pub arrows: Vec<Arrows>,
    pub chords: Vec<(String, String)>,
    pub refused: Vec<Refused>,
    /// one per snapshot: the focus is on a node `keep` dropped, and on no
    /// node it kept (a `view=` audit's walk on a shell stop), so the step
    /// is not the view's to judge
    pub outside: Vec<bool>,
}

/// An element the fork left out of the tree because an earlier one had its
/// accessibility id (AX-124), as one snapshot's frame refused it: the door
/// id of the node that kept it (its `GlobalElementId`, else its `NodeId`,
/// where the snapshot does not show it), the refused element's
/// `GlobalElementId`, and which of the identical refusals on that frame
/// this is (two siblings given one element id refuse twice with one path).
#[derive(Debug)]
pub struct Refused {
    pub kept: String,
    pub element: String,
    pub nth: usize,
}

fn note_refused(
    into: &mut Vec<Refused>,
    list: &[(NodeId, GlobalElementId)],
    name: &str,
    keep: impl Fn(&str) -> bool,
    kept_name: impl Fn(NodeId) -> String,
) {
    for (n, (id, element)) in list.iter().enumerate() {
        if !keep(&tree::scope_of(name, element)) {
            continue;
        }
        into.push(Refused {
            kept: kept_name(*id),
            element: format!("GlobalElementId({:?})", &**element),
            nth: list[..n]
                .iter()
                .filter(|before| **before == list[n])
                .count(),
        });
    }
}

/// An arrow probe (AX-107): the composite the focus sat in, the pairs it
/// tried (an arrow and the arrow back; an arrow of each but the last moved
/// nothing), and the focused id before, after the last pair's arrow, and
/// after its arrow back, with the row of a grid each sat in (else the id
/// again).
#[derive(Debug)]
pub struct Arrows {
    pub composite: AxNode,
    pub keys: Vec<[&'static str; 2]>,
    pub focused: [Option<String>; 3],
    pub rows: [Option<String>; 3],
}

impl Arrows {
    fn spots(&self) -> &[Option<String>; 3] {
        match self.composite.role == "Grid" && matches!(self.keys.last(), Some(["down" | "up", _]))
        {
            true => &self.rows,
            false => &self.focused,
        }
    }
}

const TEXT_INPUT: [&str; 9] = [
    "TextInput",
    "EditableComboBox",
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

const ARROWED: [&str; 7] = [
    "Tree",
    "ListBox",
    "Menu",
    "Grid",
    "EditableComboBox",
    "TabList",
    "RadioGroup",
];
const ITEM: [&str; 10] = [
    "Tab",
    "RadioButton",
    "ListBoxOption",
    "MenuItem",
    "MenuItemCheckBox",
    "MenuItemRadio",
    "TreeItem",
    "Row",
    "Cell",
    "GridCell",
];
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

fn named(node: &AxNode) -> bool {
    !node.name.trim().is_empty()
}

fn offers(node: &AxNode, action: &str) -> bool {
    node.actions.contains(&action)
}

fn has(node: &AxNode, state: &str) -> bool {
    node.state.contains(&state)
}

fn shell(node: &AxNode) -> bool {
    !node.scope.contains('/')
}

fn focused(nodes: &[AxNode]) -> Vec<&str> {
    nodes
        .iter()
        .filter(|node| has(node, "focused"))
        .map(|node| node.id.as_str())
        .collect()
}

struct Snapshot<'a>(HashMap<&'a str, &'a AxNode>);

impl<'a> Snapshot<'a> {
    fn of(nodes: &'a [AxNode]) -> Self {
        Self(nodes.iter().map(|node| (node.id.as_str(), node)).collect())
    }

    fn ancestors(&self, node: &'a AxNode) -> impl Iterator<Item = &'a AxNode> + '_ {
        let up = |node: &AxNode| {
            node.more
                .parent
                .as_deref()
                .and_then(|id| self.0.get(id).copied())
        };
        std::iter::successors(up(node), move |node| up(node))
    }

    fn within(&self, node: &'a AxNode, ancestor: &str) -> bool {
        node.id == ancestor || self.ancestors(node).any(|above| above.id == ancestor)
    }
}

fn focus_within(nodes: &[AxNode], snapshot: &Snapshot<'_>, id: &str) -> bool {
    nodes
        .iter()
        .any(|node| has(node, "focused") && snapshot.within(node, id))
}

fn required_container_roles(role: &str) -> Option<&'static [&'static str]> {
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

fn in_flight(name: &str) -> bool {
    let name = name.trim();
    let first = name.split_whitespace().next().unwrap_or_default();
    let first = first.trim_end_matches(['…', '.']);
    name.starts_with("Loading") || (first.ends_with("ing") && name.ends_with(['…', '.']))
}

#[derive(Default)]
struct Tally {
    applicable: BTreeSet<(&'static str, String)>,
    failed: BTreeMap<(&'static str, String), Violation>,
}

impl Tally {
    fn check(
        &mut self,
        rule: &'static str,
        node: &AxNode,
        pass: bool,
        message: impl Fn() -> String,
    ) {
        self.step(rule, &node.id, &node.role, &node.name, pass, message);
    }

    fn step(
        &mut self,
        rule: &'static str,
        id: &str,
        role: &str,
        name: &str,
        pass: bool,
        message: impl Fn() -> String,
    ) {
        self.applicable.insert((rule, id.to_owned()));
        if !pass {
            self.failed
                .entry((rule, id.to_owned()))
                .or_insert_with(|| Violation {
                    rule,
                    severity: severity(rule),
                    id: id.to_owned(),
                    role: role.to_owned(),
                    name: name.to_owned(),
                    message: message(),
                });
        }
    }
}

fn node_rules(
    node: &AxNode,
    nodes: &[AxNode],
    snapshot: &Snapshot<'_>,
    reading: &Reading,
    tally: &mut Tally,
) {
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
        let composite = snapshot
            .ancestors(node)
            .any(|above| tree::COMPOSITES.contains(&above.role.as_str()) && offers(above, "focus"));
        let linked = node.synthetic && snapshot.ancestors(node).any(|above| offers(above, "focus"));
        let chord = shell(node)
            && snapshot.ancestors(node).any(|above| {
                above.more.keyboard_shortcut.is_some()
                    && !offers(above, "press")
                    && !focus_within(nodes, snapshot, &above.id)
            });
        let held = has(node, "focused");
        tally.check(
            "AX-012",
            node,
            focus || held || composite || linked || chord,
            || "press without focus: a keyboard never reaches it".to_owned(),
        );
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
    let more = &node.more;
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
    if let Some(homes) = required_container_roles(role) {
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
    if TEXT_INPUT.contains(&role) {
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
    if CONTROL.contains(&role) && in_flight(&node.name) {
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

fn refused_rules(reading: &Reading, tally: &mut Tally) {
    let mut seen = BTreeSet::new();
    for Refused { kept, element, nth } in &reading.refused {
        if !seen.insert((element, nth)) {
            continue;
        }
        let id = match nth {
            0 => format!("{kept} <- {element}"),
            nth => format!("{kept} <- {element} #{}", nth + 1),
        };
        tally.step("AX-124", &id, "", "", false, || {
            format!(
                "{element} was refused: it has the same element id as {kept}, which the tree kept. Give one of the two its own id, or the refused one is invisible to a screen reader"
            )
        });
    }
}

fn screen_rules(nodes: &[AxNode], reading: &Reading, launcher: bool, tally: &mut Tally) {
    if !launcher || reading.modal {
        return;
    }
    let headings = nodes
        .iter()
        .filter(|node| shell(node) && node.role == "Heading")
        .count();
    let window = nodes
        .first()
        .map_or("", |node| node.scope.split('/').next().unwrap_or_default());
    tally.step("AX-018", window, "", "", headings == 1, || {
        format!("launcher screen has {headings} shell headings")
    });
}

fn snapshot_rules(nodes: &[AxNode], snapshot: &Snapshot<'_>, tally: &mut Tally) {
    for node in nodes.iter().filter(|node| has(node, "focused")) {
        let role = node.role.as_str();
        if ARROWED.contains(&role) && enabled_arrow_targets(nodes, snapshot, node) > 0 {
            tally.check("AX-107", node, false, || {
                format!("the focus sits on the {role}, and no row is active")
            });
        }
    }
}

fn enabled_arrow_targets(nodes: &[AxNode], snapshot: &Snapshot<'_>, composite: &AxNode) -> usize {
    match composite.role.as_str() {
        "Grid" => cells(nodes, snapshot, composite).count(),
        _ => rows(nodes, snapshot, composite),
    }
}

fn rows(nodes: &[AxNode], snapshot: &Snapshot<'_>, composite: &AxNode) -> usize {
    nodes
        .iter()
        .filter(|node| {
            ITEM.contains(&node.role.as_str())
                && !has(node, "disabled")
                && snapshot.within(node, &composite.id)
        })
        .count()
}

fn cells<'a>(
    nodes: &'a [AxNode],
    snapshot: &Snapshot<'a>,
    grid: &AxNode,
) -> impl Iterator<Item = &'a AxNode> {
    nodes.iter().filter(move |node| {
        matches!(node.role.as_str(), "Cell" | "GridCell")
            && !has(node, "disabled")
            && snapshot.within(node, &grid.id)
    })
}

fn arrowed(nodes: &[AxNode]) -> Option<&AxNode> {
    let snapshot = Snapshot::of(nodes);
    let focus = nodes.iter().find(|node| has(node, "focused"))?;
    let up: Vec<_> = std::iter::once(focus)
        .chain(snapshot.ancestors(focus))
        .collect();
    let at = up.iter().position(|node| {
        ARROWED.contains(&node.role.as_str()) && enabled_arrow_targets(nodes, &snapshot, node) > 1
    })?;
    up[..at]
        .iter()
        .all(|node| !offers(node, "focus") || ITEM.contains(&node.role.as_str()))
        .then_some(up[at])
}

fn arrow_pairs(nodes: &[AxNode], composite: &AxNode) -> Vec<[&'static str; 2]> {
    const ROWS: [[&str; 2]; 2] = [["down", "up"], ["up", "down"]];
    const CELLS: [[&str; 2]; 2] = [["right", "left"], ["left", "right"]];
    if composite.role != "Grid" {
        let horizontal = composite
            .more
            .orientation
            .map_or(composite.role == "TabList", |way| way == "horizontal");
        return match horizontal {
            true => CELLS.to_vec(),
            false => ROWS.to_vec(),
        };
    }
    let snapshot = Snapshot::of(nodes);
    let mut rows: Vec<(&str, Vec<&str>)> = Vec::new();
    for cell in cells(nodes, &snapshot, composite) {
        let row = snapshot
            .ancestors(cell)
            .find(|row| row.role == "Row")
            .map_or("", |row| row.id.as_str());
        match rows.iter_mut().find(|(id, _)| *id == row) {
            Some((_, cells)) => cells.push(&cell.id),
            None => rows.push((row, vec![&cell.id])),
        }
    }
    if rows.len() == 1 {
        return CELLS.to_vec();
    }
    let focus = nodes.iter().find(|node| has(node, "focused"));
    let at = rows.iter().enumerate().find_map(|(r, (_, cells))| {
        cells
            .iter()
            .position(|id| focus.is_some_and(|focus| snapshot.within(focus, id)))
            .map(|i| (r, i))
    });
    let alone = at.is_some_and(|(r, i)| {
        let holds = |row: Option<usize>| {
            row.and_then(|row| rows.get(row))
                .is_some_and(|(_, cells)| cells.len() > i)
        };
        !holds(r.checked_sub(1)) && !holds(Some(r + 1))
    });
    match alone {
        true => [ROWS, CELLS].concat(),
        false => ROWS.to_vec(),
    }
}

fn spot(nodes: &[AxNode], composite: &AxNode) -> [Option<String>; 2] {
    let snapshot = Snapshot::of(nodes);
    let Some(focus) = nodes.iter().find(|node| has(node, "focused")) else {
        return [None, None];
    };
    let row = std::iter::once(focus)
        .chain(snapshot.ancestors(focus))
        .take_while(|node| node.id != composite.id)
        .find(|node| composite.role == "Grid" && node.role == "Row")
        .unwrap_or(focus);
    [Some(focus.id.clone()), Some(row.id.clone())]
}

fn holds(node: &AxNode) -> bool {
    has(node, "focused") || node.more.active_descendant.is_some()
}

fn walk_rules(reading: &Reading, tally: &mut Tally) {
    let snapshots = &reading.snapshots;
    let Some(first) = snapshots.first() else {
        return;
    };
    let step = |n: usize| format!("step {n}");
    let outside = |n: usize| reading.outside.get(n).copied().unwrap_or(false);
    for (n, nodes) in snapshots.iter().enumerate().skip(1) {
        if outside(n) {
            continue;
        }
        let now = focused(nodes);
        tally.step("AX-020", &step(n), "", "", now.len() == 1, || {
            format!("{} nodes focused after tab: {now:?}", now.len())
        });
        let moved = now != focused(&snapshots[n - 1]) || outside(n - 1);
        tally.step("AX-022", &step(n), "", "", moved, || {
            format!("tab left focus on {now:?}")
        });
    }
    let walked = snapshots.len() > 1;
    let opened = Snapshot::of(first);
    let closed = |id: &str| {
        snapshots[1..]
            .iter()
            .any(|nodes| nodes.iter().all(|other| other.id != id))
    };
    let mut through = std::collections::HashSet::new();
    for nodes in &snapshots[1..] {
        let snapshot = Snapshot::of(nodes);
        for node in nodes
            .iter()
            .filter(|node| has(node, "focused") && !offers(node, "focus"))
        {
            through.extend(
                snapshot
                    .ancestors(node)
                    .find(|above| offers(above, "focus"))
                    .map(|above| above.id.as_str()),
            );
        }
    }
    for node in first.iter().filter(|node| walked && offers(node, "focus")) {
        let reached = snapshots[1..].iter().any(|nodes| {
            nodes
                .iter()
                .any(|other| other.id == node.id && holds(other))
        }) || through.contains(node.id.as_str())
            || holds(node)
                && opened.ancestors(node).any(|above| {
                    matches!(above.role.as_str(), "Dialog" | "Menu") && closed(&above.id)
                });
        tally.check("AX-021", node, reached, || {
            "offers focus but the tab walk never reached it".to_owned()
        });
    }
    for (n, nodes) in snapshots.iter().enumerate() {
        let some = !focused(nodes).is_empty() || outside(n);
        if reading.modal && walked && !outside(n) {
            tally.step("AX-023", &step(n), "", "", some, || {
                "focus left the modal's subtree".to_owned()
            });
        }
        let dialog = nodes.iter().find(|node| node.role == "Dialog");
        if let Some(dialog) = dialog {
            tally.check("AX-025", dialog, some, || {
                "a dialog shows and nothing is focused".to_owned()
            });
            if let Some(escape) = reading.escape.get(n).filter(|_| shell(dialog)) {
                tally.check("AX-024", dialog, *escape, || {
                    "a shell dialog shows and escape is not bound".to_owned()
                });
            }
        }
    }
    for nodes in snapshots {
        let snapshot = Snapshot::of(nodes);
        for dialog in nodes.iter().filter(|node| node.role == "Dialog") {
            tally.check(
                "AX-104",
                dialog,
                focus_within(nodes, &snapshot, &dialog.id),
                || "the dialog shows and the focus is outside it".to_owned(),
            );
        }
    }
    for dialog in first.iter().filter(|node| node.role == "Dialog") {
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
    for arrows in &reading.arrows {
        let spots = arrows.spots();
        let pass = spots[1] != spots[0] && spots[2] == spots[0];
        tally.check("AX-107", &arrows.composite, pass, || {
            let keys = &arrows.keys;
            let stuck: String = keys[..keys.len() - 1]
                .iter()
                .map(|[there, _]| format!("{there} moved nothing; "))
                .collect();
            let [there, back] = keys[keys.len() - 1];
            format!("{stuck}{there} then {back} leaves the active row at {spots:?}")
        });
    }
}

/// The report of `docs/ax.md` §2 over `reading`. `launcher`: the shell screen
/// is a launcher screen (AX-018 applies); false leaves the rule unevaluated.
pub fn audit(reading: &Reading, launcher: bool) -> Report {
    let mut tally = Tally::default();
    for nodes in &reading.snapshots {
        let snapshot = Snapshot::of(nodes);
        for node in nodes {
            node_rules(node, nodes, &snapshot, reading, &mut tally);
        }
        screen_rules(nodes, reading, launcher, &mut tally);
        snapshot_rules(nodes, &snapshot, &mut tally);
    }
    walk_rules(reading, &mut tally);
    refused_rules(reading, &mut tally);
    let mut ids = BTreeSet::new();
    let mut actionable = BTreeSet::new();
    for node in reading.snapshots.iter().flatten() {
        ids.insert(node.id.as_str());
        if !node.actions.is_empty() {
            actionable.insert(node.id.as_str());
        }
    }
    let broken: BTreeSet<&str> = tally
        .failed
        .values()
        .filter(|violation| violation.severity == Error)
        .map(|violation| violation.id.as_str())
        .collect();
    let clean = actionable.iter().filter(|id| !broken.contains(*id)).count();
    let ratio = |num: usize, den: usize| {
        if den == 0 {
            1.
        } else {
            num as f64 / den as f64
        }
    };
    let mut applicable: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (rule, _) in &tally.applicable {
        *applicable.entry(rule).or_default() += 1;
    }
    Report {
        nodes: ids.len(),
        actionable: actionable.len(),
        modal: reading.modal,
        presses: reading.snapshots.len().saturating_sub(1),
        launcher,
        coverage: Coverage {
            actionable: ratio(clean, actionable.len()),
            rule: ratio(
                tally.applicable.len() - tally.failed.len(),
                tally.applicable.len(),
            ),
        },
        violations: tally.failed.into_values().collect(),
        applicable,
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Report {
    pub fn errors(&self) -> impl Iterator<Item = &Violation> {
        self.violations
            .iter()
            .filter(|violation| violation.severity == Error)
    }
}

impl Reading {
    fn take(&mut self, nodes: Vec<AxNode>, outside: bool, window: &Window, cx: &App) {
        self.escape.push(escape_bound(window, cx));
        self.outside.push(outside);
        self.snapshots.push(nodes);
    }
}

fn escape_bound(window: &Window, cx: &App) -> bool {
    serde_json::to_value(shortcuts(window, cx))
        .ok()
        .and_then(|list| {
            list.as_array()
                .map(|list| list.iter().any(|binding| binding["keys"] == "escape"))
        })
        .unwrap_or(false)
}

/// The reading of one window, `name` as its snapshots name it, `keep` the
/// scopes it is asked about: `snap` once, and with `walk`, `tab` through the
/// window's own key dispatch, `snap` after each, until the first press that
/// brings focus back to where the first press put it (a first press that
/// puts it nowhere ends the walk): once round the Tab cycle, stops the
/// first snapshot does not show (scrolled away, drawn since) included, up
/// to 4 (M + 1) presses, M the most nodes offering focus a snapshot has
/// shown so far (counted before `keep` filters). Those nodes are no floor:
/// a composite and its chosen row both offer focus and are one stop.
/// A Tab that leaves the focus where it was (a view editor keeps Tab for
/// its indent) is followed by `escape tab`, the way out Help gives (owner,
/// 2026-09-28), once until the focus moves, and never under a modal:
/// there `escape` closes the modal (Spotlight, whose one Tab stop Tab
/// comes back to, would close mid-walk). A stay the walk leaves so is not
/// the focus coming back round. Where the focus starts, and after each
/// press, when it sits in a composite with two rows or more (a grid, two
/// cells) that has the keys and was not probed yet, the arrow probe: an arrow and the arrow
/// back, `snap` after each ([`Arrows`], [`arrow_pairs`]); an arrow that
/// moved nothing is followed by the next pair's instead. Focus goes back
/// where it was — nowhere included.
///
/// All in the caller's one update: a key a view hears as an event the
/// tree emits reaches it only once that update ends, so this is for tests
/// of native windows. `GET /audit` drives the [`Observer`] a key per
/// update.
#[cfg(any(test, feature = "test-support"))]
pub fn observe(
    window: &mut Window,
    cx: &mut App,
    name: &str,
    walk: bool,
    keep: impl Fn(&str) -> bool,
    snap: impl FnMut(&mut Window, &mut App) -> Vec<AxNode>,
) -> Reading {
    let mut observer = Observer::open(window, cx, name, walk, keep, snap);
    while observer.press(window, cx) {
        observer.read(window, cx);
    }
    observer.finish(window, cx)
}

/// The reading of one window (`observe` above), a key at a time:
/// [`Observer::open`], then [`Observer::press`] and [`Observer::read`]
/// until `press` says the walk is done, then [`Observer::finish`], each in
/// any update the caller likes.
pub struct Observer<K, S> {
    name: String,
    walk: bool,
    keep: K,
    snap: S,
    reading: Reading,
    refused: Vec<Refused>,
    before: Option<FocusHandle>,
    probe: Option<Probe>,
    probed: std::collections::HashSet<String>,
    from: Option<FocusHandle>,
    presses: usize,
    most: usize,
    first: Option<FocusHandle>,
    round: bool,
    next: &'static str,
    escaped: bool,
}

impl<K: Fn(&str) -> bool, S: FnMut(&mut Window, &mut App) -> Vec<AxNode>> Observer<K, S> {
    /// The first snapshot, and whether a modal scopes it.
    pub fn open(
        window: &mut Window,
        cx: &mut App,
        name: &str,
        walk: bool,
        keep: K,
        snap: S,
    ) -> Self {
        let mut observer = Self {
            name: name.to_owned(),
            walk,
            keep,
            snap,
            reading: Reading::default(),
            refused: Vec::new(),
            before: window.focused(cx),
            probe: None,
            probed: Default::default(),
            from: None,
            presses: 0,
            most: 0,
            first: None,
            round: false,
            next: "tab",
            escaped: false,
        };
        let (nodes, outside, stops) = observer.look(window, cx);
        observer.reading.take(nodes, outside, window, cx);
        observer.most = stops;
        observer.reading.modal = tree::modal_active(window);
        let first = std::mem::take(&mut observer.reading.snapshots[0]);
        observer.start_probe(&first);
        observer.reading.snapshots[0] = first;
        observer
    }

    fn start_probe(&mut self, nodes: &[AxNode]) {
        let Some(composite) = arrowed(nodes).filter(|_| self.walk) else {
            return;
        };
        if self.probed.insert(composite.id.clone()) {
            let mut left = arrow_pairs(nodes, composite);
            let [id, row] = spot(nodes, composite);
            self.probe = Some(Probe {
                arrows: Arrows {
                    composite: composite.clone(),
                    keys: vec![left.remove(0)],
                    focused: [id, None, None],
                    rows: [row, None, None],
                },
                left,
                pressed: 0,
            });
        }
    }

    /// Presses the next key: the probe's, then the walk's. False, pressing
    /// nothing, once the walk is done.
    pub fn press(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.from = window.focused(cx);
        if let Some(probe) = &mut self.probe {
            let spots = probe.arrows.spots();
            let stuck = probe.pressed == 1 && spots[1] == spots[0];
            if stuck && !probe.left.is_empty() {
                probe.arrows.keys.push(probe.left.remove(0));
                probe.pressed = 0;
            }
            if probe.pressed < 2 {
                let keys = probe.arrows.keys[probe.arrows.keys.len() - 1];
                let _ = press_keys(window, cx, keys[probe.pressed], "");
                probe.pressed += 1;
                return true;
            }
            self.reading
                .arrows
                .extend(self.probe.take().map(|probe| probe.arrows));
        }
        let done = self.presses > 0 && (self.round || self.first.is_none());
        if !self.walk || done || self.presses >= 4 * (self.most + 1) {
            return false;
        }
        self.presses += 1;
        if self.next != "tab" {
            self.escaped = true;
        }
        let _ = press_keys(window, cx, self.next, "");
        true
    }

    /// Reads the window after the key `press` sent. For the probe, whether
    /// the focus has moved off where the key was pressed yet: a view moves
    /// it once it has heard the key, and a caller that pressed the key in
    /// an update of its own reads again until it has, or a deadline passes
    /// (the last read counts). A walk step is read once: Tab moves the
    /// focus natively.
    pub fn read(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let now = window.focused(cx);
        let (nodes, outside, seen) = self.look(window, cx);
        if let Some(probe) = self.probe.as_mut().filter(|probe| probe.pressed > 0) {
            let n = probe.pressed;
            [probe.arrows.focused[n], probe.arrows.rows[n]] = spot(&nodes, &probe.arrows.composite);
            return probe.arrows.focused[n] != probe.arrows.focused[n - 1];
        }
        self.most = self.most.max(seen);
        self.start_probe(&nodes);
        self.reading.take(nodes, outside, window, cx);
        let stayed = now.is_some() && now == self.from;
        self.escaped &= stayed;
        let leave = stayed && !self.escaped && !self.reading.modal;
        match self.presses {
            1 => self.first = now,
            _ => self.round |= now == self.first && !leave,
        }
        self.next = match leave {
            true => "escape tab",
            false => "tab",
        };
        true
    }

    /// The reading, the focus given back where it was.
    pub fn finish(mut self, window: &mut Window, cx: &mut App) -> Reading {
        if self.walk {
            match self.before {
                Some(handle) => handle.focus(window, cx),
                None => window.blur(cx),
            }
        }
        self.reading
            .arrows
            .extend(self.probe.map(|probe| probe.arrows));
        self.reading.refused = self.refused;
        self.reading
    }

    fn look(&mut self, window: &mut Window, cx: &mut App) -> (Vec<AxNode>, bool, usize) {
        let mut nodes = (self.snap)(window, cx);
        let kept_name = |id: NodeId| match nodes.iter().find(|node| node.node == id) {
            Some(node) => node.id.clone(),
            None => window
                .a11y_element_id(id)
                .map_or_else(|| format!("{id:?}"), |kept| format!("{kept:?}")),
        };
        note_refused(
            &mut self.refused,
            window.a11y_refused_elements(),
            &self.name,
            &self.keep,
            kept_name,
        );
        let stops = nodes.iter().filter(|node| offers(node, "focus")).count();
        let dropped = nodes
            .iter()
            .any(|node| has(node, "focused") && !(self.keep)(&node.scope));
        nodes.retain(|node| (self.keep)(&node.scope));
        let outside = dropped && focused(&nodes).is_empty();
        (nodes, outside, stops)
    }
}

struct Probe {
    arrows: Arrows,
    left: Vec<[&'static str; 2]>,
    pressed: usize,
}

#[cfg(any(test, feature = "test-support"))]
/// What `GET /audit?walk=1` answers over `window`, served as `w`, with
/// `view=` when given; `keep` is the door's (`Door::keep`), what the walk
/// puts back once it ends.
pub async fn door_audit(
    window: gpui_kit::AnyWindowHandle,
    view: Option<&str>,
    keep: &'static (dyn Fn(&Window, &App) -> Option<crate::ax::Restore> + Send + Sync),
    cx: &mut gpui_kit::TestAppContext,
) -> serde_json::Value {
    let key = crate::runtime::WindowKey::unique();
    let served = move |_: &App| vec![("w".to_owned(), key, window)];
    let filter = crate::ax::Filter {
        window: None,
        view: view.map(str::to_owned),
    };
    let reply = cx
        .spawn(async move |mut cx| {
            let request = crate::ax::Request::Audit {
                filter,
                walk: true,
                launcher: false,
            };
            let door = crate::ax::Door {
                keep,
                chords: &Vec::new,
                views: &|_| Vec::new(),
                windows: &served,
                settle: &|_| true,
            };
            crate::ax::answer(request, &door, &mut Default::default(), &mut cx).await
        })
        .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    serde_json::from_str(&reply.body).unwrap()
}
