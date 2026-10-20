//! Whatever opens over the desk takes the keys as it opens, and gives them
//! back as it closes (docs/ax.md §4 gap 5): the dialogs on a scrim and the
//! menus hanging from the bar alike, and one giving way to the next. A menu
//! the keys leave closes, and leaves them where they went.
use super::super::layers::tests::open_now;
use super::*;
use crate::runtime::consent;
use gpui_kit::{Pixels, Role};
use std::collections::HashMap;

fn focus_inside(window: &Window, role: Role, name: &str) -> bool {
    let update = window.a11y_tree().unwrap();
    let nodes: HashMap<_, _> = update.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let Some(card) = nodes
        .iter()
        .find(|(_, node)| node.role() == role && node.label() == Some(name))
        .map(|(id, _)| *id)
    else {
        return false;
    };
    let mut stack = vec![card];
    while let Some(id) = stack.pop() {
        if id == update.focus {
            return true;
        }
        stack.extend(nodes.get(&id).map_or(&[][..], |node| node.children()));
    }
    false
}

fn focused_node(window: &Window) -> Option<(Role, String)> {
    let update = window.a11y_tree()?;
    update
        .nodes
        .iter()
        .find(|(id, _)| *id == update.focus)
        .map(|(_, node)| (node.role(), node.label().unwrap_or_default().to_owned()))
}

fn show(view: &Entity<WindowRoot>, native: &mut VisualTestContext, overlay: Option<Overlay>) {
    super::super::layers::tests::show(view, overlay, native);
    native.update(draw);
    native.update(draw);
}

#[gpui_kit::test]
fn every_overlay_takes_the_keys_as_it_opens_and_gives_them_back(cx: &mut TestAppContext) {
    let overlays = [
        (Overlay::Spotlight, Role::Dialog, "Search"),
        (Overlay::Approve, Role::Dialog, "Add a device"),
        (
            Overlay::Settings(SettingsPage::Appearance),
            Role::Dialog,
            "Settings",
        ),
    ]
    .into_iter()
    .chain(MENUS);
    for (overlay, role, name) in overlays {
        let (view, mut native) = open(gate::desk(), cx);
        native.update(draw);
        native.simulate_keystrokes("tab");
        let before = native.update(|window, cx| {
            draw(window, cx);
            window.focused(cx)
        });
        assert!(before.is_some());
        show(&view, &mut native, Some(overlay));
        native.update(|window, _| {
            assert!(
                focus_inside(window, role, name),
                "{name} took no keys: {:?}",
                focused_node(window)
            );
        });
        native.simulate_keystrokes("escape");
        native.update(draw);
        native.update(|window, cx| {
            draw(window, cx);
            assert_eq!(window.focused(cx), before, "{name} kept the keys");
        });
    }

    let (view, mut native) = open(gate::desk(), cx);
    native.update(draw);
    native.simulate_keystrokes("tab");
    let before = native.update(|window, cx| {
        draw(window, cx);
        window.focused(cx)
    });
    show(
        &view,
        &mut native,
        Some(Overlay::Menu(Popover::Notifications)),
    );
    show(
        &view,
        &mut native,
        Some(Overlay::Settings(SettingsPage::Appearance)),
    );
    native.update(|window, _| assert!(focus_inside(window, Role::Dialog, "Settings")));
    native.simulate_keystrokes("escape");
    native.update(draw);
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(window.focused(cx), before);
    });
}

const MENUS: [(Overlay, Role, &str); 4] = [
    (Overlay::Network, Role::Menu, "Networks"),
    (Overlay::Menu(Popover::Node), Role::Dialog, "Node status"),
    (Overlay::Menu(Popover::Account), Role::Dialog, "Account"),
    (
        Overlay::Menu(Popover::Notifications),
        Role::Dialog,
        "Notifications",
    ),
];

