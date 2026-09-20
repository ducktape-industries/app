//! ax_contract (#114): every screen's accessibility tree, read headless off
//! gpui-pre's test window, held to what a screen reader needs. A screen that
//! breaks a rule fails here, naming the window, the node and the rule; there
//! is no allowlist.
use super::*;
use accesskit_consumer::Tree;
use gpui_kit::accesskit::{Action, NodeId, Role, TreeUpdate};
use gpui_kit::{AnyWindowHandle, AppContext, HeadlessAppContext, px, size};
use std::collections::{HashMap, HashSet};

/// Activates `window`'s tree, draws it, walks its tab order, and returns
/// every rule the tree breaks.
fn audit(name: &str, cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Vec<String> {
    let draw = |cx: &mut HeadlessAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let update = window.last_a11y_tree_update();
            let debug = window.debug_a11y_tree_json();
            (update.expect("an active window sends its tree"), debug)
        })
        .unwrap()
    };
    cx.update_window(window, |_, window, _| window.activate_a11y())
        .unwrap();
    let (update, debug) = draw(cx);
    cx.run_until_parked();
    // Tab order is GPUI's tab-stop cycle: follow it once round, noting the
    // node each stop puts focus on.
    let mut reached = HashSet::new();
    let mut stops = Vec::new();
    let mut nodeless = 0;
    loop {
        let focused = cx
            .update_window(window, |_, window, cx| {
                window.focus_next(cx);
                window.focused(cx)
            })
            .unwrap();
        let Some(focused) = focused else { break };
        if stops.contains(&focused) || stops.len() > 1000 {
            break;
        }
        stops.push(focused);
        let focus = draw(cx).0.focus;
        if focus == update.tree.as_ref().unwrap().root {
            nodeless += 1;
        }
        reached.insert(focus);
    }
    let mut failures = rules(name, update, debug, &reached);
    if nodeless > 0 {
        failures.push(format!(
            "{name}: {nodeless} tab stop(s) focus an element with no node — interactive node with no role"
        ));
    }
    failures
}

fn rules(
    window: &str,
    update: TreeUpdate,
    debug: Option<String>,
    reached: &HashSet<NodeId>,
) -> Vec<String> {
    // GPUI's debug dump names each node's element id and source line.
    let debug: serde_json::Value = debug
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let mut origin = HashMap::new();
    if let Some(nodes) = debug["nodes"].as_object() {
        for node in nodes.values() {
            let field = |key: &str| node[key].as_str().unwrap_or("?").to_owned();
            origin.insert(
                field("accesskit_id"),
                format!("`{}` at {}", field("element_id"), field("source_location")),
            );
        }
    }
    let mut failures = Vec::new();
    let mut fail = |id: NodeId, role: Role, rule: &str| {
        let at = origin.get(&id.0.to_string()).map_or("", String::as_str);
        failures.push(format!("{window}: node {} {role:?} {at} — {rule}", id.0));
    };
    let mut ids = HashSet::new();
    let mut authors = HashSet::new();
    for (id, node) in &update.nodes {
        let author = node.author_id().map(str::to_owned);
        if !ids.insert(*id) || author.is_some_and(|author| !authors.insert(author)) {
            fail(*id, node.role(), "duplicate id within the window");
        }
    }
    let tree = Tree::new(update, true);
    let mut stack = vec![tree.state().root()];
    while let Some(node) = stack.pop() {
        stack.extend(node.children());
        if node.is_root() {
            continue;
        }
        let (id, role, data) = (node.locate().0, node.role(), node.data());
        let interactive = [Action::Click, Action::Focus, Action::SetValue]
            .into_iter()
            .any(|action| data.supports_action(action));
        let named = node.label().is_some_and(|name| !name.trim().is_empty());
        if interactive && matches!(role, Role::Unknown | Role::GenericContainer) {
            fail(id, role, "interactive node with no role");
        }
        if node.is_text_input() && !named {
            fail(id, role, "text input without a label");
        } else if interactive && !named && role == Role::Button && node.children().next().is_none()
        {
            fail(id, role, "image-only button without a name");
        } else if interactive && !named {
            fail(id, role, "interactive node without a name");
        }
        let stated = match role {
            Role::CheckBox
            | Role::Switch
            | Role::RadioButton
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio => node.toggled().is_some(),
            Role::Tab => data.is_selected().is_some(),
            Role::DisclosureTriangle => data.is_expanded().is_some(),
            _ => true,
        };
        if !stated {
            fail(id, role, "stateful control does not report its state");
        }
        // A control that a pointer cannot press is disabled, and says so;
        // otherwise a screen reader announces it as available (#115).
        let pressable = data.supports_action(Action::Click);
        let control = matches!(
            role,
            Role::Button
                | Role::Link
                | Role::Tab
                | Role::CheckBox
                | Role::Switch
                | Role::RadioButton
                | Role::MenuItem
                | Role::ListBoxOption
        );
        if control && !pressable && !node.is_disabled() {
            fail(
                id,
                role,
                "control that cannot be pressed does not report disabled",
            );
        }
        // What a pointer can press, a keyboard can reach.
        if pressable && !node.is_disabled() && !data.supports_action(Action::Focus) {
            fail(id, role, "pressable node a keyboard cannot focus");
        }
        if node.is_dialog() && !named {
            fail(id, role, "dialog without a name");
        }
        if data.supports_action(Action::Focus) && !node.is_disabled() && !reached.contains(&id) {
            fail(id, role, "focusable node not reachable in tab order");
        }
    }
    failures
}

