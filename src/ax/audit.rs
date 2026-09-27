//! The phase-1 audit (`docs/ax.md` §1.2, §2): the rule table applied to what
//! the door saw. Per-node rules run over every node of every snapshot; walk
//! rules over the sequence a Tab walk yields. [`audit`] is pure: [`observe`]
//! is the one function here that touches a window, and it only takes the
//! snapshots and presses the keys. What the door cannot see it is told:
//! whether a modal scopes the snapshot, whether `escape` was bound at each
//! step, and — the caller's word, never a guess from the tree — whether the
//! shell screen is a launcher screen (AX-018).
use super::actions::{press_keys, shortcuts};
use super::{AxNode, tree};
use gpui_kit::{App, Window};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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

pub(crate) const RULES: [Rule; 24] = [
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
    rule("AX-012", Error, "A node with press also offers focus."),
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
        "Every node offering focus in the first snapshot is focused once in the walk.",
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
];

fn severity(rule: &str) -> Severity {
    RULES
        .iter()
        .find(|row| row.id == rule)
        .map_or(Error, |row| row.severity)
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub(crate) struct Violation {
    pub(crate) rule: &'static str,
    pub(crate) severity: Severity,
    /// the node's door id; a walk step's `step N` (N presses in)
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
    /// tab presses taken; 0 when the walk was not asked for
    pub(crate) presses: usize,
    /// AX-018 was evaluated: the caller said the screen is a launcher screen
    pub(crate) launcher: bool,
    pub(crate) violations: Vec<Violation>,
    /// per rule: distinct nodes (or steps) its antecedent selected
    pub(crate) applicable: BTreeMap<&'static str, usize>,
    pub(crate) coverage: Coverage,
}

/// What one screen state showed the door: the first snapshot, then one per
/// tab press; `escape`, one per snapshot, whether `escape` was bound then.
#[derive(Debug, Default)]
pub(crate) struct Reading {
    pub(crate) snapshots: Vec<Vec<AxNode>>,
    pub(crate) escape: Vec<bool>,
    pub(crate) modal: bool,
}

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

/// AX-001 … AX-017 on one node; AX-016 looks at its siblings in `nodes`.
fn node_rules(node: &AxNode, nodes: &[AxNode], tally: &mut Tally) {
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

/// AX-020 … AX-025 over the sequence; `step N` is the snapshot after N
/// presses.
fn walk_rules(reading: &Reading, tally: &mut Tally) {
    let snapshots = &reading.snapshots;
    let Some(first) = snapshots.first() else {
        return;
    };
    let step = |n: usize| format!("step {n}");
    for (n, nodes) in snapshots.iter().enumerate().skip(1) {
        let now = focused(nodes);
        tally.step("AX-020", &step(n), "", "", now.len() == 1, || {
            format!("{} nodes focused after tab: {now:?}", now.len())
        });
        let was = focused(&snapshots[n - 1]);
        tally.step("AX-022", &step(n), "", "", now != was, || {
            format!("tab left focus on {now:?}")
        });
    }
    let walked = snapshots.len() > 1;
    for node in first.iter().filter(|node| walked && offers(node, "focus")) {
        let reached = snapshots.iter().any(|nodes| {
            nodes
                .iter()
                .any(|other| other.id == node.id && has(other, "focused"))
        });
        tally.check("AX-021", node, reached, || {
            "offers focus but the tab walk never reached it".to_owned()
        });
    }
    for (n, nodes) in snapshots.iter().enumerate() {
        let some = !focused(nodes).is_empty();
        if reading.modal && walked {
            tally.step("AX-023", &step(n), "", "", some, || {
                "focus left the modal's subtree".to_owned()
            });
        }
        let dialog = nodes.iter().find(|node| node.role == "Dialog");
        if let Some(dialog) = dialog {
            tally.check("AX-025", dialog, some, || {
                "a dialog shows and nothing is focused".to_owned()
            });
            if shell(dialog) {
                if let Some(escape) = reading.escape.get(n) {
                    tally.check("AX-024", dialog, *escape, || {
                        "a shell dialog shows and escape is not bound".to_owned()
                    });
                }
            }
        }
    }
}

/// The report of `docs/ax.md` §2 over `reading`. `launcher`: the shell screen
/// is a launcher screen (AX-018 applies); false leaves the rule unevaluated.
pub(crate) fn audit(reading: &Reading, launcher: bool) -> Report {
    let mut tally = Tally::default();
    for nodes in &reading.snapshots {
        for node in nodes {
            node_rules(node, nodes, &mut tally);
        }
        screen_rules(nodes, reading, launcher, &mut tally);
    }
    walk_rules(reading, &mut tally);
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

impl Report {
    pub(crate) fn errors(&self) -> impl Iterator<Item = &Violation> {
        self.violations
            .iter()
            .filter(|violation| violation.severity == Error)
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

/// The reading of one window: `snap` once, and with `walk`, `tab` through the
/// window's own key dispatch N + 1 times (N: nodes offering focus in the
/// first snapshot, counted before `keep` filters), `snap` after each. Focus
/// goes back where it was.
pub(crate) fn observe(
    window: &mut Window,
    cx: &mut App,
    walk: bool,
    keep: impl Fn(&AxNode) -> bool,
    mut snap: impl FnMut(&mut Window, &mut App) -> Vec<AxNode>,
) -> Reading {
    let before = window.focused(cx);
    let mut reading = Reading::default();
    let mut take = |window: &mut Window, cx: &mut App, reading: &mut Reading| {
        let mut nodes = snap(window, cx);
        let stops = nodes.iter().filter(|node| offers(node, "focus")).count();
        nodes.retain(&keep);
        reading.escape.push(escape_bound(window, cx));
        reading.snapshots.push(nodes);
        stops
    };
    let stops = take(window, cx, &mut reading);
    // after the first snap: a read switches the tree on and draws it
    reading.modal = tree::modal_active(window);
    if walk {
        for _ in 0..=stops {
            let _ = press_keys(window, cx, "tab", "");
            take(window, cx, &mut reading);
        }
        if let Some(handle) = before {
            handle.focus(window, cx);
        }
    }
    reading
}