#[gpui_kit::test]
fn a_menu_closes_when_the_keys_leave_it(cx: &mut TestAppContext) {
    for (overlay, role, name) in MENUS {
        for (out, presses) in [("shift-tab", 1), ("tab", 30)] {
            let (view, mut native) = open(gate::desk(), cx);
            native.update(draw);
            show(
                &view,
                &mut native,
                Some(Overlay::Settings(SettingsPage::Appearance)),
            );
            show(&view, &mut native, None);
            show(&view, &mut native, Some(overlay));
            native.update(|window, _| {
                assert!(focus_inside(window, role, name), "{name} took no keys");
            });
            let mut left = 0;
            while open_now(&view, &mut native).is_some() && left < presses {
                native.simulate_keystrokes(out);
                native.update(draw);
                native.update(draw);
                left += 1;
            }
            assert_eq!(open_now(&view, &mut native), None, "{out} out of {name}");
            let (keys, root) = native.update(|window, cx| {
                draw(window, cx);
                (window.focused(cx), view.read(cx).focus.clone())
            });
            assert!(
                keys.is_some() && keys != Some(root),
                "{out} out of {name}: the keys went where {out} took them"
            );
        }
    }
}

#[gpui_kit::test]
fn a_click_outside_a_menu_closes_it(cx: &mut TestAppContext) {
    for (overlay, _, name) in MENUS {
        let (view, mut native) = open(gate::desk(), cx);
        super::activate(&mut native);
        native.update(draw);
        show(&view, &mut native, Some(overlay));
        native.simulate_click(
            gpui_kit::point(px(640.), px(790.)),
            gpui_kit::Modifiers::none(),
        );
        native.update(draw);
        assert_eq!(open_now(&view, &mut native), None, "a click under {name}");

        show(&view, &mut native, Some(overlay));
        let nodes = native.update(|window, cx| {
            draw(window, cx);
            serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
        });
        let bar = find(&nodes, "MenuBar", "Ducktape");
        let at = |n: usize| bar["bounds"][n].as_f64().unwrap() as f32;
        native.simulate_click(
            gpui_kit::point(px(at(0) + 3.), px((at(1) + at(3)) / 2.)),
            gpui_kit::Modifiers::none(),
        );
        native.update(draw);
        native.update(draw);
        assert_eq!(open_now(&view, &mut native), None, "a click beside {name}");
    }
}

#[gpui_kit::test]
fn a_menu_keeps_the_keys_that_move_inside_it(cx: &mut TestAppContext) {
    use crate::runtime::notify::{CenterHandle, Permission, Settings};
    let (view, mut native) = open(gate::desk(), cx);
    native.update(draw);
    show(&view, &mut native, Some(Overlay::Menu(Popover::Account)));
    native.update(|window, cx| press("menubar", "rail-notifications", window, cx));
    native.update(draw);
    native.update(draw);
    let bell = Overlay::Menu(Popover::Notifications);
    assert_eq!(open_now(&view, &mut native), Some(bell));
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Notifications"),
            "{:?}",
            focused_node(window)
        )
    });

    let mut seed = gate::desk();
    let center = CenterHandle::default();
    seed.center = center.clone();
    let silent = Settings {
        banners: true,
        in_front: false,
        burst: 6,
        views: [("chat".to_owned(), Permission::Silent)].into(),
        unasked: None,
    };
    center.lock().post(
        &silent,
        "chat",
        "Chat",
        view_wire::methods::Notification {
            title: "Ada".into(),
            body: "look".into(),
            tag: String::new(),
            link: String::new(),
            ..Default::default()
        },
        std::time::Instant::now(),
        crate::runtime::notify::wall(),
    );
    let (view, mut native) = open((seed, bell), cx);
    native.update(draw);
    native.update(draw);
    native.update(|window, cx| press("notifications", "notif-mark-all", window, cx));
    native.update(draw);
    native.update(draw);
    assert_eq!(open_now(&view, &mut native), Some(bell));
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Notifications"),
            "{:?}",
            focused_node(window)
        )
    });

    native.update(|window, cx| press("notifications", "notif-settings", window, cx));
    native.update(draw);
    native.update(draw);
    assert_eq!(
        open_now(&view, &mut native),
        Some(Overlay::Settings(SettingsPage::Notifications))
    );
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Settings"),
            "{:?}",
            focused_node(window)
        )
    });
}

