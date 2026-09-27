//! The launcher's frame and the screen inside it agree, over every stage.
use super::*;
use gpui_kit::TestAppContext;

/// The screen drawn: the id just inside the launcher's frame, or the
/// desk's menu bar.
fn drawn_ids(window: &mut Window, cx: &mut gpui_kit::App) -> std::collections::BTreeSet<String> {
    draw(window, cx);
    let nodes: Vec<_> = window
        .a11y_tree()
        .unwrap()
        .nodes
        .iter()
        .map(|(node, _)| *node)
        .collect();
    nodes
        .into_iter()
        .filter_map(|node| window.a11y_element_id(node))
        .filter_map(|path| {
            let names: Vec<String> = path
                .iter()
                .filter_map(|element| match element {
                    ElementId::Name(name) => Some(name.to_string()),
                    _ => None,
                })
                .collect();
            match names.iter().position(|name| name == "launcher") {
                Some(at) => names.get(at + 1).cloned(),
                None => names.into_iter().find(|name| name == "menubar"),
            }
        })
        .collect()
}

/// For every stage and the sub-steps it draws, the console window draws
/// that stage's screen, and it is launcher-sized exactly when that screen
/// is not the desk.
#[gpui_kit::test]
fn the_launcher_size_agrees_with_the_screen_drawn(cx: &mut TestAppContext) {
    use crate::ui::{Account, Phrase, Unlock};
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    type Build = fn() -> Stage;
    let stages: Vec<(Build, &str)> = vec![
        (|| Stage::Connect, "connect"),
        (|| Stage::Unlock(Unlock::default()), "sign-in"),
        (
            || {
                Stage::Unlock(Unlock {
                    awaiting: true,
                    ..Default::default()
                })
            },
            "sign-in",
        ),
        (
            || {
                Stage::Phrase(Phrase {
                    words: String::from("canoe pond forest").into(),
                    ..Default::default()
                })
            },
            "recovery",
        ),
        (
            || {
                Stage::Phrase(Phrase {
                    words: String::from("canoe pond forest").into(),
                    quiz: Some([0, 1, 2]),
                    ..Default::default()
                })
            },
            "recovery-check",
        ),
        (|| Stage::Recover(Default::default()), "recover"),
        (|| Stage::Account(Account::default()), "account-step"),
        (
            || {
                Stage::Account(Account {
                    link_code: "ABCD-EFGH".into(),
                    ..Default::default()
                })
            },
            "link-waiting",
        ),
        (|| Stage::Desk, "menubar"),
    ];
    let screens: std::collections::BTreeSet<&str> = stages.iter().map(|(_, id)| *id).collect();
    for (stage, id) in &stages {
        // the key seated or not, locked or not, an old password key or not:
        // none of it picks the screen
        for bits in 0..8u8 {
            let (mut state, _) = Ducktape::boot();
            state.stage = stage();
            if bits & 1 != 0 {
                state.signer_key = "ab".into();
            }
            state.sign_in.locked = bits & 2 != 0;
            state.key_exists = bits & 4 != 0;
            let launcher = state.in_launcher();
            let (_view, mut native) = open(state, cx);
            let ids = native.update(drawn_ids);
            let drawn: Vec<_> = screens
                .iter()
                .filter(|screen| ids.contains(**screen))
                .collect();
            assert_eq!(drawn, vec![id], "{id} with bits {bits:03b}");
            assert_eq!(launcher, *id != "menubar", "{id} with bits {bits:03b}");
        }
    }
}
