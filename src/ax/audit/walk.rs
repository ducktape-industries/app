//! The rules over a screen and over a walk: AX-018 on each snapshot,
//! AX-020 … AX-025 over the sequence a Tab walk yields.
use super::*;

/// AX-018 on one snapshot: `launcher` is the caller's word that the shell
/// screen is not the desk; under a modal the rule does not apply.
pub(super) fn screen_rules(nodes: &[AxNode], reading: &Reading, launcher: bool, tally: &mut Tally) {
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

/// `node` has the keys: focused, or a composite whose active row is.
fn holds(node: &AxNode) -> bool {
    has(node, "focused") || node.more.active_descendant.is_some()
}

/// AX-020 … AX-025 over the sequence; `step N` is the snapshot after N
/// presses.
pub(super) fn walk_rules(reading: &Reading, tally: &mut Tally) {
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
        // a Tab press reached it: where the state opened does not count
        let reached = snapshots[1..].iter().any(|nodes| {
            nodes
                .iter()
                .any(|other| other.id == node.id && holds(other))
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
            if let Some(escape) = reading.escape.get(n).filter(|_| shell(dialog)) {
                tally.check("AX-024", dialog, *escape, || {
                    "a shell dialog shows and escape is not bound".to_owned()
                });
            }
        }
    }
}