#[gpui_kit::test]
fn tab_scrolls_a_settings_row_below_the_fold_into_view(cx: &mut TestAppContext) {
    let notifications = || (gate::desk(), Overlay::Settings(SettingsPage::Notifications));
    let (_view, mut native) = open(notifications(), cx);
    native.simulate_resize(size(px(1280.), px(360.)));
    gate::passes(&mut native, "settings-notifications", false);

    let (_view, mut native) = open(notifications(), cx);
    native.update(draw);
    native.simulate_keystrokes("tab");
    let (keys, nodes) = native.update(|window, cx| {
        draw(window, cx);
        let nodes = crate::ax::snapshot("shell", window, true);
        (window.focused(cx), serde_json::to_value(nodes).unwrap())
    });
    let words = find(&nodes, "Label", "Burst limit");
    let at = |n: usize| words["bounds"][n].as_f64().unwrap() as f32;
    native.simulate_click(
        gpui_kit::point(px(at(0) + 4.), px(at(1) + 4.)),
        gpui_kit::Modifiers::none(),
    );
    native.update(|window, cx| {
        draw(window, cx);
        assert!(keys.is_some());
        assert_eq!(window.focused(cx), keys, "a press on a row took the keys");
    });
}

fn bar_button(native: &mut VisualTestContext, id: &str) -> gpui_kit::Point<Pixels> {
    let nodes = native.update(|window, cx| {
        draw(window, cx);
        serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
    });
    let id = format!("shell:{id}");
    let button = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("missing {id} in {nodes}"));
    let at = |n: usize| button["bounds"][n].as_f64().unwrap() as f32;
    gpui_kit::point(px((at(0) + at(2)) / 2.), px((at(1) + at(3)) / 2.))
}

#[gpui_kit::test]
fn a_menu_left_by_tab_does_not_pull_the_keys_back(cx: &mut TestAppContext) {
    for (overlay, role, name) in MENUS {
        let (view, mut native) = open(gate::desk(), cx);
        let root = native.update(|window, cx| {
            draw(window, cx);
            view.read(cx).focus.clone()
        });
        show(&view, &mut native, Some(overlay));
        native.update(|window, _| assert!(focus_inside(window, role, name), "{name} took no keys"));
        let mut presses = 0;
        while open_now(&view, &mut native).is_some() && presses < 30 {
            native.simulate_keystrokes("tab");
            presses += 1;
        }
        assert_eq!(open_now(&view, &mut native), None, "tab out of {name}");
        let went = native.update(|window, cx| window.focused(cx));
        native.update(draw);
        native.update(draw);
        native.update(|window, cx| {
            draw(window, cx);
            assert!(
                went.is_some() && went != Some(root),
                "{name}: the keys went nowhere"
            );
            assert_eq!(window.focused(cx), went, "{name}: the close moved the keys");
            let (role, focused) = focused_node(window).expect("nothing has the keys");
            assert!(
                role == Role::Button || role == Role::Tab,
                "{name}: the keys went to {role:?} {focused}"
            );
        });
    }
}

#[gpui_kit::test]
fn opening_a_menu_does_not_close_it(cx: &mut TestAppContext) {
    for (overlay, role, name) in MENUS {
        let (view, mut native) = open(gate::desk(), cx);
        super::activate(&mut native);
        native.update(draw);
        show(&view, &mut native, Some(overlay));
        native.run_until_parked();
        native.update(draw);
        assert_eq!(
            open_now(&view, &mut native),
            Some(overlay),
            "{name} closed as it opened"
        );
        native.update(|window, _| {
            assert!(
                focus_inside(window, role, name),
                "{name}: {:?} has the keys",
                focused_node(window)
            )
        });
    }
}

#[gpui_kit::test]
fn a_press_on_the_menus_own_button_toggles_it_once(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    let node = bar_button(&mut native, "rail-connection");
    let press = |native: &mut VisualTestContext, at| {
        native.simulate_click(at, gpui_kit::Modifiers::none());
        native.update(draw);
        native.update(draw);
    };
    let menu = Some(Overlay::Menu(Popover::Node));
    press(&mut native, node);
    assert_eq!(open_now(&view, &mut native), menu, "the first press");
    press(&mut native, node);
    assert_eq!(open_now(&view, &mut native), None, "the second press");
    press(&mut native, node);
    assert_eq!(open_now(&view, &mut native), menu, "the third press");
    let account = bar_button(&mut native, "rail-account");
    press(&mut native, account);
    assert_eq!(
        open_now(&view, &mut native),
        Some(Overlay::Menu(Popover::Account)),
        "a press on the next menu's button"
    );
}

