//! Whatever opens over the desk takes the keys as it opens, and gives them
//! back as it closes (docs/ax.md §4 gap 5): the dialogs on a scrim and the
//! menus hanging from the bar alike, and one giving way to the next.
use super::*;
use crate::{Overlay, Popover};
use gpui_kit::Role;

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
