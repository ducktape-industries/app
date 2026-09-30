//! Whatever opens over the desk takes the keys as it opens, and gives them
//! back as it closes (docs/ax.md §4 gap 5): the dialogs on a scrim and the
//! menus hanging from the bar alike, and one giving way to the next. A menu
//! the keys leave closes, and leaves them where they went.
use super::*;
use crate::{Overlay, Popover};
use gpui_kit::{Pixels, Role};

/// Whether `window`'s focused node is the `role` named `name`, or inside it.
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

/// The focused node's role and name, for a failure to say where the keys went.
fn focused_node(window: &Window) -> Option<(Role, String)> {
    let update = window.a11y_tree()?;
    update
        .nodes
        .iter()
        .find(|(id, _)| *id == update.focus)
        .map(|(_, node)| (node.role(), node.label().unwrap_or_default().to_owned()))
}

fn show(view: &Entity<DesktopWindow>, native: &mut VisualTestContext, overlay: Option<Overlay>) {
    view.update(native, |view, cx| {
        view.model.update(cx, |model, cx| {
            model.state.overlay = overlay;
            model.bridge(false, cx);
            cx.notify();
        })
    });
    // the draw that sees it open or shut, then the one after its deferred focus
    native.update(draw);
    native.update(draw);
}

#[gpui_kit::test]
fn every_overlay_takes_the_keys_as_it_opens_and_gives_them_back(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let overlays = [
        (Overlay::Spotlight, Role::Dialog, "Search"),
        (Overlay::Approve, Role::Dialog, "Add a device"),
        (Overlay::Settings, Role::Dialog, "Settings"),
        (Overlay::Network, Role::Menu, "Networks"),
        (Overlay::Menu(Popover::Node), Role::Dialog, "Node status"),
        (Overlay::Menu(Popover::Account), Role::Dialog, "Account"),
        (
            Overlay::Menu(Popover::Notifications),
            Role::Dialog,
            "Notifications",
        ),
    ];
    for (overlay, role, name) in overlays {
        let (view, mut native) = open(gate::desk(), cx);
        native.update(draw);
        // the keys on the bar, where a keyboard user leaves them
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

    // a menu giving way to a dialog hands the keys on; closing gives them
    // back to what had them before the menu
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
    show(&view, &mut native, Some(Overlay::Settings));
    native.update(|window, _| assert!(focus_inside(window, Role::Dialog, "Settings")));
    native.simulate_keystrokes("escape");
    native.update(draw);
    native.update(|window, cx| {
        draw(window, cx);
        assert_eq!(window.focused(cx), before);
    });
}

/// Every menu hanging from the bar.
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

fn open_now(view: &Entity<DesktopWindow>, native: &mut VisualTestContext) -> Option<Overlay> {
    native.update(|_, cx| view.read(cx).model.read(cx).state.overlay)
}

/// A menu closes when the keys leave it, whichever way (the menu pattern;
/// owner, 2026-09-28): Shift+Tab before its first control, Tab past its
/// last. The keys stay where they went. It is no focus trap, also once a
/// dialog on a scrim has been open in the window.
#[gpui_kit::test]
fn a_menu_closes_when_the_keys_leave_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    for (overlay, role, name) in MENUS {
        for (out, presses) in [("shift-tab", 1), ("tab", 30)] {
            let (view, mut native) = open(gate::desk(), cx);
            native.update(draw);
            show(&view, &mut native, Some(Overlay::Settings));
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

/// A click outside a menu closes it: low on the desk, on the backdrop
/// under the menu, which closes it and reaches no window behind; and on
/// the bar where nothing is (its end past the window's edge), which takes
/// the keys to the window, out of the menu.
#[gpui_kit::test]
fn a_click_outside_a_menu_closes_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    for (overlay, _, name) in MENUS {
        let (view, mut native) = open(gate::desk(), cx);
        // gpui reports focus moves only in the active window
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

/// The keys move inside a menu or from one menu to the next and the menu
/// stays: one menu giving way to the next hands them on, and a control
/// that goes (Mark all read, once all is read) leaves them in its menu.
/// The bell's panel still hands off to Settings, the keys in it.
#[gpui_kit::test]
fn a_menu_keeps_the_keys_that_move_inside_it(cx: &mut TestAppContext) {
    use crate::runtime::notify::{CenterHandle, Permission, Settings};
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    // Account, then a click on the bell
    let (view, mut native) = open(gate::desk(), cx);
    native.update(draw);
    show(&view, &mut native, Some(Overlay::Menu(Popover::Account)));
    // a press on the bell, in the frame that placed it
    native.update(|window, cx| press("menubar", "rail-notifications", window, cx));
    native.update(draw);
    native.update(draw);
    let bell = Some(Overlay::Menu(Popover::Notifications));
    assert_eq!(open_now(&view, &mut native), bell);
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Notifications"),
            "{:?}",
            focused_node(window)
        )
    });

    // the bell's panel, one notice unread: Mark all read goes as it works
    let mut state = gate::desk();
    let center = CenterHandle::default();
    state.center = center.clone();
    let silent = Settings {
        banners: true,
        in_front: false,
        burst: 6,
        views: [("chat".to_owned(), Permission::Silent)].into(),
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
        },
        std::time::Instant::now(),
        crate::runtime::notify::wall(),
    );
    state.overlay = bell;
    let (view, mut native) = open(state, cx);
    native.update(draw);
    native.update(draw);
    native.update(|window, cx| press("notifications", "notif-mark-all", window, cx));
    native.update(draw);
    native.update(draw);
    assert_eq!(open_now(&view, &mut native), bell);
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Notifications"),
            "{:?}",
            focused_node(window)
        )
    });

    // and its Notification settings opens Settings, the keys in it
    native.update(|window, cx| press("notifications", "notif-settings", window, cx));
    native.update(draw);
    native.update(draw);
    assert_eq!(open_now(&view, &mut native), Some(Overlay::Settings));
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Settings"),
            "{:?}",
            focused_node(window)
        )
    });
}