#[gpui_kit::test]
fn the_bells_settings_row_opens_the_notifications_page(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    native.update(draw);
    show(
        &view,
        &mut native,
        Some(Overlay::Settings(SettingsPage::About)),
    );
    show(&view, &mut native, None);
    show(
        &view,
        &mut native,
        Some(Overlay::Menu(Popover::Notifications)),
    );
    native.update(|window, cx| press("notifications", "notif-settings", window, cx));
    native.update(draw);
    native.run_until_parked();
    native.update(draw);
    assert_eq!(
        open_now(&view, &mut native),
        Some(Overlay::Settings(SettingsPage::Notifications))
    );
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Settings"),
            "{:?}",
            focused_node(window)
        )
    });
    let nodes = native.update(draw);
    find(&nodes, "Switch", "Desktop banners");
    find(&nodes, "RadioGroup", "Burst limit");
}

#[gpui_kit::test]
fn a_menu_hangs_four_pixels_under_the_bar_at_every_scale(cx: &mut TestAppContext) {
    for (overlay, role, name) in [
        (Overlay::Network, Role::Menu, "Networks"),
        (Overlay::Menu(Popover::Node), Role::Dialog, "Node status"),
        (
            Overlay::Menu(Popover::Notifications),
            Role::Dialog,
            "Notifications",
        ),
    ] {
        let (_view, mut native) = open((gate::desk(), overlay), cx);
        for scale in [1., 2.] {
            let window = native.update(|window, _| window.window_handle());
            native.simulate_window_scale_factor_change(window, scale);
            let nodes = native.update(|window, cx| {
                draw(window, cx);
                serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
            });
            let card = find(&nodes, &format!("{role:?}"), name);
            assert_eq!(
                card["bounds"][1], 40,
                "{name} at {scale}x: {}",
                card["bounds"]
            );
            if overlay == Overlay::Menu(Popover::Notifications) && scale == 1. {
                let footer = find(&nodes, "Button", "Notification settings");
                assert_eq!(footer["bounds"][1], 221, "{}", footer["bounds"]);
            }
        }
    }
}

fn ask(
    view: &Entity<WindowRoot>,
    native: &mut VisualTestContext,
    program: &'static str,
    said: &str,
) -> consent::Told {
    let words = consent::Words {
        title: consent::ACCOUNT,
        said: said.into(),
        id: None,
        shown: None,
        described: false,
        card: Default::default(),
    };
    let told = consent::queue(program, 0, words).expect("queued");
    sync(view, native);
    told
}

/// The console's card synced to the consent queue (`Windows::sync_consent`),
/// then the draw that shows it and the one after the keys enter it.
fn sync(view: &Entity<WindowRoot>, native: &mut VisualTestContext) {
    let app = super::super::layers::tests::entities(view, native);
    app.windows
        .update(native, |windows, cx| windows.sync_consent(cx));
    native.run_until_parked();
    native.update(draw);
    native.update(draw);
}

fn shows(native: &mut VisualTestContext, said: &str) -> bool {
    let nodes = native.update(|window, cx| {
        draw(window, cx);
        serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
    });
    nodes
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["name"] == said)
}

fn approved(told: &mut consent::Told) -> bool {
    futures::FutureExt::now_or_never(told.answer()) == Some(Some(true))
}

fn key(native: &mut VisualTestContext, down: bool, is_held: bool) {
    let keystroke = gpui_kit::Keystroke::parse("enter").unwrap();
    if down {
        native.simulate_event(gpui_kit::KeyDownEvent {
            keystroke,
            is_held,
            prefer_character_input: false,
        });
    } else {
        native.simulate_event(gpui_kit::KeyUpEvent { keystroke });
    }
    native.update(draw);
}

fn second_click(native: &mut VisualTestContext, at: gpui_kit::Point<Pixels>) {
    let (button, modifiers) = (gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());
    native.simulate_event(gpui_kit::MouseDownEvent {
        button,
        position: at,
        modifiers,
        click_count: 2,
        first_mouse: false,
    });
    native.simulate_event(gpui_kit::MouseUpEvent {
        button,
        position: at,
        modifiers,
        click_count: 2,
    });
    native.update(draw);
}

