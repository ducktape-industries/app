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
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Error,
    Warn,
}

/// One row of the table, as `docs/ax.md` §7 lists it.
#[derive(Debug, Serialize)]
pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) severity: Severity,
    pub(crate) predicate: &'static str,
}

const fn rule(id: &'static str, severity: Severity, predicate: &'static str) -> Rule {
    Rule {
        id,
        severity,
        predicate,
    }
}

use Severity::{Error, Warn};

pub(crate) const RULES: [Rule; 43] = [
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
pub(crate) struct Violation {
    pub(crate) rule: &'static str,
    pub(crate) severity: Severity,
    /// the node's door id; a walk step's `step N` (N presses in); a refused
    /// element's `<kept door id> <- <its GlobalElementId>` (AX-124)
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) message: String,
}

/// `docs/ax.md` §2: `actionable` = actionable nodes with no error-severity
/// violation over actionable nodes; `rule` = 1 − failed over applicable,
/// every rule counted. Both 1 over an empty denominator.
#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct Coverage {
    pub(crate) actionable: f64,
    pub(crate) rule: f64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    /// distinct ids over every snapshot
    pub(crate) nodes: usize,
    pub(crate) actionable: usize,
    pub(crate) modal: bool,
    /// walk presses taken (`tab`, or `escape tab` after a stay); 0 when
    /// the walk was not asked for
    pub(crate) presses: usize,
    /// AX-018 was evaluated: the caller said the screen is a launcher screen
    pub(crate) launcher: bool,
    pub(crate) violations: Vec<Violation>,
    /// per rule: distinct nodes (or steps) its antecedent selected
    pub(crate) applicable: BTreeMap<&'static str, usize>,
    pub(crate) coverage: Coverage,
}

/// What one screen state showed the door: the first snapshot, then one per
/// tab press; `escape`, one per snapshot, whether `escape` was bound then;
/// `outside`, one per snapshot, whether its focus was outside `keep`;
/// `arrows`, the arrow probe of each composite the focus sat in; and,
/// the caller's word, `chords`: `(control name, chord)` for each control
/// the app's Help lists with a chord (AX-114); and `refused`, what the fork
/// dropped from the tree on the frame of each snapshot (AX-124).
#[derive(Debug, Default)]
pub(crate) struct Reading {
    pub(crate) snapshots: Vec<Vec<AxNode>>,
    pub(crate) escape: Vec<bool>,
    pub(crate) modal: bool,
    pub(crate) arrows: Vec<Arrows>,
    pub(crate) chords: Vec<(String, String)>,
    pub(crate) refused: Vec<Refused>,
    /// one per snapshot: the focus is on a node `keep` dropped, and on no
    /// node it kept (a `view=` audit's walk on a shell stop), so the step
    /// is not the view's to judge
    pub(crate) outside: Vec<bool>,
}

/// An element the fork left out of the tree because an earlier one had its
/// accessibility id (AX-124), as one snapshot's frame refused it: the door
/// id of the node that kept it (its `GlobalElementId`, else its `NodeId`,
/// where the snapshot does not show it), the refused element's
/// `GlobalElementId`, and which of the identical refusals on that frame
/// this is (two siblings given one element id refuse twice with one path).
#[derive(Debug)]
pub(crate) struct Refused {
    pub(crate) kept: String,
    pub(crate) element: String,
    pub(crate) nth: usize,
}