fn assert_clean(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} accessibility rule(s) broken:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn native(name: &str, app: Ducktape, kind: crate::shell::WindowKind) -> Vec<String> {
    let (mut cx, window) = open_native(app, kind);
    audit(name, &mut cx, window)
}

fn open_native(
    app: Ducktape,
    kind: crate::shell::WindowKind,
) -> (HeadlessAppContext, AnyWindowHandle) {
    let (width, height) = match kind {
        crate::shell::WindowKind::Console => (1280., 800.),
        crate::shell::WindowKind::Onboarding => (480., 640.),
        crate::shell::WindowKind::Huddle => (320., 460.),
    };
    let mut cx = crate::frame_probe::headless_context();
    let window = cx
        .open_window(size(px(width), px(height)), |window, cx| {
            let view = crate::shell::test_window(app, kind, window, cx);
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .expect("native screen opens");
    (cx, window.into())
}

/// Every step of the launch window.
const STEPS: [HubStep; 12] = [
    HubStep::Loading,
    HubStep::Password,
    HubStep::Phrase,
    HubStep::Confirm,
    HubStep::Wallets,
    HubStep::Restore,
    HubStep::Networks,
    HubStep::Join,
    HubStep::Provisioning,
    HubStep::Live,
    HubStep::Account,
    HubStep::Offline,
];

#[test]
fn ax_contract_native() {
    use crate::shell::WindowKind::{Console, Huddle, Onboarding};
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let updates = tempfile::tempdir().unwrap();
    let current = app_update::Sha::digest(b"current");
    let staged = app_update::Phase::Staged(app_update::Staged {
        current,
        previous: None,
        pinned_sequence: 2,
        staged: app_update::Sha::digest(b"next"),
        sequence: 3,
        display: "2026.09.3+abcdef0".into(),
        node_contract: backend::EXPECTED_NODE_CONTRACT,
        refused: None,
    });
    let rolled_back = app_update::Phase::RolledBack(app_update::RolledBack {
        current,
        failed: app_update::Sha::digest(b"next"),
        reason: app_update::RollbackReason::NeverRendered,
        pinned_sequence: 2,
    });
    let idle = app_update::Phase::Idle(app_update::Idle {
        current,
        previous: None,
        pinned_sequence: 2,
    });
    let mut failures = Vec::new();
    for step in STEPS {
        let mut app = Ducktape::initial_state();
        app.hub_step = step;
        failures.extend(native(&format!("onboarding {step:?}"), app, Onboarding));
    }
    // the launch window's refused row with its update strip and a toast
    for (phase, strip) in [(staged.clone(), "ready"), (idle, "check")] {
        let mut app = Ducktape::initial_state();
        app.hub_step = HubStep::Networks;
        app.hub_networks = vec![super::shell::refused_workspace_row(
            "walk",
            "http://127.0.0.1:1",
        )];
        app.hub_selected = "walk".into();
        app.updater = Some(super::shell::armed_updater(phase, updates.path()));
        app.toast = "Invite copied".into();
        failures.extend(native(
            &format!("onboarding refused row, {strip} strip"),
            app,
            Onboarding,
        ));
    }
    // a silent row's line and offer, and the wait's ports hint (#137)
    failures.extend(native("onboarding silent row", silent_row(), Onboarding));
    failures.extend(native(
        "onboarding wait, ports held",
        ports_held_wait(),
        Onboarding,
    ));
    let connected = || {
        let mut app = Ducktape::initial_state();
        app.connected = true;
        app.connected_rpc = "http://127.0.0.1:8844".into();
        app.network_name = "walk".into();
        app
    };
    failures.extend(native("console", connected(), Console));
    // a tab whose view is still on its way, and one whose load failed
    for (failed, name) in [
        (false, "console, view loading"),
        (true, "console, view failed"),
    ] {
        crate::module_view::loading_tests::seat_standin("chat", failed);
        let mut app = connected();
        app.shell_tab = ShellTab::View("chat");
        failures.extend(native(name, app, Console));
    }
    crate::module_view::loading_tests::unseat("chat");
    for (phase, strip) in [(staged, "ready"), (rolled_back, "rolled-back")] {
        let mut app = connected();
        app.updater = Some(super::shell::armed_updater(phase, updates.path()));
        app.error = "The node refused the write".into();
        app.toast = "Saved".into();
        failures.extend(native(
            &format!("console, error plate, toast, {strip} strip"),
            app,
            Console,
        ));
    }
    let mut app = connected();
    app.bell_open = true;
    failures.extend(native("console, bell open", app, Console));
    let (app, _) = crate::frame_probe::console_in_huddle();
    failures.extend(native("huddle", app, Huddle));
    assert_clean(failures);
}

/// Every view staged in `DUCKTAPE_VIEWS_DIR` (the deployed set, wire epoch
/// 10), with no exemption — mounted as the canary mounts them
/// (tests/canary.rs), in the state their session props give.
#[test]
fn ax_contract_views() {
    let views = std::env::var_os("DUCKTAPE_VIEWS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/views")
        });
    assert_clean(views_audit(&views));
}