/// Enter held down since before the card opened (pressed on the desk, the
/// card opening under it and taking the keys, the key repeating, released
/// on the card) does not approve it: that press did not begin on the card.
/// A whole press on Approve after it opened does.
#[gpui_kit::test]
fn enter_held_from_before_the_card_opened_does_not_approve_it(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    native.update(draw);
    key(&mut native, true, false);
    let mut told = ask(&view, &mut native, "chat", "chat asks to suspend agent #3.");
    native.update(|window, _| {
        assert_eq!(
            focused_node(window),
            Some((Role::Dialog, consent::ACCOUNT.to_owned())),
            "the card took the keys"
        );
    });
    for _ in 0..3 {
        key(&mut native, true, true);
    }
    key(&mut native, false, false);
    assert!(consent::front().is_some(), "a held key answered the card");
    assert!(shows(&mut native, "chat asks to suspend agent #3."));
    key(&mut native, true, false);
    key(&mut native, false, false);
    assert!(
        consent::front().is_some(),
        "an Enter on the card itself answered it"
    );
    native.simulate_keystrokes("tab");
    native.update(draw);
    native.update(|window, _| {
        assert_eq!(
            focused_node(window),
            Some((Role::Button, "Approve".to_owned())),
            "Tab reaches Approve"
        );
    });
    key(&mut native, true, false);
    key(&mut native, false, false);
    assert!(approved(&mut told), "an Enter on Approve approves it");
}

#[gpui_kit::test]
fn approve_takes_no_click_that_carries_on_a_count_or_enter_on_the_card(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    let said = "Program chat asks your key to vote in the validators' program.";
    let mut told = consent::queue(
        "chat",
        0,
        consent::Words {
            title: consent::NETWORK,
            said: said.into(),
            id: None,
            shown: None,
            described: true,
            card: Default::default(),
        },
    )
    .expect("queued");
    sync(&view, &mut native);
    assert!(shows(&mut native, said));
    let at = bar_button(&mut native, "consent-approve");
    let none = gpui_kit::Modifiers::none();
    second_click(&mut native, at);
    key(&mut native, true, false);
    key(&mut native, false, false);
    assert!(
        consent::front().is_some(),
        "a second click of a count, or an Enter on the card, approved it"
    );
    native.simulate_click(at, none);
    assert!(approved(&mut told), "a click of its own approves it");
}

#[gpui_kit::test]
fn approve_stays_on_screen_under_a_long_description(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    native.simulate_resize(size(px(1280.), px(400.)));
    super::activate(&mut native);
    let fields: Vec<String> = (0..6)
        .map(|n| format!("field{n} {}", "x".repeat(150)))
        .collect();
    let _told = consent::queue(
        "chat",
        0,
        consent::Words {
            title: consent::ACCOUNT,
            said: "Program chat asks your key to sign an identity operation.".into(),
            id: None,
            shown: Some(format!("Add key\n{}", fields.join(" · "))),
            described: true,
            card: Default::default(),
        },
    )
    .expect("queued");
    sync(&view, &mut native);
    let at = bar_button(&mut native, "consent-approve");
    assert!(at.y < px(400. - 22.), "Approve below the window: {at:?}");
}

#[gpui_kit::test]
fn a_card_for_the_network_says_so_and_shows_the_description(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    let shown = "Schedule · chat\nchange Set · height 2442";
    let told = consent::queue(
        "explorer",
        0,
        consent::Words {
            title: consent::NETWORK,
            said: "Program explorer asks your key to sign an op on the programs role. \
                   Approve only if you meant to."
                .into(),
            id: Some(8..16),
            shown: Some(shown.into()),
            described: true,
            card: Default::default(),
        },
    )
    .expect("queued");
    sync(&view, &mut native);
    assert!(shows(&mut native, consent::NETWORK), "the network's title");
    assert!(!shows(&mut native, consent::ACCOUNT));
    assert!(shows(&mut native, shown), "the description");
    drop(told);
    sync(&view, &mut native);
}

#[gpui_kit::test]
fn a_click_begun_before_the_card_opened_does_not_approve_it(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    let told = ask(&view, &mut native, "chat", "chat asks to suspend agent #3.");
    let at = bar_button(&mut native, "consent-approve");
    drop(told);
    sync(&view, &mut native);
    assert!(!shows(&mut native, "chat asks to suspend agent #3."));
    let none = gpui_kit::Modifiers::none();
    native.simulate_mouse_down(at, gpui_kit::MouseButton::Left, none);
    native.update(draw);
    let mut told = ask(&view, &mut native, "chat", "chat asks to revoke agent #4.");
    native.simulate_mouse_up(at, gpui_kit::MouseButton::Left, none);
    native.update(draw);
    assert!(
        consent::front().is_some(),
        "a press from before answered it"
    );
    native.simulate_click(at, none);
    assert!(approved(&mut told), "a click on the card approves it");
}

