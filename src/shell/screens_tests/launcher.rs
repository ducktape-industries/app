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
    let resizable = |native: &mut VisualTestContext| {
        let id = native.update(|window, _| window.window_handle().window_id());
        crate::shell::windows::RESIZABLE.with_borrow(|told| {
            let last = told.iter().rev().find(|(at, _)| *at == id);
            last.map(|(_, resizable)| *resizable)
        })
    };
    assert_eq!(
        resizable(&mut native),
        Some(false),
        "the launcher opens fixed"
    );
    for (screen, expected) in [(Screen::Desk, true), (Screen::Connect, false)] {
        set_screen(&view, screen, &mut native);
        native.run_until_parked();
        assert_eq!(resizable(&mut native), Some(expected), "{screen:?}");
    }
}

#[gpui_kit::test]
fn a_size_the_compositor_gave_the_console_stays_across_the_crossing(cx: &mut TestAppContext) {
    use crate::shell::entities::Screen;
    let given = size(px(1000.), px(700.));
    for compositor_sizes in [false, true] {
        let (view, mut native) = open(Seed::boot(), cx);
        let app = entities(&view, &mut native);
        if compositor_sizes {
            native.update(|_, cx| {
                app.windows
                    .update(cx, |windows, _| windows.compositor_sizes_for_test())
            });
        }
        native.simulate_resize(given);
        native.run_until_parked();
        set_screen(&view, Screen::Desk, &mut native);
        native.run_until_parked();
        let (id, drawn) = native.update(|window, cx| {
            draw(window, cx);
            (window.window_handle().window_id(), window.viewport_size())
        });
        let asked = crate::shell::windows::BOUNDED.with_borrow(|told| {
            told.iter()
                .rev()
                .find(|(at, _)| *at == id)
                .map(|(_, asked)| *asked)
        });
        match compositor_sizes {
            false => assert_eq!(
                asked,
                Some(size(px(1280.), px(800.))),
                "the desk takes its own size where the app sizes it"
            ),
            true => {
                assert_eq!(asked, None, "the compositor's size was asked over");
                assert_eq!(drawn, given, "the desk lays out to the size it was given");
            }
        }
    }
}