/// Settings' page scrolls the row Tab reaches into view: in a short window
/// the Notifications rows run past the dialog's foot, and the walk still
/// finds every stop it reaches on screen (AX-020). A press on a row's words
/// leaves the keys where they were.
#[gpui_kit::test]
fn tab_scrolls_a_settings_row_below_the_fold_into_view(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let notifications = || {
        let mut state = gate::desk();
        state.overlay = Some(Overlay::Settings);
        state.settings_page = crate::ui::SettingsPage::Notifications;
        state
    };
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

/// The middle of the bar button with the id `id`.
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

/// Tab past a menu's last control closes it (M6), and the keys stay where
/// Tab took them: the next control in the walk, not what had them before
/// the menu opened (the window itself, here).
#[gpui_kit::test]
fn tab_past_a_menus_end_closes_it_and_keeps_the_keys_where_they_went(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
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
        // where the closing Tab left them, before any frame ran a handoff
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

/// Opening a menu moves the keys from its handle to its first row: the
/// belt hears the keys leaving the menu, not its handle, so the menu
/// stays open with its first row focused.
#[gpui_kit::test]
fn opening_a_menu_does_not_close_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    for (overlay, role, name) in MENUS {
        let (view, mut native) = open(gate::desk(), cx);
        // gpui reports focus moves only in the active window
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

/// A press on a menu's own bar button toggles it: open, shut, open again;
/// and a press on another menu's button opens that one. Nothing under the
/// bar closes the menu first and swallows the press.
#[gpui_kit::test]
fn a_press_on_the_menus_own_button_toggles_it_once(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
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

/// The bell's Settings row opens Settings as the bell closes: the keys
/// leave the bell's subtree, and the belt, seeing the bell no longer open,
/// leaves Settings alone.
#[gpui_kit::test]
fn the_bells_settings_row_opens_settings_and_the_belt_leaves_it_open(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (view, mut native) = open(gate::desk(), cx);
    super::activate(&mut native);
    native.update(draw);
    show(
        &view,
        &mut native,
        Some(Overlay::Menu(Popover::Notifications)),
    );
    native.update(|window, cx| press("notifications", "notif-settings", window, cx));
    native.update(draw);
    native.run_until_parked();
    native.update(draw);
    assert_eq!(open_now(&view, &mut native), Some(Overlay::Settings));
    native.update(|window, _| {
        assert!(
            focus_inside(window, Role::Dialog, "Settings"),
            "{:?}",
            focused_node(window)
        )
    });
}
