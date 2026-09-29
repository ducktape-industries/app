//! AX-tree coverage for the shell's own screens: a native input reports its
//! current text as the AX value (a private one masked by the door, a
//! password's none), an error alert carries a name (`aria_label`), and every screen
//! state passes the phase-1 audit ([`gate`]).
use super::*;

mod combo;
mod fields;
mod gate;
mod launcher;
mod live;
mod names;
mod overlays;
mod text;
use gpui_kit::accesskit::{Action, ActionData, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};

fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    window.render_frame(cx);
    serde_json::to_value(crate::ax::snapshot("shell", window, false)).unwrap()
}

/// Sends the door's own SetValue action to the node whose element is
/// `field` (`"{key}/field"`), the way a real AX client types into it.
fn type_into(field: &str, text: &str, window: &mut Window, cx: &mut gpui_kit::App) {
    draw(window, cx);
    let target = window
        .a11y_tree()
        .unwrap()
        .nodes
        .iter()
        .find_map(|(node, _)| {
            window
                .a11y_element_id(*node)
                .is_some_and(|path| {
                    path.last().is_some_and(|element| {
                        matches!(element, ElementId::Name(name) if name.as_ref() == field)
                    })
                })
                .then_some(*node)
        })
        .unwrap_or_else(|| panic!("missing AX control {field}"));
    window.dispatch_a11y_action(
        ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: Some(ActionData::Value(text.into())),
        },
        cx,
    );
}

/// An assistive technology's press (AccessKit's Click) on the node whose
/// element is `id` inside the element `within`: the path the door's
/// `/act press` takes.
fn press(within: &str, id: &str, window: &mut Window, cx: &mut gpui_kit::App) {
    draw(window, cx);
    let named = |element: &ElementId, want: &str| matches!(element, ElementId::Name(name) if name.as_ref() == want);
    let target = window
        .a11y_tree()
        .unwrap()
        .nodes
        .iter()
        .find_map(|(node, _)| {
            window
                .a11y_element_id(*node)
                .is_some_and(|path| {
                    path.last().is_some_and(|element| named(element, id))
                        && path.iter().any(|element| named(element, within))
                })
                .then_some(*node)
        })
        .unwrap_or_else(|| panic!("missing AX control {id} in {within}"));
    window.dispatch_a11y_action(
        ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: None,
        },
        cx,
    );
}

fn find<'a>(nodes: &'a serde_json::Value, role: &str, name: &str) -> &'a serde_json::Value {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["role"] == role && node["name"] == name)
        .unwrap_or_else(|| panic!("missing {role} {name:?} in {nodes}"))
}

pub(super) fn open(
    state: Ducktape,
    cx: &mut TestAppContext,
) -> (Entity<DesktopWindow>, VisualTestContext) {
    // a state that has a console window (with a desk laid out for it) is drawn in it
    let key = state.console_win.unwrap_or_else(WindowKey::unique);
    let model = cx.new(|cx| Desktop::new(state, crate::tray::init(cx).0));
    let mut view = None;
    let handle = cx.open_window(size(px(1280.), px(800.)), |window, cx| {
        let desktop =
            cx.new(|cx| DesktopWindow::new(model.clone(), key, WindowKind::Console, window, cx));
        view = Some(desktop.clone());
        gpui_kit::component::Root::new(desktop, window, cx)
    });
    let view = view.unwrap();
    model.update(cx, |model, _| {
        model.windows.insert(key, handle.into());
        model.views.insert(key, view.downgrade());
    });
    (view, VisualTestContext::from_window(handle.into(), cx))
}

#[gpui_kit::test]
fn connect_screen_exposes_the_endpoint_and_names_its_error(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.endpoint = "127.0.0.1:9000".to_string();
    state.endpoint_error = "no route to host".to_string();
    let (_view, mut native) = open(state, cx);

    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Node address")["value"],
        "127.0.0.1:9000"
    );
    assert_eq!(
        find(&nodes, "Alert", "no route to host")["name"],
        "no route to host"
    );

    // The value tracks what is actually typed, not just the seed the field
    // was constructed with.
    native.update(|window, cx| type_into("endpoint/field", "10.0.0.5:8844", window, cx));
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Node address")["value"],
        "10.0.0.5:8844"
    );
}

