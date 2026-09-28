//! The rule table, as `docs/ax.md` §7 lists it: each rule's id, severity
//! and predicate.
use serde::Serialize;

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

pub(super) fn severity(rule: &str) -> Severity {
    RULES
        .iter()
        .find(|row| row.id == rule)
        .map_or(Error, |row| row.severity)
}