fn views_audit(views: &std::path::Path) -> Vec<String> {
    use crate::backend::view_source::tests::{FakeDeployment, fake_node};
    let set = views.file_name().unwrap_or_default().to_string_lossy();
    let mut modules = std::fs::read_dir(views)
        .unwrap_or_else(|error| panic!("{}: {error}; build ducktape-views and point DUCKTAPE_VIEWS_DIR at its target/views", views.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let module = name.strip_suffix("_view.wasm")?.to_owned();
            Some(&*Box::leak(module.into_boxed_str()))
        })
        .collect::<Vec<&'static str>>();
    modules.sort();
    assert!(!modules.is_empty(), "{} stages no views", views.display());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _turn = runtime.block_on(crate::module_view::canary::connection_turn());
    // every view off a node whose registry lists it, as a module's or on
    // its own — the staged set is the fixture registry's artifacts
    use crate::backend::view_source::MODULE_OWNED;
    let served = modules
        .iter()
        .enumerate()
        .map(|(index, module)| {
            let view = std::fs::read(views.join(format!("{module}_view.wasm"))).unwrap();
            let artifact = module_artifact::Artifact::Module(module_artifact::ModuleArtifact {
                component: vec![index as u8],
                index: None,
                view: Some(module_artifact::ViewArtifact {
                    component: view,
                    assets: Default::default(),
                }),
                lanes: Vec::new(),
            });
            (*module, artifact)
        })
        .collect::<Vec<_>>();
    let node = FakeDeployment::serving(served[0].0, &served[0].1);
    node.artifacts
        .lock()
        .unwrap()
        .extend(served[1..].iter().map(|(_, artifact)| artifact.clone()));
    *node.status.lock().unwrap() = serde_json::json!({"module_status":{"modules":
        served.iter().map(|(module, artifact)| serde_json::json!({
            "module_id": module,
            "kind": if MODULE_OWNED.contains(module) { "module" } else { "view" },
            "active_code_hash": artifact.hash(),
            "pending": null, "history": [{"height": 7, "code_hash": artifact.hash()}]
        })).collect::<Vec<_>>()
    }});
    // one open, empty room: on a network with none, chat draws "No channels
    // yet" and no composer to audit
    let room = serde_json::json!({ "id": "general", "name": "general", "archived": false,
        "post_policy": "open", "voice": false, "head_seq": 0, "huddle": [] });
    node.answer_view(
        "chat",
        serde_json::json!({
            "channels": { "channels": { "channels": [room], "has_more": false } },
            "channel": { "channel": room },
            "roots": { "roots": { "roots": [], "has_more": false } },
            "members": { "members": { "members": [], "has_more": false } },
        }),
    );
    let client = runtime.block_on(fake_node(node));
    crate::module_view::connected(&client).joined();
    let mut failures = Vec::new();
    for module in modules {
        let mut app = Ducktape::initial_state();
        app.connected = true;
        // a reader on an account, in the room a connect lands on (the only
        // one): a key that holds no account gets chat's account notice where
        // the composer would be
        app.account_number = "1".to_owned();
        app.active_channel = "general".to_owned();
        app.shell_tab = ShellTab::View(module);
        let (spec, _) = app.native_view();
        let mut cx = crate::frame_probe::headless_context();
        let window = cx
            .open_window(size(px(1100.), px(800.)), |window, cx| {
                let view = cx.new(|cx| {
                    let mut view = crate::module_view::NativeModuleView::new(module);
                    view.set_props(spec.props, cx);
                    view
                });
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            })
            .unwrap();
        // a view's own reads (chat's rooms) land off the window thread, each
        // answer the next draw's: draw until none is owed
        for _ in 0..64 {
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
            cx.run_until_parked();
            if !crate::module_view::canary::wait_for_reads(module) {
                break;
            }
        }
        let root = crate::module_view::canary::frame(module)
            .unwrap_or_else(|| panic!("view {module} did not render a frame"));
        // the view's own tree, held to the wire's rules
        let name = format!("{set}: view {module}");
        for fault in view_wire::accessibility_faults(&root) {
            failures.push(format!(
                "{name}: {} — {:?}",
                fault.path.join("/"),
                fault.kind
            ));
        }
        failures.extend(audit(&name, &mut cx, window.into()));
        // a wire Tab is a native Tab: the kit button draws a role of its own
        let wire_tabs = tabs_in(&root);
        let native_tabs = cx
            .update_window(window.into(), |_, window, _| {
                let update = window.last_a11y_tree_update().expect("a tree");
                update
                    .nodes
                    .iter()
                    .filter(|(_, node)| {
                        node.role() == gpui_kit::Role::Tab && node.is_selected().is_some()
                    })
                    .count()
            })
            .unwrap();
        if native_tabs < wire_tabs {
            failures.push(format!(
                "{name}: {wire_tabs} wire tab(s), {native_tabs} native Tab node(s) reporting selected"
            ));
        }
        failures.extend(found_by_the_walk(
            &name,
            module,
            &tree(&mut cx, window.into()),
        ));
        if !tab_key_leaves_a_button(&mut cx, window.into()) {
            failures.push(format!(
                "{name}: Tab pressed on a button inside the view does not move focus"
            ));
        }
        let opens = match module {
            "chat" => Some("New channel"),
            "pages" => Some("New page"),
            _ => None,
        };
        if let Some(button) = opens {
            failures.extend(press_changes_view(
                &name,
                button,
                module,
                &mut cx,
                window.into(),
            ));
        }
    }
    failures
}