#[gpui_kit::test]
fn sign_in_screens_keep_secret_fields_out_of_the_ax_value(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Unlock(Default::default());
    // a password-locked key from before: its password is asked once
    state.key_exists = true;
    state.sign_in.unlock_error = "wrong password".to_string();
    let (view, mut native) = open(state, cx);

    // Typing into either field clears the error (screens.rs's own UX), so
    // it has to be checked before that, not folded into the same draw.
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "Alert", "wrong password")["name"],
        "wrong password"
    );

    native.update(|window, cx| type_into("password/field", "hunter2", window, cx));
    let nodes = native.update(draw);
    let password = find(&nodes, "PasswordInput", "Password");
    // masked on the screen, so no value at all: the door's mask is not the
    // platform's
    assert!(
        password.get("value").is_none(),
        "password value leaked into the AX tree: {password}"
    );

    // The recovery-phrase input is not visually masked (it is not a
    // PasswordInput), but its typed text must leave the process masked too.
    let model = native.update(|_, cx| view.read(cx).model.clone());
    model.update(cx, |model, _| {
        model.state.signer_key = "ab".into();
        model.state.stage = Stage::Recover(Default::default());
    });
    native.update(|window, cx| {
        type_into(
            "restore-phrase/field",
            "abandon abandon abandon",
            window,
            cx,
        )
    });
    let nodes = native.update(draw);
    let phrase = find(&nodes, "TextInput", "Recovery key");
    assert_eq!(
        phrase["value"], "•••",
        "recovery phrase leaked into the AX tree: {phrase}"
    );
    // a Label is named by its value now: no node anywhere in the tree may
    // carry the words, not the field's own text either
    assert!(
        !nodes.to_string().contains("abandon"),
        "recovery phrase leaked somewhere in the AX tree: {nodes}"
    );

    // The new key's sheet: its words are the sheet's name, masked; no other
    // node carries them.
    model.update(cx, |model, _| {
        model.state.stage = Stage::Phrase(crate::ui::Phrase {
            words: crate::Secret::from(String::from("canoe pond forest")),
            ..Default::default()
        });
    });
    let nodes = native.update(draw);
    assert!(
        !nodes.to_string().contains("canoe"),
        "new recovery phrase leaked into the AX tree: {nodes}"
    );
}

/// The key and the account are two steps: the key screen is only about
/// this device's key (no passkey there), and the account step, once a key
/// is unlocked, offers creating an account or adding this device to one.
#[gpui_kit::test]
fn the_key_step_asks_nothing_about_accounts_and_the_account_step_does(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Unlock(Default::default());
    let (view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    find(&nodes, "Button", "Read without a key");
    let passkeys = nodes
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| {
            node["name"]
                .as_str()
                .is_some_and(|name| name.contains("passkey"))
        })
        .count();
    assert_eq!(passkeys, 0, "the key screen offered a passkey: {nodes}");

    let model = native.update(|_, cx| view.read(cx).model.clone());
    model.update(cx, |model, _| {
        model.state.signer_key = "ab".into();
        model.state.stage = Stage::Account(Default::default());
    });
    native.update(|window, cx| type_into("create-account-name/field", "duck", window, cx));
    let nodes = native.update(draw);
    assert_eq!(find(&nodes, "TextInput", "Account name")["value"], "duck");
    gate::passes(&mut native, "account-step-typed", true);
    find(&nodes, "Button", "Create account");
    find(&nodes, "Button", "Add this device from another device");
    find(&nodes, "Button", "a passkey");
    find(&nodes, "Button", "a recovery key");
    find(&nodes, "Button", "Create with a passkey");
}

/// The Connect screen's "Recent" list had no tab stop at all: a
/// keyboard-only reader could see a previously used node but never reach it
/// (or its "Forget" button) with Tab, only a mouse.
#[gpui_kit::test]
fn recent_endpoint_rows_and_their_forget_buttons_are_tab_reachable(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Connect;
    state.recent_endpoints = vec![crate::backend::RecentEndpoint {
        url: "http://127.0.0.1:9000".to_string(),
        network: "testkit".to_string(),
        ..Default::default()
    }];
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    for id in [
        "shell:recent/http://127.0.0.1:9000",
        "shell:forget/http://127.0.0.1:9000",
    ] {
        let node = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("missing {id}: {nodes}"));
        assert!(
            node["actions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|a| a == "focus")),
            "{id} has no tab stop, so a keyboard-only reader can never reach it: {node}"
        );
    }
}

