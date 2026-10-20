//! The launcher's frame and the screen inside it agree, over every stage.
use super::*;
use gpui_kit::TestAppContext;

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
        (Screen::Layout, "layout-step"),
        (Screen::Desk, "menubar"),
    ];
    let names: std::collections::BTreeSet<&str> = screens.iter().map(|(_, id)| *id).collect();
    for (screen, id) in &screens {
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
            let launcher = *screen != Screen::Desk;
            let (_view, mut native) = open(seed, cx);
            let ids = native.update(drawn_ids);
            let drawn: Vec<_> = names
                .iter()
                .filter(|screen| ids.contains(**screen))
                .collect();
            assert_eq!(drawn, vec![id], "{id} with bits {bits:03b}");
            assert_eq!(launcher, *id != "menubar", "{id} with bits {bits:03b}");
        }
    }
}

#[gpui_kit::test]
fn the_launcher_is_fixed_and_the_desk_resizes(cx: &mut TestAppContext) {
    use crate::shell::entities::Screen;
    let (view, mut native) = open(Seed::boot(), cx);
    let resizable =
        |native: &mut VisualTestContext| native.update(|window, _| window.is_resizable());
    assert!(!resizable(&mut native), "the launcher opens fixed");
    for (screen, expected) in [(Screen::Desk, true), (Screen::Connect, false)] {
        set_screen(&view, screen, &mut native);
        native.run_until_parked();
        assert_eq!(resizable(&mut native), expected, "{screen:?}");
    }
}