/// A deployed view's primary create action reaches its guest and exposes its
/// immediate result. This crosses the exact door -> AccessKit -> ViewTree ->
/// WASM boundary used by a QA walk.
fn press_changes_view(
    name: &str,
    button: &str,
    module: &str,
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
) -> Vec<String> {
    cx.update_window(window, |_, window, cx| {
        window.activate_a11y();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let before = cx
        .update_window(window, |_, window, _| {
            crate::ax_door::snapshot("console", window, false)
        })
        .unwrap();
    let Some(target) = before.iter().find(|node| node.name == button) else {
        return vec![format!("{name}: {button} is not in the door tree")];
    };
    let pressed = cx
        .update_window(window, |_, window, cx| {
            crate::ax_door::perform_by_id("console", window, cx, &target.id, "press", "")
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.run_until_parked();
    let after = self::tree(cx, window);
    let changed = match module {
        "chat" => nodes(&after).iter().any(|node| node.role() == Role::Dialog),
        "pages" => reading(&after, button)
            .first()
            .is_some_and(|node| node.is_disabled()),
        _ => unreachable!("only state-changing view actions call this helper"),
    };
    if pressed && changed {
        Vec::new()
    } else {
        vec![format!(
            "{name}: pressing {button} did not change the {module} view"
        )]
    }
}

/// What the QA walk of #116 could not find in a view, found: the chat
/// composer as a named text area, and Home's Copy button and This node card.
fn found_by_the_walk(name: &str, module: &str, tree: &Tree) -> Vec<String> {
    let all = nodes(tree);
    let missing = |what: &str| format!("{name}: {what} is not in the tree");
    let mut failures = Vec::new();
    match module {
        "chat" => {
            let composer = all
                .iter()
                .any(|node| node.role() == Role::MultilineTextInput && !said(node).is_empty());
            if !composer {
                failures.push(missing("a named message composer"));
            }
        }
        "home" => {
            if !all
                .iter()
                .any(|node| node.role() == Role::Button && said(node) == "Copy")
            {
                failures.push(missing("the Copy button"));
            }
            if !all.iter().any(|node| said(node) == "This node") {
                failures.push(missing("This node"));
            }
        }
        _ => {}
    }
    failures
}

/// Whether Tab, pressed as a key while a button inside the view has focus,
/// moves focus on: the view hears the key, and does not keep it. True of a
/// view with no button.
fn tab_key_leaves_a_button(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> bool {
    for _ in 0..200 {
        let focused = cx
            .update_window(window, |_, window, cx| {
                window.focus_next(cx);
                window.focused(cx)
            })
            .unwrap();
        let on_button = self::tree(cx, window)
            .state()
            .focus()
            .is_some_and(|node| node.role() == Role::Button);
        if !on_button {
            continue;
        }
        cx.update_window(window, |_, window, cx| {
            window.dispatch_keystroke(gpui_kit::Keystroke::parse("tab").unwrap(), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let after = cx
            .update_window(window, |_, window, cx| window.focused(cx))
            .unwrap();
        return after.is_some() && after != focused;
    }
    true
}

/// How many nodes of `node`'s tree the wire says are tabs.
fn tabs_in(node: &view_wire::Node) -> usize {
    let own = crate::view_tree::accessible(node).role == Some(gpui_kit::Role::Tab);
    usize::from(own) + node.children().iter().map(tabs_in).sum::<usize>()
}

/// `window`'s tree as a screen reader has it now.
fn tree(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Tree {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.activate_a11y();
        window.draw(cx).clear(cx);
        let update = window.last_a11y_tree_update();
        Tree::new(update.expect("an active window sends its tree"), true)
    })
    .unwrap()
}

/// Every node of `tree` but its root.
fn nodes(tree: &Tree) -> Vec<accesskit_consumer::Node<'_>> {
    let mut all = Vec::new();
    let mut stack = vec![tree.state().root()];
    while let Some(node) = stack.pop() {
        stack.extend(node.children());
        if !node.is_root() {
            all.push(node);
        }
    }
    all
}

/// What a screen reader reads a node as: its name, or a text's words.
fn said(node: &accesskit_consumer::Node<'_>) -> String {
    node.label().or_else(|| node.value()).unwrap_or_default()
}

/// The nodes of `tree` a screen reader reads as `words`.
fn reading<'a>(tree: &'a Tree, words: &str) -> Vec<accesskit_consumer::Node<'a>> {
    nodes(tree)
        .into_iter()
        .filter(|node| said(node) == words)
        .collect()
}

/// Whether `node` or a node above it plays `role`.
fn within(node: &accesskit_consumer::Node<'_>, role: Role) -> bool {
    let mut at = Some(*node);
    while let Some(node) = at {
        if node.role() == role {
            return true;
        }
        at = node.parent();
    }
    false
}

fn connected_app() -> Ducktape {
    let mut app = Ducktape::initial_state();
    app.connected = true;
    app.connected_rpc = "http://127.0.0.1:8844".into();
    app.network_name = "walk".into();
    app
}

/// #115: a control that cannot be pressed is reported disabled — a native
/// one (Join with no invite) and a view's (a Button with no handler).
#[test]
fn ax_contract_a_control_that_cannot_be_pressed_reports_disabled() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Join;
    let (mut cx, window) = open_native(app, crate::shell::WindowKind::Onboarding);
    let tree = tree(&mut cx, window);
    let join = reading(&tree, "Join network");
    assert_eq!(join.len(), 1, "one Join network button");
    assert_eq!(join[0].role(), Role::Button);
    assert!(
        join[0].is_disabled(),
        "Join network with no invite is disabled"
    );
    let root = view_wire::kit::column(
        "page",
        [
            view_wire::kit::button("send", "Send", None, view_wire::ButtonPreset::Primary),
            view_wire::kit::button("save", "Save", Some(1), view_wire::ButtonPreset::Primary),
        ],
    );
    let mut cx = crate::frame_probe::headless_context();
    let window = cx
        .open_window(size(px(400.), px(300.)), |window, cx| {
            let view = cx.new(|_| crate::view_tree::ViewTree::new(root));
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .unwrap();
    let tree = self::tree(&mut cx, window.into());
    assert!(reading(&tree, "Send")[0].is_disabled());
    assert!(!reading(&tree, "Save")[0].is_disabled());
}

/// #116: the rail's sections are tabs a screen reader can find, name and
/// press, the open one selected, under their headings, in a navigation
/// landmark.
#[test]
fn ax_contract_the_rail_sections_are_selectable_tabs() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open_native(connected_app(), crate::shell::WindowKind::Console);
    let tree = tree(&mut cx, window);
    let tabs: Vec<_> = nodes(&tree)
        .into_iter()
        .filter(|node| node.role() == Role::Tab)
        .collect();
    assert!(
        tabs.len() >= 2,
        "the rail's sections are tabs: {}",
        tabs.len()
    );
    for tab in &tabs {
        assert!(!said(tab).is_empty(), "a section is named");
        assert!(
            tab.data().supports_action(Action::Click),
            "{} presses",
            said(tab)
        );
        assert!(
            tab.data().supports_action(Action::Focus),
            "{} focuses",
            said(tab)
        );
        assert!(
            within(tab, Role::Navigation),
            "{} is in the rail",
            said(tab)
        );
    }
    let selected = tabs
        .iter()
        .filter(|tab| tab.data().is_selected() == Some(true))
        .count();
    assert_eq!(selected, 1, "the open section is the selected tab");
    for heading in ["Workspace", "Network"] {
        assert!(
            reading(&tree, heading)
                .iter()
                .any(|node| node.role() == Role::Heading),
            "the rail heading {heading} is read"
        );
    }
    for button in ["Search", "Notifications"] {
        assert_eq!(reading(&tree, button)[0].role(), Role::Button, "{button}");
    }
}

/// The console's announcements are live regions holding their words: the
/// error an Alert, the toast and the update strip a Status.
#[test]
fn ax_contract_announcements_are_live_regions() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let updates = tempfile::tempdir().unwrap();
    let staged = app_update::Phase::Staged(app_update::Staged {
        current: app_update::Sha::digest(b"current"),
        previous: None,
        pinned_sequence: 2,
        staged: app_update::Sha::digest(b"next"),
        sequence: 3,
        display: "2026.09.3+abcdef0".into(),
        node_contract: backend::EXPECTED_NODE_CONTRACT,
        refused: None,
    });
    let mut app = connected_app();
    app.updater = Some(super::shell::armed_updater(staged, updates.path()));
    app.error = "The node refused the write".into();
    app.toast = "Saved".into();
    app.account_exists = false;
    let (mut cx, window) = open_native(app, crate::shell::WindowKind::Console);
    let tree = tree(&mut cx, window);
    for (words, role) in [
        ("The node refused the write", Role::Alert),
        ("Saved", Role::Status),
        ("Ducktape 2026.09.3+abcdef0 is ready", Role::Status),
        ("Sign in to use your account on this network.", Role::Status),
    ] {
        let found = reading(&tree, words);
        assert_eq!(found.len(), 1, "{words} is read");
        assert!(within(&found[0], role), "{words} is in a {role:?}");
    }
}

const SILENT_LINE: &str =
    "dognet has not answered. If it was re-founded, ask a member for a new invite.";
const PORTS_HELD: &str = "dognet#b5b6ea90, another network saved on this machine, holds port 8844.";

/// The launch window over a saved network whose node has not answered.
fn silent_row() -> Ducktape {
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Networks;
    app.hub_networks = vec![super::shell::silent_workspace_row("dognet#b5b6ea90")];
    app
}

/// The waiting step while another saved workspace's node holds its ports.
fn ports_held_wait() -> Ducktape {
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Provisioning;
    app.provision_steps = vec![backend::ProvisionStep {
        index: 4,
        label: "Waiting for your node".into(),
        state: "waiting".into(),
        settled: false,
        hint: String::new(),
        command: "ducktape-node-launcher run".into(),
        ports_held: PORTS_HELD.into(),
    }];
    app
}

/// A silent row's line is read as text beside its offer, and the waiting
/// step's ports hint is read inside a Status region — the step still waits;
/// the hint is not an error (#137).
#[test]
fn ax_contract_the_silent_row_and_the_ports_hint_are_read() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let (mut cx, window) = open_native(silent_row(), crate::shell::WindowKind::Onboarding);
    let row = tree(&mut cx, window);
    assert_eq!(reading(&row, SILENT_LINE).len(), 1, "{SILENT_LINE} is read");
    assert_eq!(reading(&row, "Join with a new invite").len(), 1);
    let (mut cx, window) = open_native(ports_held_wait(), crate::shell::WindowKind::Onboarding);
    let wait = tree(&mut cx, window);
    let found = reading(&wait, PORTS_HELD);
    assert_eq!(found.len(), 1, "the ports hint is read");
    assert!(
        within(&found[0], Role::Status),
        "the ports hint is a status"
    );
    assert!(
        !within(&found[0], Role::Alert),
        "the ports hint is not an error"
    );
}

/// The bell's popover is a dialog called Notifications: opening it puts
/// focus inside it, and Escape closes it.
#[test]
fn ax_contract_the_bell_is_a_dialog_that_takes_focus() {
    let _turn = crate::module_view::tests::blocking_connection_turn();
    let mut app = connected_app();
    app.bell_open = true;
    let (mut cx, window) = open_native(app, crate::shell::WindowKind::Console);
    let tree = tree(&mut cx, window);
    let dialog = nodes(&tree)
        .into_iter()
        .find(|node| node.role() == Role::Dialog)
        .expect("the bell is a dialog");
    assert_eq!(said(&dialog), "Notifications");
    let focus = tree.state().focus().expect("focus is on a node");
    assert!(
        focus.is_descendant_of(&dialog),
        "focus is in the dialog, on {:?} {}",
        focus.role(),
        said(&focus)
    );
    cx.update_window(window, |_, window, cx| {
        window.dispatch_keystroke(gpui_kit::Keystroke::parse("escape").unwrap(), cx);
    })
    .unwrap();
    let tree = self::tree(&mut cx, window);
    assert!(
        !nodes(&tree).iter().any(|node| node.role() == Role::Dialog),
        "Escape closed the bell"
    );
}

/// The pages editor: every block kind with its gutter drawn, held to the
/// same rules. #117: the gutter's two icon buttons beside each block are
/// named. The selection toolbar over selected words is held to its names,
/// roles, states and presses; not to Tab reach: it lives only while the
/// caret's block holds focus, and Tab inside a block indents (#114).
#[test]
fn ax_contract_editor() {
    use gpui_notion::editor::{BlockAttrs, BlockContent, NotionEditor, types};
    let content = vec![
        BlockContent::new(types::HEADING, "Title").with_attrs(BlockAttrs::level(1)),
        BlockContent::paragraph("A paragraph with words to select."),
        BlockContent::new(types::TASK_LIST, "A task"),
        BlockContent::new(types::TOGGLE, "A toggle"),
        BlockContent::new(types::CODE_BLOCK, "fn main() {}")
            .with_attrs(BlockAttrs::language("rust")),
        gpui_notion::editor::table_content(&[&["Name", "Role"], &["Ada", "Author"]]),
        BlockContent::new(types::IMAGE, ""),
        BlockContent::paragraph(""),
    ];
    let mut failures = Vec::new();
    for selecting in [false, true] {
        let mut cx = crate::frame_probe::headless_context();
        let content = content.clone();
        let mut opened = None;
        let window = cx
            .open_window(size(px(900.), px(900.)), |window, cx| {
                let editor = cx.new(|cx| {
                    let mut editor = NotionEditor::with_content(content, window, cx);
                    editor.show_gutter_always();
                    editor
                });
                opened = Some(editor.clone());
                cx.new(|cx| gpui_kit::component::Root::new(editor, window, cx))
            })
            .unwrap();
        let editor = opened.expect("the editor opened");
        if selecting {
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                editor.update(cx, |editor, cx| {
                    editor.select_text_in_block(1, 2..11, window, cx)
                });
            })
            .unwrap();
        }
        let tree = tree(&mut cx, window.into());
        for name in ["Insert block", "Block options"] {
            let buttons = reading(&tree, name);
            assert!(
                buttons.len() >= 2 && buttons.iter().all(|node| node.role() == Role::Button),
                "every block's gutter has a {name} button: {}",
                buttons.len()
            );
        }
        if !selecting {
            failures.extend(audit("editor", &mut cx, window.into()));
            continue;
        }
        for name in [
            "Bold",
            "Italic",
            "Underline",
            "Strikethrough",
            "Code",
            "Comment",
            "Link",
            "Color",
            "More formatting",
        ] {
            let found = reading(&tree, name);
            assert_eq!(found.len(), 1, "the toolbar's {name}");
            let button = found[0];
            assert_eq!(button.role(), Role::Button, "{name}");
            assert!(
                button.data().supports_action(Action::Click),
                "{name} presses"
            );
        }
        for mark in ["Bold", "Italic", "Underline", "Strikethrough", "Code"] {
            assert!(
                reading(&tree, mark)[0].toggled().is_some(),
                "{mark} is a toggle"
            );
        }
    }
    assert_clean(failures);
}