/// A screen change (Connect → sign-in, the way `ConnectSubmit` does once the
/// node answers) unmounts whatever the reader had focused. Before the fix,
/// the window's `on_focus_lost` handler called `window.blur`, dropping
/// focus for good: every later Tab was silently swallowed (there is no
/// dispatch path with nothing focused), a full keyboard trap. It must
/// instead fall back to the window's own root, the same handle a fresh
/// window starts focused on, so Tab still reaches the new screen.
#[gpui_kit::test]
fn a_screen_change_that_unmounts_the_focused_control_refocuses_the_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Connect;
    let (view, mut native) = open(state, cx);
    native.update(draw);
    native.update(|window, cx| {
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("tab").unwrap(), cx)
    });
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Node address")["state"],
        serde_json::json!(["focused"]),
        "sanity: tab reaches the endpoint field"
    );

    // ConnectSubmit swaps Connect for sign-in once the node answers: the
    // endpoint field the reader was on is gone.
    let model = native.update(|_, cx| view.read(cx).model.clone());
    model.update(cx, |model, _| {
        model.state.stage = Stage::Unlock(Default::default())
    });
    native.update(draw);

    // A keyboard-only reader's next Tab must still land somewhere on the
    // new screen, not be swallowed because the window itself went blurred.
    native.update(|window, cx| {
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("tab").unwrap(), cx)
    });
    let nodes = native.update(draw);
    assert!(
        nodes.as_array().unwrap().iter().any(|node| {
            node["state"]
                .as_array()
                .is_some_and(|states| states.iter().any(|s| s == "focused"))
        }),
        "Tab after a screen change should reach a control, not be swallowed: {nodes}"
    );
}

/// Shift+Tab leaves a text field for the stop before it, and Tab comes
/// back. The node that names the field (`a11y::text_field`) and the kit's
/// input inside it track one focus handle; gpui's tab order must hold it
/// once, or Shift+Tab lands on the handle's other entry, the field itself.
#[gpui_kit::test]
fn shift_tab_leaves_a_text_field_and_tab_comes_back(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Connect;
    let (_view, mut native) = open(state, cx);
    native.update(draw);
    let mut press = |keys: &str| {
        native.simulate_keystrokes(keys);
        native.update(|window, cx| {
            let nodes = draw(window, cx);
            let field = &find(&nodes, "TextInput", "Node address")["state"];
            (window.focused(cx), *field == serde_json::json!(["focused"]))
        })
    };
    let (field, on_field) = press("tab");
    assert!(on_field, "sanity: tab reaches the endpoint field");
    let (keys, on_field) = press("shift-tab");
    assert!(
        keys.is_some() && keys != field && !on_field,
        "shift-tab stays on the endpoint field"
    );
    let (keys, on_field) = press("tab");
    assert!(
        keys == field && on_field,
        "tab comes back to the endpoint field"
    );
}

#[test]
fn initials_take_the_first_letter_of_two_words() {
    use super::screens::initials;
    assert_eq!(initials("Ada Lovelace King"), "AL");
    assert_eq!(initials("ada"), "A");
    assert_eq!(initials("  "), "?");
    assert_eq!(initials("émile zola"), "ÉZ");
    // a word's first letter or digit, not its punctuation
    assert_eq!(initials("Dev #7"), "D7");
    assert_eq!(initials("# ?"), "?");
}