/// Add what one frame refused, `list` (`Window::a11y_refused_elements`), to
/// `into`, the kept node named by `kept_name`. Only the elements whose scope
/// `keep` takes: the scope a snapshot named `name` gives a node the element
/// draws.
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
            element: format!("{element:?}"),
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
/// after its arrow back.
#[derive(Debug)]
pub(crate) struct Arrows {
    pub(crate) composite: AxNode,
    pub(crate) keys: Vec<[&'static str; 2]>,
    pub(crate) focused: [Option<String>; 3],
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

/// A composite whose rows the arrows pick (AX-107).
const ARROWED: [&str; 7] = [
    "Tree",
    "ListBox",
    "Menu",
    "Grid",
    "EditableComboBox",
    "TabList",
    "RadioGroup",
];
/// A row of a composite.
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

fn named(node: &AxNode) -> bool {
    !node.name.trim().is_empty()
}

fn offers(node: &AxNode, action: &str) -> bool {
    node.actions.contains(&action)
}

fn has(node: &AxNode, state: &str) -> bool {
    node.state.contains(&state)
}

/// `in` has no `/`: a native node.
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

/// One snapshot's nodes by door id, to follow `parent`.
struct Snapshot<'a>(HashMap<&'a str, &'a AxNode>);

impl<'a> Snapshot<'a> {
    fn of(nodes: &'a [AxNode]) -> Self {
        Self(nodes.iter().map(|node| (node.id.as_str(), node)).collect())
    }

    /// `node`'s ancestors the snapshot has, nearest first.
    fn ancestors(&self, node: &'a AxNode) -> impl Iterator<Item = &'a AxNode> + '_ {
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

/// The applicable set and the failures, one entry per `(rule, id)` however
/// many snapshots repeat the node.
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

/// Every per-node rule on one node: AX-001 … AX-017; then what the door's
/// phase-2 keys say about it (AX-101, 102, 106, 108 … 114, 116 … 118) and
/// where it sits (AX-105, 119). AX-016 and AX-112 look at its siblings in
/// `nodes`, AX-012, AX-105 and AX-119 at its ancestors through
/// `snapshot`, AX-114 at the chords `reading` carries.
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
        // a composite's rows are reached by the arrows, from the composite,
        // once the keys are in it: one that takes focus (view_wire::audit's
        // Unreachable also asks it to hear a key, which the door cannot see)
        let composite = snapshot
            .ancestors(node)
            .any(|above| tree::COMPOSITES.contains(&above.role.as_str()) && offers(above, "focus"));
        // a rich text's links, which no element draws, are reached by the
        // arrows from the box around the text once Tab has put the keys in
        // it (src/render/text/links.rs); the picked one is a node the keys
        // are on, the box's active descendant, which the snapshot reports
        // focused
        let linked = node.synthetic && snapshot.ancestors(node).any(|above| offers(above, "focus"));
        // the shell's own: a desk window without the keys is reached by the
        // chord its box names (⌘1…⌘9), which hands that window the keys, as
        // a press on it does. The box holding the focus has them already:
        // there the chord moves nothing. A view's rows answer to
        // view_wire::audit alone.
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

/// AX-124: one violation per refused element, however many snapshots saw
/// it, named by the first: the kept node's door id can change from one
/// snapshot to the next. A refused element has no node to count, so the
/// rule is applicable only where it fails. The fork panics on a duplicate
/// in a debug build, so only a release build gets here.
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

/// AX-018 on one snapshot: `launcher` is the caller's word that the shell
/// screen is not the desk; under a modal the rule does not apply.
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

/// AX-107's own half on one snapshot: focus on a composite that has rows
/// is on one of them, not on the composite.
fn snapshot_rules(nodes: &[AxNode], snapshot: &Snapshot<'_>, tally: &mut Tally) {
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

/// The cells of a grid `nodes` has inside `grid`.
fn cells<'a>(
    nodes: &'a [AxNode],
    snapshot: &Snapshot<'a>,
    grid: &AxNode,
) -> impl Iterator<Item = &'a AxNode> {
    nodes.iter().filter(move |node| {
        matches!(node.role.as_str(), "Cell" | "GridCell") && snapshot.within(node, &grid.id)
    })
}

/// The composite an arrow press moves in `nodes` (AX-107): the nearest
/// one at or above the focus with two rows or more (a grid, two cells:
/// its arrows move among them), when it has the keys: between the focus
/// and it, nothing takes focus of its own but a row of a list, tree, menu
/// or grid (whose rows the arrows may move the focus among). A separate
/// Tab stop inside it (a rich text's links in a message) keeps its arrows,
/// and the composite is probed when Tab lands on it; a tab list or radio
/// group whose items are each a Tab stop is reached by Tab, and only the
/// one-stop shape is probed.
fn arrowed(nodes: &[AxNode]) -> Option<&AxNode> {
    let snapshot = Snapshot::of(nodes);
    let focus = nodes.iter().find(|node| has(node, "focused"))?;
    let up: Vec<_> = std::iter::once(focus)
        .chain(snapshot.ancestors(focus))
        .collect();
    let at = up.iter().position(|node| {
        let items = match node.role.as_str() {
            "Grid" => cells(nodes, &snapshot, node).count(),
            _ => rows(nodes, &snapshot, node),
        };
        ARROWED.contains(&node.role.as_str()) && items > 1
    })?;
    let roving = !matches!(up[at].role.as_str(), "TabList" | "RadioGroup");
    up[..at]
        .iter()
        .all(|node| !offers(node, "focus") || roving && ITEM.contains(&node.role.as_str()))
        .then_some(up[at])
}

/// The arrow pairs the probe of `composite` in `nodes` tries, each a key
/// and the key back, each way round, since the active item may sit at an
/// end the first key does not pass: its orientation's (a tab list's is
/// horizontal unless it says, anything else's vertical); a grid's rows,
/// or its cells when one row holds them all (a header row of column
/// headers holds none).
fn arrow_pairs(nodes: &[AxNode], composite: &AxNode) -> Vec<[&'static str; 2]> {
    const ROWS: [[&str; 2]; 2] = [["down", "up"], ["up", "down"]];
    const CELLS: [[&str; 2]; 2] = [["right", "left"], ["left", "right"]];
    let horizontal = match composite.role.as_str() {
        "Grid" => {
            let snapshot = Snapshot::of(nodes);
            let rows: std::collections::HashSet<_> = cells(nodes, &snapshot, composite)
                .filter_map(|cell| snapshot.ancestors(cell).find(|row| row.role == "Row"))
                .map(|row| &row.id)
                .collect();
            rows.len() == 1
        }
        role => composite
            .more
            .orientation
            .map_or(role == "TabList", |way| way == "horizontal"),
    };
    match horizontal {
        true => CELLS.to_vec(),
        false => ROWS.to_vec(),
    }
}

/// `node` has the keys: focused, or a composite whose active row is.
fn holds(node: &AxNode) -> bool {
    has(node, "focused") || node.more.active_descendant.is_some()
}

/// AX-020 … AX-025 over the sequence, `step N` the snapshot after N
/// presses, a step whose focus is outside the audited scope left to the
/// scope it is in; then AX-103, AX-104 and AX-107's arrows. A dialog that
/// shows has the focus, in every snapshot (AX-104): a menu the walk leaves
/// has closed (owner, 2026-09-28). One the Tab walk never leaves is modal
/// to the keyboard, and says so (AX-103). Each arrow press of the probe
/// moves the active row (AX-107).
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
        // off a node `keep` dropped, the focus has moved
        let moved = now != focused(&snapshots[n - 1]) || outside(n - 1);
        tally.step("AX-022", &step(n), "", "", moved, || {
            format!("tab left focus on {now:?}")
        });
    }
    let walked = snapshots.len() > 1;
    let opened = Snapshot::of(first);
    // gone from a snapshot the walk took: a menu Tab left, which closed
    // (a Dialog or a Menu, as the shell's hang from the bar)
    let closed = |id: &str| {
        snapshots[1..]
            .iter()
            .any(|nodes| nodes.iter().all(|other| other.id != id))
    };
    // a focused node that offers no focus of its own is the active
    // descendant of the node that has the keys, its nearest ancestor that
    // offers focus (a rich text's picked link, under the box around the
    // text): that ancestor was reached
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
        // a Tab press reached it: where the state opened does not count,
        // but in a dialog that closed when Tab left it: it had the keys as
        // the dialog opened, and no Tab comes back to it
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
        // focus on a node `keep` dropped is still focus
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
    for Arrows {
        composite,
        keys,
        focused,
    } in &reading.arrows
    {
        let pass = focused[0] != focused[1] && focused[1] != focused[2];
        tally.check("AX-107", composite, pass, || {
            let stuck: String = keys[..keys.len() - 1]
                .iter()
                .map(|[there, _]| format!("{there} moved nothing; "))
                .collect();
            let [there, back] = keys[keys.len() - 1];
            format!("{stuck}{there} then {back} leaves the active row at {focused:?}")
        });
    }
}

/// The report of `docs/ax.md` §2 over `reading`. `launcher`: the shell screen
/// is a launcher screen (AX-018 applies); false leaves the rule unevaluated.
pub(crate) fn audit(reading: &Reading, launcher: bool) -> Report {
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

#[cfg(test)]
impl Report {
    pub(crate) fn errors(&self) -> impl Iterator<Item = &Violation> {
        self.violations
            .iter()
            .filter(|violation| violation.severity == Error)
    }
}

impl Reading {
    /// One more snapshot, with whether its focus is outside what the
    /// audit keeps and whether `escape` is bound now.
    fn take(&mut self, nodes: Vec<AxNode>, outside: bool, window: &Window, cx: &App) {
        self.escape.push(escape_bound(window, cx));
        self.outside.push(outside);
        self.snapshots.push(nodes);
    }
}

/// `escape` is bound where focus is now.
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
/// window's own key dispatch, `snap` after each: N + 1 times (N: nodes
/// offering focus in the first snapshot, counted before `keep` filters),
/// and on until focus has come back to where the first press put it. A
/// stop the first snapshot does not show (scrolled away, drawn since) makes
/// the Tab cycle longer than N + 1; the walk still goes all the way round,
/// up to 4 (M + 1) presses, M the most stops a snapshot has shown so far.
/// A Tab that leaves the focus where it was (a guest editor keeps Tab for
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
#[cfg(test)]
pub(crate) fn observe(
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
pub(crate) struct Observer<K, S> {
    name: String,
    walk: bool,
    keep: K,
    snap: S,
    reading: Reading,
    refused: Vec<Refused>,
    /// the focus before the first press, given back by `finish`
    before: Option<FocusHandle>,
    /// the arrow probe under way, and the composites probed so far
    probe: Option<Probe>,
    probed: std::collections::HashSet<String>,
    /// the focus the last key was pressed from
    from: Option<FocusHandle>,
    /// the Tab walk: presses taken, N and M, the handle the first press
    /// gave the focus (the handle, not the node: a stop off the viewport
    /// has no node), whether the focus has come back to it, the next key,
    /// and whether `escape` was pressed since the focus last moved
    presses: usize,
    stops: usize,
    most: usize,
    first: Option<FocusHandle>,
    round: bool,
    next: &'static str,
    escaped: bool,
}

impl<K: Fn(&str) -> bool, S: FnMut(&mut Window, &mut App) -> Vec<AxNode>> Observer<K, S> {
    /// The first snapshot, and whether a modal scopes it.
    pub(crate) fn open(
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
            stops: 0,
            most: 0,
            first: None,
            round: false,
            next: "tab",
            escaped: false,
        };
        let (nodes, outside, stops) = observer.look(window, cx);
        observer.reading.take(nodes, outside, window, cx);
        (observer.stops, observer.most) = (stops, stops);
        // after the first snap: a read switches the tree on and draws it
        observer.reading.modal = tree::modal_active(window);
        let first = std::mem::take(&mut observer.reading.snapshots[0]);
        observer.start_probe(&first);
        observer.reading.snapshots[0] = first;
        observer
    }

    /// The arrow probe of the composite the focus sits in, when it has
    /// the keys and was not probed yet (a walk probes; a look does not).
    fn start_probe(&mut self, nodes: &[AxNode]) {
        let Some(composite) = arrowed(nodes).filter(|_| self.walk) else {
            return;
        };
        if self.probed.insert(composite.id.clone()) {
            let mut left = arrow_pairs(nodes, composite);
            self.probe = Some(Probe {
                arrows: Arrows {
                    composite: composite.clone(),
                    keys: vec![left.remove(0)],
                    focused: [focused_id(nodes), None, None],
                },
                left,
                pressed: 0,
            });
        }
    }

    /// Presses the next key: the probe's, then the walk's. False, pressing
    /// nothing, once the walk is done.
    pub(crate) fn press(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.from = window.focused(cx);
        if let Some(probe) = &mut self.probe {
            // an arrow that moved nothing: the next pair, from where it began
            let stuck = probe.pressed == 1 && probe.arrows.focused[1] == probe.arrows.focused[0];
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
        let done = self.presses > self.stops && (self.round || self.first.is_none());
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
    pub(crate) fn read(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let now = window.focused(cx);
        let (nodes, outside, seen) = self.look(window, cx);
        if let Some(probe) = self.probe.as_mut().filter(|probe| probe.pressed > 0) {
            let n = probe.pressed;
            probe.arrows.focused[n] = focused_id(&nodes);
            return probe.arrows.focused[n] != probe.arrows.focused[n - 1];
        }
        self.most = self.most.max(seen);
        self.start_probe(&nodes);
        self.reading.take(nodes, outside, window, cx);
        let stayed = now.is_some() && now == self.from;
        self.escaped &= stayed;
        let leave = stayed && !self.escaped && !self.reading.modal;
        match self.presses {
            1 => self.first = now.clone(),
            // a stay the walk is about to leave has not come back round
            _ => self.round |= now == self.first && !leave,
        }
        self.next = match leave {
            true => "escape tab",
            false => "tab",
        };
        true
    }

    /// The reading, the focus given back where it was.
    pub(crate) fn finish(mut self, window: &mut Window, cx: &mut App) -> Reading {
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

    /// One snapshot, what the fork refused on its frame noted: the nodes
    /// `keep` takes, whether the focus is outside them, and how many
    /// nodes offer focus before the filter.
    fn look(&mut self, window: &mut Window, cx: &mut App) -> (Vec<AxNode>, bool, usize) {
        let mut nodes = (self.snap)(window, cx);
        // the kept node by its door id in `nodes` (before any filter), or
        // through the element-id map where `nodes` does not show it
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

/// An arrow probe under way: the pairs left to try, and how many keys of
/// the one being pressed (the last of `arrows.keys`) were.
struct Probe {
    arrows: Arrows,
    left: Vec<[&'static str; 2]>,
    pressed: usize,
}

/// The first focused id of `nodes`.
fn focused_id(nodes: &[AxNode]) -> Option<String> {
    focused(nodes).first().map(|id| (*id).to_owned())
}
