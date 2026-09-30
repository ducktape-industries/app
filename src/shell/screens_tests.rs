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
mod roving;
mod text;
use super::entities::{Overlay, Popover, SettingsPage};
use gpui_kit::accesskit::{Action, ActionData, ActionRequest, TreeId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};

fn draw(window: &mut Window, cx: &mut gpui_kit::App) -> serde_json::Value {
    window.activate_a11y();
    window.render_frame(cx);
    // the frame's callbacks (the desk's size) run before the next frame
    window.simulate_next_frame(cx);
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

/// Activates the test window and waits for it to be so: gpui reports focus
/// moves (`on_focus_out`, `on_blur`) only in the active window, and
/// `activate_window` takes effect once the executor runs.
fn activate(native: &mut VisualTestContext) {
    native.update(|window, _| window.activate_window());
    native.run_until_parked();
    native.update(|window, _| assert!(window.is_window_active(), "the window did not activate"));
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

/// A screen state, and what is open over its desk.
pub(super) struct Scene(Ducktape, Option<Overlay>);

impl From<Ducktape> for Scene {
    fn from(state: Ducktape) -> Self {
        Self(state, None)
    }
}

impl From<(Ducktape, Overlay)> for Scene {
    fn from((state, overlay): (Ducktape, Overlay)) -> Self {
        Self(state, Some(overlay))
    }
}

pub(super) fn open(
    scene: impl Into<Scene>,
    cx: &mut TestAppContext,
) -> (Entity<WindowRoot>, VisualTestContext) {
    let Scene(state, overlay) = scene.into();
    let (_, _, view, mut native) = super::layers::tests::open_console(state, cx);
    if let Some(overlay) = overlay {
        super::layers::tests::show(&view, Some(overlay), &mut native);
    }
    (view, native)
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
    model.update(cx, |model, cx| {
        model.state.signer_key = "ab".into();
        model.state.stage = Stage::Recover(Default::default());
        model.bridge(false, cx);
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
    model.update(cx, |model, cx| {
        model.state.stage = Stage::Phrase(crate::ui::Phrase {
            words: crate::Secret::from(String::from("canoe pond forest")),
            ..Default::default()
        });
        model.bridge(false, cx);
    });
    let nodes = native.update(draw);
    let sheet = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "shell:phrase")
        .unwrap_or_else(|| panic!("no phrase sheet: {nodes}"));
    assert!(
        sheet["name"].as_str().is_some_and(|name| !name.is_empty()),
        "the sheet shows no words: {sheet}"
    );
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
    model.update(cx, |model, cx| {
        model.state.signer_key = "ab".into();
        model.state.stage = Stage::Account(Default::default());
        model.bridge(false, cx);
    });
    native.update(|window, cx| type_into("create-account-name/field", "duck", window, cx));
    let nodes = native.update(draw);
    assert_eq!(find(&nodes, "TextInput", "Account name")["value"], "duck");
    // the field owns the name, and the reducer heard it as it was typed
    let heard = model.read_with(cx, |model, _| match &model.state.stage {
        Stage::Account(step) => step.name.clone(),
        _ => String::new(),
    });
    assert_eq!(heard, "duck", "the typed name never reached the reducer");
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
    model.update(cx, |model, cx| {
        model.state.stage = Stage::Unlock(Default::default());
        model.bridge(false, cx);
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

/// The field owns what is typed, and the reducer's copy of a secret can go
/// without it: after "Read without a key" and back the model's password is
/// empty, and the password field used to go on showing the old dots —
/// while a retry sent nothing. A screen change empties the fields of the
/// step it left, in the launcher's observer.
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
            view.read(cx)
                .launcher()
                .read(cx)
                .fields
                .password
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

    // reading without a key wipes it; the key screen comes back (from the
    // desk's account menu)
    for message in [Message::BrowseWithoutKey, Message::SignIn] {
        model.update(cx, |model, cx| model.dispatch(message, cx));
        native.update(draw);
    }
    assert_eq!(
        field(&mut native),
        "",
        "the field kept a password the model wiped"
    );
}

/// The field is the source of what is typed: a change reaches `Session` as
/// it happens, with no draw between, and a draw writes nothing into the
/// field either way — keys can land in a field before their change goes
/// out (the AX door's `type` sends a word's keys in one update), and a
/// draw that put the model's older text back would eat them.
#[gpui_kit::test]
fn typed_text_reaches_the_entity_on_change_not_on_draw(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (state, _) = Ducktape::boot();
    let (view, mut native) = open(state, cx);
    native.update(draw);
    let (model, field) = native.update(|_, cx| {
        let view = view.read(cx);
        let field = view.launcher().read(cx).fields.endpoint.state.clone();
        (view.model.clone(), field)
    });
    let session = native.update(|_, cx| model.read(cx).entities.session.clone());
    let endpoint = |native: &mut VisualTestContext| {
        native.update(|_, cx| session.read(cx).get().endpoint.clone())
    };
    native.update(|window, cx| {
        field.update(cx, |field, cx| {
            field.replace_all("10.0.0.5:8844", window, cx)
        })
    });
    assert_eq!(
        endpoint(&mut native),
        "10.0.0.5:8844",
        "the session waited for a draw"
    );

    native.update(|window, cx| {
        // set_value emits no change: nobody has heard of this text
        field.update(cx, |field, cx| field.set_value("abcdef", window, cx));
    });
    native.update(draw);
    let shown = native.update(|_, cx| field.read(cx).value().to_string());
    assert_eq!(
        shown, "abcdef",
        "a draw wiped keys the session had not heard"
    );
    assert_eq!(
        endpoint(&mut native),
        "10.0.0.5:8844",
        "a draw sent the field's text"
    );
}

/// The account's name is typed once: the reducer carries it to the
/// recovery key's screen and back ("← Back"), and so does its field; it
/// goes when the account steps do, and a new account step starts empty.
#[gpui_kit::test]
fn the_account_name_goes_only_with_the_account_steps(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (mut state, _) = Ducktape::boot();
    state.signer_key = "ab".into();
    state.stage = Stage::Account(Default::default());
    let (view, mut native) = open(state, cx);
    native.update(|window, cx| type_into("create-account-name/field", "duck", window, cx));
    let model = native.update(|_, cx| view.read(cx).model.clone());
    let name = |native: &mut VisualTestContext| {
        native.update(|_, cx| {
            let field = &view.read(cx).launcher().read(cx).fields.name;
            field.state.read(cx).value().to_string()
        })
    };
    let heard = |cx: &mut TestAppContext| {
        model.read_with(cx, |model, _| match &model.state.stage {
            Stage::Account(step) => step.name.clone(),
            _ => String::new(),
        })
    };
    for message in [Message::RecoverShow, Message::RecoverCancel] {
        model.update(cx, |model, cx| model.dispatch(message, cx));
        native.update(draw);
    }
    assert_eq!(heard(cx), "duck", "the reducer lost the name");
    assert_eq!(name(&mut native), "duck", "the field lost the name");

    // "Not now", then "Create account" from the desk
    for message in [Message::CreateAccountLater, Message::ShowCreateAccount] {
        model.update(cx, |model, cx| model.dispatch(message, cx));
        native.update(draw);
    }
    assert_eq!(heard(cx), "");
    assert_eq!(
        name(&mut native),
        "",
        "a new account step kept the old name"
    );
}

/// The console closed to the tray mid-step and opened again: its new
/// fields show what the reducer holds of the step, which is what a submit
/// sends.
#[gpui_kit::test]
fn a_console_opened_mid_step_shows_what_the_reducer_holds(cx: &mut TestAppContext) {
    let unlock = Stage::Unlock(crate::ui::Unlock {
        password: "hunter2".to_string().into(),
        ..Default::default()
    });
    let recover = Stage::Recover(crate::ui::Recover {
        phrase: "abandon ability".to_string().into(),
        name: "duck".into(),
    });
    let account = Stage::Account(crate::ui::Account {
        name: "duck".into(),
        ..Default::default()
    });
    let phrase = Stage::Phrase(crate::ui::Phrase {
        quiz: Some([0, 5, 9]),
        answers: ["one", "two", "three"].map(|word| word.to_string().into()),
        ..Default::default()
    });
    for (stage, want) in [
        (unlock, ["hunter2", "", "", "", "", ""]),
        (recover, ["", "abandon ability", "duck", "", "", ""]),
        (account, ["", "", "duck", "", "", ""]),
        (phrase, ["", "", "", "one", "two", "three"]),
    ] {
        let (mut state, _) = Ducktape::boot();
        state.signer_key = "ab".into();
        let step = stage.step();
        state.stage = stage;
        let (view, mut native) = open(state, cx);
        let shown = native.update(|_, cx| {
            let fields = &view.read(cx).launcher().read(cx).fields;
            let [a, b, c] = &fields.words;
            [&fields.password, &fields.restore, &fields.name, a, b, c]
                .map(|field| field.state.read(cx).value().to_string())
        });
        assert_eq!(
            shown, want,
            "{step}: the fields lost what the reducer holds"
        );
    }
}

/// The reducer puts another address in `Session` (the node reached, a
/// switch that did not land): the address field shows it, and what is
/// typed after goes out as before.
#[gpui_kit::test]
fn the_address_field_shows_where_the_session_moved_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (state, _) = Ducktape::boot();
    let (view, mut native) = open(state, cx);
    native.update(|window, cx| type_into("endpoint/field", "10.0.0.5:8844", window, cx));
    let model = native.update(|_, cx| view.read(cx).model.clone());
    model.update(cx, |model, cx| {
        model.state.endpoint = "http://127.0.0.1:1".into();
        model.bridge(false, cx);
    });
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Node address")["value"],
        "http://127.0.0.1:1"
    );
    native.update(|window, cx| type_into("endpoint/field", "10.0.0.6:8844", window, cx));
    let typed = model.read_with(cx, |model, _| model.state.endpoint.clone());
    assert_eq!(typed, "10.0.0.6:8844");
}

/// "Add a device…" opens with its code field empty, as the reducer's copy
/// of the code is emptied as it opens (`ApproveOpen`).
#[gpui_kit::test]
fn add_a_device_opens_with_its_code_field_empty(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        keys::bind(cx);
    });
    let (view, mut native) = open((gate::desk(), Overlay::Approve), cx);
    native.update(|window, cx| type_into("approve-code/field", "ABCD-EFGH", window, cx));
    let nodes = native.update(draw);
    assert_eq!(find(&nodes, "TextInput", "Code")["value"], "ABCD-EFGH");
    let model = native.update(|_, cx| view.read(cx).model.clone());
    let heard = model.read_with(cx, |model, _| model.state.sign_in.approve_code.clone());
    assert_eq!(
        heard, "ABCD-EFGH",
        "the typed code never reached the reducer"
    );
    super::layers::tests::show(&view, None, &mut native);
    native.update(draw);
    super::layers::tests::show(&view, Some(Overlay::Approve), &mut native);
    let nodes = native.update(draw);
    assert_eq!(
        find(&nodes, "TextInput", "Code")["value"],
        "",
        "the code field kept the last code"
    );
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
    let (_view, mut native) = open((state, Overlay::Network), cx);
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
/// in the header.
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
    let bell = Overlay::Menu(Popover::Notifications);
    let (view, mut native) = open((state, bell), cx);

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
    // the bell reads the centre through its slice: as a dispatch would
    view.update(&mut native, |view, cx| {
        view.model.update(cx, |model, cx| model.bridge(false, cx))
    });
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
    let overlays = native.update(|_, cx| view.read(cx).overlays());
    for open in [false, true] {
        native.update(|_, cx| overlays.update(cx, |it, cx| it.toggle(bell, cx)));
        assert_eq!(
            native.update(|_, cx| *overlays.read(cx).get()),
            open.then_some(bell)
        );
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
        (Overlay::Settings(SettingsPage::Appearance), "Settings"),
        (Overlay::Menu(Popover::Node), "Node status"),
    ] {
        let (view, mut native) = open((gate::desk(), overlay), cx);
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
        let open = native.update(|_, cx| *view.read(cx).overlays().read(cx).get());
        assert_eq!(open, Some(overlay), "a click inside {name} closed it");
    }
}
