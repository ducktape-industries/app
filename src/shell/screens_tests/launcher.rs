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
/// that stage's screen, and the window is launcher-sized exactly while that
/// screen is not the desk.
#[gpui_kit::test]
fn the_launcher_size_agrees_with_the_screen_drawn(cx: &mut TestAppContext) {
    use crate::shell::entities::{AccountStep, Screen};
    use crate::shell::layers::tests::Seed;
    let words = || {
        Some(crate::shell::entities::Secret::from(String::from(
            "canoe pond forest",
        )))
    };
    let screens: Vec<(Screen, &str)> = vec![
        (Screen::Connect, "connect"),
        (Screen::Unlock { awaiting: false }, "sign-in"),
        (Screen::Unlock { awaiting: true }, "sign-in"),
        (Screen::Phrase { quiz: None }, "recovery"),
        (
            Screen::Phrase {
                quiz: Some([0, 1, 2]),
            },
            "recovery-check",
        ),
        (Screen::Recover, "recover"),
        (
            Screen::Account {
                step: AccountStep::Name,
            },
            "account-step",
        ),
        (
            Screen::Account {
                step: AccountStep::Link,
            },
            "link-waiting",
        ),
        (Screen::Desk, "menubar"),
    ];
    let names: std::collections::BTreeSet<&str> = screens.iter().map(|(_, id)| *id).collect();
    for (screen, id) in &screens {
        // the key seated or not, locked or not, an old password key or not:
        // none of it picks the screen
        for bits in 0..8u8 {
            let mut seed = Seed::boot();
            seed.screen = *screen;
            if matches!(screen, Screen::Phrase { .. }) {
                seed.phrase = words();
            }
            if let Screen::Account {
                step: AccountStep::Link,
            } = screen
            {
                seed.account.link_code = "ABCD-EFGH".into();
            }
            if bits & 1 != 0 {
                seed.account.signer_key = "ab".into();
            }
            seed.account.locked = bits & 2 != 0;
            seed.account.key_exists = bits & 4 != 0;
            let (_view, mut native) = open(seed, cx);
            let ids = native.update(drawn_ids);
            let drawn: Vec<_> = names
                .iter()
                .filter(|screen| ids.contains(**screen))
                .collect();
            assert_eq!(drawn, vec![id], "{id} with bits {bits:03b}");
        }
    }

    // the console crossing to the desk takes the desk's size, and back to
    // a launcher screen the launcher's
    let mut seed = Seed::boot();
    seed.screen = Screen::Connect;
    let (view, mut native) = open(seed, cx);
    let measured = |native: &mut VisualTestContext| native.update(|window, _| window.bounds().size);
    set_screen(&view, Screen::Desk, &mut native);
    native.run_until_parked();
    let desk = crate::shell::windows::WINDOW_SIZE;
    assert_eq!(measured(&mut native), size(px(desk.0), px(desk.1)));
    set_screen(&view, Screen::Connect, &mut native);
    native.run_until_parked();
    let launcher = crate::shell::layers::LAUNCHER_SIZE;
    assert_eq!(measured(&mut native), size(px(launcher.0), px(launcher.1)));
}
