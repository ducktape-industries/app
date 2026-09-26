//! AX-tree coverage for the shell's own screens: a native input reports its
//! current text as the AX value unless it is `secret`, and an error alert
//! carries a name (`aria_label`), the way `src/ax/tree.rs`'s compact filter
//! requires.
use super::*;
use gpui_kit::accesskit::{Action, ActionData, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};

fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    window.render_frame(cx);
    serde_json::to_value(crate::ax::snapshot("shell", window, false)).unwrap()
}

/// Sends the door's own SetValue action to the field named `"{key}/field"`,
/// the way a real AX client types into it.
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
                    path.iter().any(|element| {
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

fn find<'a>(nodes: &'a serde_json::Value, role: &str, name: &str) -> &'a serde_json::Value {
    nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["role"] == role && node["name"] == name)
        .unwrap_or_else(|| panic!("missing {role} {name:?} in {nodes}"))
}

fn open(state: Ducktape, cx: &mut TestAppContext) -> (Entity<DesktopWindow>, VisualTestContext) {
    let model = cx.new(|cx| Desktop::new(state, crate::tray::init(cx).0));
    let key = WindowKey::unique();
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
    assert!(
        password.get("value").is_none(),
        "password value leaked into the AX tree: {password}"
    );

    // The recovery-phrase input is not visually masked (it is not a
    // PasswordInput), but its typed text must stay out of the tree too.
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
    assert!(
        phrase.get("value").is_none(),
        "recovery phrase leaked into the AX tree: {phrase}"
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

#[test]
fn initials_take_the_first_letter_of_two_words() {
    use super::screens::initials;
    assert_eq!(initials("Ada Lovelace King"), "AL");
    assert_eq!(initials("ada"), "A");
    assert_eq!(initials("  "), "?");
    assert_eq!(initials("émile zola"), "ÉZ");
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
    use crate::runtime::notify::{Permission, Settings, center};
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
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
        let mut center = center();
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
    find(&nodes, "MenuItem", "Unread. Ada mentioned you: @grace look");
    find(&nodes, "MenuItem", "Unread. Lin: @grace look");
    find(&nodes, "Button", "Mark all read");
    assert!(!nodes.to_string().contains("all caught up"));

    view.update(&mut native, |view, cx| {
        view.model.update(cx, |model, cx| {
            model.dispatch(Message::NotifyMarkAllRead, cx);
            model.dispatch(Message::NotifySettings, cx);
        })
    });
    let nodes = native.update(draw);
    find(&nodes, "Button", "Notifications");
    find(&nodes, "Switch", "Desktop banners");
    find(&nodes, "RadioGroup", "Burst limit");
    center().clear_read();
}

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