/// The field state outlives the model's copy of a secret: after "Read
/// without a key" the model's password is empty, and the password
/// field used to go on showing the old dots — while a retry sent nothing.
/// The field mirrors the model on every draw.
#[gpui_kit::test]
fn a_password_the_model_wiped_leaves_the_field_empty(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Unlock(Default::default());
    state.key_exists = true;
    let (view, mut native) = open(state, cx);
    native.update(|window, cx| type_into("password/field", "hunter22", window, cx));
    native.update(draw);
    let field = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            view.read(cx).inputs["password"]
                .state
                .read(cx)
                .value()
                .to_string()
        })
    };
    assert_eq!(field(&mut native), "hunter22");
    let model = native.update(|_, cx| view.read(cx).model.clone());
    assert_eq!(
        model.read_with(cx, |model, _| match &model.state.stage {
            Stage::Unlock(step) => step.password.to_string(),
            _ => String::new(),
        }),
        "hunter22"
    );

    model.update(cx, |model, _| {
        // reading without a key wipes it; the key screen comes back
        model.state.update(Message::BrowseWithoutKey);
        model.state.update(Message::SignIn);
    });
    native.update(draw);
    assert_eq!(
        field(&mut native),
        "",
        "the field kept a password the model wiped"
    );
}

/// Keys can land in a field before their change event reaches the model —
/// the AX door's `type` sends a word's keys in one update. A draw in
/// between must not put the model's older text back over them: the door
/// typed "abcdef" into a phrase-check word and the field showed "ef".
#[gpui_kit::test]
fn a_draw_leaves_text_the_model_has_not_heard_yet(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.stage = Stage::Unlock(Default::default());
    state.key_exists = true;
    let (view, mut native) = open(state, cx);
    native.update(draw);
    native.update(|window, cx| {
        let field = view.read(cx).inputs["password"].state.clone();
        // set_value emits no change: the model has not heard of this text.
        field.update(cx, |field, cx| field.set_value("abcdef", window, cx));
    });
    native.update(draw);
    let shown = native.update(|_, cx| {
        view.read(cx).inputs["password"]
            .state
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(shown, "abcdef", "a draw wiped keys the model had not heard");
}

/// The rail's network name is the switcher: a button that says which
/// network is in hand and whether its menu is open, and the menu lists
/// every node reached — the current one checked, a different chain that
/// shares its name marked — plus the way to add another.
#[gpui_kit::test]
fn the_network_switcher_names_the_network_and_its_menu_marks_the_current_one(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    // its own roster, not the app's one every test shares
    state.roster = Default::default();
    state.stage = Stage::Desk;
    state.connected = true;
    state.network = "testkit".into();
    state.connected_rpc = "http://127.0.0.1:1".into();
    state.recent_endpoints = vec![
        crate::backend::RecentEndpoint {
            url: "http://127.0.0.1:1".into(),
            network: "testkit".into(),
            founded: 1,
            other_chain: false,
        },
        crate::backend::RecentEndpoint {
            url: "http://127.0.0.1:2".into(),
            network: "testkit".into(),
            founded: 2,
            other_chain: true,
        },
    ];
    state.overlay = Some(crate::Overlay::Network);
    let (_view, mut native) = open(state, cx);
    let nodes = native.update(draw);
    let switcher = find(&nodes, "Button", "Network: testkit");
    assert_eq!(switcher["id"], "shell:network-switcher");
    find(&nodes, "Menu", "Networks");
    find(&nodes, "MenuItemRadio", "testkit · 127.0.0.1:1 (current)");
    find(
        &nodes,
        "MenuItemRadio",
        "testkit · 127.0.0.1:2 · different network",
    );
    find(&nodes, "MenuItem", "Add a network");
}

/// The bell's panel: calm when there is nothing, and once notices land a
/// row each (a fold's count in its name), the unread count on the bell and
/// in the header, and Settings opening on Notifications.
#[gpui_kit::test]
fn the_notification_centre_lists_rows_under_the_bell(cx: &mut TestAppContext) {
    use crate::runtime::notify::{CenterHandle, Permission, Settings};
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    // its own centre and roster, not the app's ones every test shares
    let center = CenterHandle::default();
    state.center = center.clone();
    state.roster = Default::default();
    state.stage = Stage::Desk;
    state.connected = true;
    state.network = "testkit".into();
    state.overlay = Some(crate::Overlay::Menu(crate::Popover::Notifications));
    let (view, mut native) = open(state, cx);

    let nodes = native.update(draw);
    find(&nodes, "Button", "Notifications");
    find(&nodes, "Dialog", "Notifications");
    find(&nodes, "Status", "You’re all caught up");

    let silent = Settings {
        banners: true,
        in_front: false,
        burst: 6,
        views: [("chat".to_owned(), Permission::Silent)].into(),
    };
    let now = crate::runtime::notify::wall();
    let post = |title: &str, tag: &str| view_wire::methods::Notification {
        title: title.into(),
        body: "@grace look".into(),
        tag: tag.into(),
        link: "duck://chat/design".into(),
    };
    {
        let mut center = center.lock();
        let at = std::time::Instant::now();
        center.post(
            &silent,
            "chat",
            "Chat",
            post("Ada mentioned you", "#design"),
            at,
            now,
        );
        center.post(
            &silent,
            "chat",
            "Chat",
            post("Ada mentioned you", "#design"),
            at,
            now,
        );
        center.post(
            &silent,
            "chat",
            "Chat",
            post("Lin", "direct"),
            at,
            now - 3 * 86_400,
        );
    }
    view.update(&mut native, |_, cx| cx.notify());
    let nodes = native.update(draw);
    find(&nodes, "Button", "Notifications, 2 unread");
    // the count on the bell is announced as notices land
    let heard = native.update(|window, _| live::announced(window));
    assert!(
        heard.iter().any(|(role, name, value, live)| {
            *role == gpui_kit::Role::Status
                && name == "2 unread notifications"
                && value.as_deref() == Some(name.as_str())
                && *live == Some(gpui_kit::accesskit::Live::Polite)
        }),
        "{heard:?}"
    );
    let row = find(&nodes, "MenuItem", "Unread. Ada mentioned you: @grace look");
    // when and from where, after the name
    assert_eq!(row["description"], "now · chat · #design");
    find(&nodes, "MenuItem", "Unread. Lin: @grace look");
    find(&nodes, "Button", "Mark all read");
    assert!(!nodes.to_string().contains("all caught up"));
    // the panel opened on them: its keys at its first control, as the bell
    // gives them, and a Tab past its last closes it (a menu)
    for overlay in [
        None,
        Some(crate::Overlay::Menu(crate::Popover::Notifications)),
    ] {
        view.update(&mut native, |view, cx| {
            view.model.update(cx, |model, cx| {
                model.state.overlay = overlay;
                cx.notify();
            })
        });
        native.update(draw);
        native.update(draw);
    }
    gate::passes(&mut native, "notifications-menu-unread", false);

    view.update(&mut native, |view, cx| {
        view.model.update(cx, |model, cx| {
            model.dispatch(Message::NotifyMarkAllRead, cx)
        })
    });
    let nodes = native.update(draw);
    find(&nodes, "Button", "Notifications");
    // Settings is modal: the snapshot is its subtree, the bell is behind it
    view.update(&mut native, |view, cx| {
        view.model
            .update(cx, |model, cx| model.dispatch(Message::NotifySettings, cx))
    });
    let nodes = native.update(draw);
    find(&nodes, "Switch", "Desktop banners");
    find(&nodes, "RadioGroup", "Burst limit");
}

/// A press inside an overlay's card stays inside: the card offers no press
/// of its own (AX-012), and the backdrop's click does not close it.
#[gpui_kit::test]
fn a_click_inside_an_overlay_card_leaves_it_open(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    // one under a scrim, one hanging from the bar
    for (overlay, name) in [
        (crate::Overlay::Settings, "Settings"),
        (crate::Overlay::Menu(crate::Popover::Node), "Node status"),
    ] {
        let mut state = gate::desk();
        state.overlay = Some(overlay);
        let (view, mut native) = open(state, cx);
        let nodes = native.update(|window, cx| {
            draw(window, cx);
            serde_json::to_value(crate::ax::snapshot("shell", window, true)).unwrap()
        });
        let card = find(&nodes, "Dialog", name);
        let at = |n: usize| card["bounds"][n].as_f64().unwrap() as f32;
        // just inside the card's corner: its padding, no control
        native.simulate_click(
            gpui_kit::point(px(at(0) + 8.), px(at(1) + 8.)),
            gpui_kit::Modifiers::none(),
        );
        let open = native.update(|_, cx| view.read(cx).model.read(cx).state.overlay);
        assert_eq!(open, Some(overlay), "a click inside {name} closed it");
    }
}
