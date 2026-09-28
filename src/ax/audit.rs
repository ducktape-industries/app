//! The audit (`docs/ax.md` §1.2, §1.3, §2): the rule table applied to what
//! the door saw. Per-node rules run over every node of every snapshot; walk
//! rules over the sequence a Tab walk yields. [`audit`] is pure: [`observe`]
//! is the one function here that touches a window, and it only takes the
//! snapshots and presses the keys. What the door cannot see it is told:
//! whether a modal scopes the snapshot, whether `escape` was bound at each
//! step, and — the caller's word, never a guess from the tree — whether the
//! shell screen is a launcher screen (AX-018), and which controls the
//! app's Help lists with a chord (AX-114).
use super::actions::{press_keys, shortcuts};
use super::{AxNode, tree};
use gpui_kit::{App, Window};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

mod phase_two;
mod walk;

use phase_two::Snapshot;
use walk::{screen_rules, walk_rules};

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
        "A node with press also offers focus, or sits in a composite that does.",
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
    rule(
        "AX-104",
        Error,
        "A Dialog the screen state shows holds the focus as it opens.",
    ),
    rule(
        "AX-105",
        Error,
        "A menu item, tab, radio button, option, tree item, row or cell is in its container.",
    ),
    rule("AX-106", Error, "A Heading has a level in 1..=6."),
    rule(
        "AX-107",
        Error,
        "Focus in a composite is on a row, and each down or up press moves it.",
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
        "AX-123",
        Warn,
        "A press node no element draws (a rich text's clickable range) offers focus.",
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
/// tab press; `escape`, one per snapshot, whether `escape` was bound then;
/// `arrows`, the arrow probe of the composite the focus started in; and,
/// the caller's word, `chords`: `(control name, chord)` for each control
/// the app's Help lists with a chord (AX-114).
#[derive(Debug, Default)]
pub(crate) struct Reading {
    pub(crate) snapshots: Vec<Vec<AxNode>>,
    pub(crate) escape: Vec<bool>,
    pub(crate) modal: bool,
    pub(crate) arrows: Vec<Arrows>,
    pub(crate) chords: Vec<(String, String)>,
}

/// An arrow probe (AX-107): the composite the focus sat in, and the
/// focused id before, after `down`, and after `up`.
#[derive(Debug)]
pub(crate) struct Arrows {
    pub(crate) composite: AxNode,
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

/// AX-001 … AX-017 on one node, and AX-123 in AX-012's place on a node no
/// element draws; AX-016 looks at its siblings in `nodes`, AX-012 at its
/// ancestors.
fn node_rules(node: &AxNode, nodes: &[AxNode], snapshot: &Snapshot<'_>, tally: &mut Tally) {
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
    if press && node.synthetic {
        // gpui gives a node no element draws no focus: the gap is the
        // fork's to close, not the view's (docs/ax.md §5, item 8)
        tally.check("AX-123", node, focus, || {
            format!("a {role} the pointer presses and the keyboard cannot reach")
        });
    } else if press {
        // a composite's rows are reached by the arrows, from the composite,
        // once the keys are in it: one that takes focus (view_wire::audit's
        // Unreachable also asks it to hear a key, which the door cannot see)
        let composite = snapshot
            .ancestors(node)
            .any(|above| tree::COMPOSITES.contains(&above.role.as_str()) && offers(above, "focus"));
        tally.check("AX-012", node, focus || composite, || {
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

/// The report of `docs/ax.md` §2 over `reading`. `launcher`: the shell screen
/// is a launcher screen (AX-018 applies); false leaves the rule unevaluated.
pub(crate) fn audit(reading: &Reading, launcher: bool) -> Report {
    let mut tally = Tally::default();
    for nodes in &reading.snapshots {
        let snapshot = Snapshot::of(nodes);
        for node in nodes {
            node_rules(node, nodes, &snapshot, &mut tally);
            phase_two::node_rules(node, nodes, &snapshot, reading, &mut tally);
        }
        screen_rules(nodes, reading, launcher, &mut tally);
        phase_two::snapshot_rules(nodes, &snapshot, &mut tally);
    }
    walk_rules(reading, &mut tally);
    phase_two::walk_rules(reading, &mut tally);
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

impl Reading {
    /// One more snapshot, with whether `escape` is bound now.
    fn take(&mut self, nodes: Vec<AxNode>, window: &Window, cx: &App) {
        self.escape.push(escape_bound(window, cx));
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

/// The reading of one window: `snap` once, and with `walk`, `tab` through the
/// window's own key dispatch, `snap` after each: N + 1 times (N: nodes
/// offering focus in the first snapshot, counted before `keep` filters),
/// and on until focus has come back to where the first press put it. A
/// stop the first snapshot does not show (scrolled away, drawn since) makes
/// the Tab cycle longer than N + 1; the walk still goes all the way round,
/// up to 4 (M + 1) presses, M the most stops a snapshot has shown so far.
/// Before the walk, when the focus starts in a
/// composite with two rows or more, `down` then `up`, `snap` after each
/// ([`Arrows`]). Focus goes back where it was — nowhere included.
pub(crate) fn observe(
    window: &mut Window,
    cx: &mut App,
    walk: bool,
    keep: impl Fn(&AxNode) -> bool,
    mut snap: impl FnMut(&mut Window, &mut App) -> Vec<AxNode>,
) -> Reading {
    let before = window.focused(cx);
    let mut reading = Reading::default();
    let mut read = |window: &mut Window, cx: &mut App| {
        let mut nodes = snap(window, cx);
        let stops = nodes.iter().filter(|node| offers(node, "focus")).count();
        nodes.retain(&keep);
        (nodes, stops)
    };
    let (nodes, stops) = read(window, cx);
    reading.take(nodes, window, cx);
    // after the first snap: a read switches the tree on and draws it
    reading.modal = tree::modal_active(window);
    if walk && let Some(composite) = phase_two::arrowed(&reading.snapshots[0]).cloned() {
        let at = |nodes: &[AxNode]| focused(nodes).first().map(|id| (*id).to_owned());
        let mut seen = [at(&reading.snapshots[0]), None, None];
        for (n, key) in [(1, "down"), (2, "up")] {
            let _ = press_keys(window, cx, key, "");
            seen[n] = at(&read(window, cx).0);
        }
        reading.arrows.push(Arrows {
            composite,
            focused: seen,
        });
    }
    if walk {
        // the handle, not the node: a stop off the viewport has no node
        let (mut first, mut round) = (None, false);
        let (mut press, mut most) = (0, stops);
        while press < 4 * (most + 1) {
            press += 1;
            let _ = press_keys(window, cx, "tab", "");
            let now = window.focused(cx);
            let (nodes, seen) = read(window, cx);
            most = most.max(seen);
            reading.take(nodes, window, cx);
            match press {
                1 => first = now,
                _ => round |= now == first,
            }
            if press > stops && (round || first.is_none()) {
                break;
            }
        }
        match before {
            Some(handle) => handle.focus(window, cx),
            None => window.blur(cx),
        }
    }
    reading
}
