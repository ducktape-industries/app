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

mod node;
mod phase_two;
mod table;
mod walk;

use Severity::Error;
use node::node_rules;
use phase_two::Snapshot;
pub(crate) use table::Severity;
use table::severity;
use walk::{screen_rules, walk_rules};

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
/// up to 4 (N + 1) presses. Before the walk, when the focus starts in a
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
        for press in 1..=4 * (stops + 1) {
            let _ = press_keys(window, cx, "tab", "");
            let now = window.focused(cx);
            let (nodes, _) = read(window, cx);
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
