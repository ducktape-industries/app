//! Live regions (docs/ax.md §4 gap 3): every shell Status is announced
//! politely and every Alert at once, each carrying its words as its name
//! and its value — AT-SPI and UIA speak the name, macOS the value. These
//! read the AccessKit nodes the window built, as the platform adapters do;
//! the gate reads the same through the door (AX-102).
use super::*;
use gpui_kit::Role;
use gpui_kit::accesskit::Live;

/// Every node of `window`'s last tree that is a Status or an Alert, or
/// carries a politeness: its role, name, value and politeness.
pub(super) fn announced(window: &Window) -> Vec<(Role, String, Option<String>, Option<Live>)> {
    window
        .a11y_tree()
        .map(|update| {
            update
                .nodes
                .iter()
                .map(|(_, node)| node)
                .filter(|node| {
                    matches!(node.role(), Role::Status | Role::Alert) || node.live().is_some()
                })
                .map(|node| {
                    (
                        node.role(),
                        node.label().unwrap_or_default().to_owned(),
                        node.value().map(str::to_owned),
                        node.live(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Each screen state's Status and Alert nodes: polite and assertive live
/// regions whose name is their value. The ones named below must be there,
/// so the check is not vacuous.
#[gpui_kit::test]
fn every_shell_status_and_alert_is_live_with_its_words_as_name_and_value(cx: &mut TestAppContext) {
    let expected = [
        (
            "connect-connecting",
            Role::Status,
            "Reaching 127.0.0.1:9000…",
        ),
        ("connect-error-and-recent", Role::Alert, "no route to host"),
        ("sign-in-old-password-error", Role::Alert, "wrong password"),
        ("account-passkey-qr", Role::Status, "Waiting"),
        (
            "link-waiting",
            Role::Status,
            "On a device already signed in",
        ),
        ("desk-toast", Role::Status, "Copied"),
        ("desk-reconnecting", Role::Status, "testkit isn't answering"),
        ("desk-asking", Role::Group, "gate-asking wants to show"),
    ];
    let mut failures = Vec::new();
    let mut found = vec![false; expected.len()];
    for (screen, _, build) in gate::matrix() {
        let (_view, mut native) = open(build(), cx);
        let nodes = native.update(|window, cx| {
            draw(window, cx);
            announced(window)
        });
        for (role, name, value, live) in nodes {
            let polite = match role {
                Role::Alert => Some(Live::Assertive),
                _ => Some(Live::Polite),
            };
            if name.trim().is_empty() || value.as_deref() != Some(name.as_str()) || live != polite {
                failures.push(format!(
                    "{screen}: {role:?} {name:?} value {value:?} live {live:?}, want {polite:?}"
                ));
            }
            for (at, (want_screen, want_role, words)) in expected.iter().enumerate() {
                if *want_screen == screen && *want_role == role && name.starts_with(words) {
                    found[at] = true;
                }
            }
        }
    }
    for (at, (screen, role, words)) in expected.iter().enumerate() {
        if !found[at] {
            failures.push(format!("{screen}: no live {role:?} {words:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