#[gpui_kit::test]
fn a_withdrawn_asks_card_gives_way_to_a_new_card(cx: &mut TestAppContext) {
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    let (first, second, third) = (
        "chat asks to suspend agent #3.",
        "forge asks to revoke agent #4.",
        "members asks to suspend agent #5.",
    );
    let told = ask(&view, &mut native, "chat", first);
    let second_told = ask(&view, &mut native, "forge", second);
    let mut third_told = ask(&view, &mut native, "members", third);
    assert!(shows(&mut native, first));
    let at = bar_button(&mut native, "consent-approve");
    let none = gpui_kit::Modifiers::none();

    native.simulate_mouse_down(at, gpui_kit::MouseButton::Left, none);
    native.update(draw);
    drop(told);
    sync(&view, &mut native);
    assert!(shows(&mut native, second), "the next ask's card");
    assert!(!shows(&mut native, first));
    native.simulate_mouse_up(at, gpui_kit::MouseButton::Left, none);
    native.update(draw);
    assert!(
        consent::front().is_some_and(|(_, words)| words.said == second),
        "a click begun on the old card answered the new one"
    );

    native.simulate_keystrokes("tab");
    native.update(draw);
    native.update(|window, _| {
        assert_eq!(
            focused_node(window),
            Some((Role::Button, "Approve".to_owned()))
        );
    });
    key(&mut native, true, false);
    drop(second_told);
    sync(&view, &mut native);
    assert!(shows(&mut native, third), "the next ask's card");
    key(&mut native, false, false);
    assert!(
        consent::front().is_some_and(|(_, words)| words.said == third),
        "an Enter begun on the old card answered the new one"
    );

    native.simulate_click(at, none);
    assert!(approved(&mut third_told), "a click on the card approves it");
}

/// Under touch the card is the phone's confirm sheet (mobile
/// `src/kit/Confirm.tsx`): docked to the window's bottom, the ask's title
/// as its eyebrow, "Suspend: agent #12", the program, the agent and who
/// signs, what it takes away, Cancel and Suspend. A second click of a count
/// is no yes, as on the desktop card; Cancel refuses it.
#[gpui_kit::test]
fn under_touch_the_card_is_the_phones_confirm_sheet(cx: &mut TestAppContext) {
    ducktape_host::contract::testing::touch_first_for_test(true);
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    let suspend = abi::encode(&identity::Op::Suspend { account: 12 });
    let words = consent::needed("chat", None, identity::PROGRAM, &suspend, Some(7)).unwrap();
    let mut told = consent::queue("chat", 0, words).expect("queued");
    sync(&view, &mut native);
    for said in [
        "Suspend: agent #12",
        "Program",
        "identity",
        "Agent",
        "#12",
        "Signs",
        "account 7",
        "Agent #12 stops acting until it is resumed.",
        "Suspend",
        "Cancel",
    ] {
        assert!(shows(&mut native, said), "{said}");
    }
    let nodes = native.update(|window, cx| {
        draw(window, cx);
        serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
    });
    find(&nodes, "Label", consent::ACCOUNT);
    assert!(!shows(&mut native, &consent::ACCOUNT.to_uppercase()));
    let card = find(&nodes, "Dialog", consent::ACCOUNT);
    let high = native.update(|window, _| f32::from(window.viewport_size().height));
    assert_eq!(
        card["bounds"][3].as_f64().unwrap() as f32,
        high,
        "docked to the bottom: {}",
        card["bounds"]
    );
    let none = gpui_kit::Modifiers::none();
    let at = bar_button(&mut native, "consent-approve");
    second_click(&mut native, at);
    assert!(
        consent::front().is_some(),
        "a second click of a count approved it"
    );
    let at = bar_button(&mut native, "consent-cancel");
    native.simulate_click(at, none);
    assert!(consent::front().is_none(), "Cancel answered it");
    assert!(!approved(&mut told), "and refused it");
}
