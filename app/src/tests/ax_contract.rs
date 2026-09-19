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
    audit(name, &mut cx, window.into())
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

/// Every view staged in `DUCKTAPE_VIEWS_DIR` (the deployed set: epoch 8, its
/// three exemptions apply) and, when `DUCKTAPE_VIEWS_DIR_NEXT` names one, the
/// set that deploys next, with no exemption at all — mounted as the canary
/// mounts them (tests/canary.rs), in the state their session props give.
#[test]
fn ax_contract_views() {
    let views = std::env::var_os("DUCKTAPE_VIEWS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/views")
        });
    let mut failures = views_audit(&views, true);
    if let Some(next) = std::env::var_os("DUCKTAPE_VIEWS_DIR_NEXT") {
        failures.extend(views_audit(std::path::Path::new(&next), false));
    }
    assert_clean(failures);
}

fn views_audit(views: &std::path::Path, exempt_epoch_8: bool) -> Vec<String> {
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
    let client = runtime.block_on(fake_node(node));
    crate::module_view::connected(&client).joined();
    let mut failures = Vec::new();
    for module in modules {
        let mut app = Ducktape::initial_state();
        app.connected = true;
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
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.run_until_parked();
        let root = crate::module_view::canary::frame(module)
            .unwrap_or_else(|| panic!("view {module} did not render a frame"));
        // the view's own tree, held to the wire's rules
        let epoch_8 = exempt_epoch_8
            && crate::module_view::canary::epoch(module) == Some(crate::module_view::Epoch::Eight);
        let name = format!("{set}: view {module}");
        for fault in view_wire::accessibility_faults(&root) {
            if !(epoch_8 && epoch_8_cannot_carry(&root, &fault)) {
                failures.push(format!(
                    "{name}: {} — {:?}",
                    fault.path.join("/"),
                    fault.kind
                ));
            }
        }
        // the host half of the same exemption: an epoch-8 Editor has no label
        // to hand the field it draws
        let unlabelled = match epoch_8 {
            true => unlabelled_editors(&root),
            false => Vec::new(),
        };
        failures.extend(
            audit(&name, &mut cx, window.into())
                .into_iter()
                .filter(|failure| {
                    !(failure.ends_with("text input without a label")
                        && unlabelled
                            .iter()
                            .any(|key| failure.contains(&format!("{key}/field\")"))))
                }),
        );
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
    }
    failures
}

/// How many nodes of `node`'s tree the wire says are tabs.
fn tabs_in(node: &view_wire::Node) -> usize {
    let own = crate::view_tree::accessible(node).role == Some(gpui_kit::Role::Tab);
    usize::from(own) + node.children().iter().map(tabs_in).sum::<usize>()
}

/// The keys of the Editors in `node`'s tree that carry no label.
fn unlabelled_editors(node: &view_wire::Node) -> Vec<String> {
    let mut keys: Vec<String> = node
        .children()
        .iter()
        .flat_map(unlabelled_editors)
        .collect();
    if let view_wire::Node::Editor { key, label, .. } = node
        && label.as_deref().is_none_or(str::is_empty)
    {
        keys.push(key.clone());
    }
    keys
}

/// Whether a view built at wire epoch 8 could not have avoided `fault`: its
/// wire has no label on an Editor, Slider, ComboBox or PickList
/// (`UnlabeledInput`), no role on a MouseArea (`NoRole`) and no label on an
/// Overlay (`Unnamed`). Every other fault it answers for, and an epoch-10
/// view answers for all of them.
fn epoch_8_cannot_carry(root: &view_wire::Node, fault: &view_wire::Fault) -> bool {
    use view_wire::{FaultKind, Node};
    fn at<'a>(node: &'a Node, path: &[String]) -> Option<&'a Node> {
        let (first, rest) = path.split_first()?;
        if node.key().unwrap_or_default() != first {
            return None;
        }
        if rest.is_empty() {
            return Some(node);
        }
        node.children().iter().find_map(|child| at(child, rest))
    }
    matches!(
        (fault.kind, at(root, &fault.path)),
        (
            FaultKind::UnlabeledInput,
            Some(
                Node::Editor { .. }
                    | Node::Slider { .. }
                    | Node::ComboBox { .. }
                    | Node::PickList { .. }
            )
        ) | (FaultKind::NoRole, Some(Node::MouseArea { .. }))
            | (FaultKind::Unnamed, Some(Node::Overlay { .. }))
    )
}

#[test]
fn an_epoch_8_view_answers_only_for_the_faults_its_wire_can_carry() {
    use view_wire::{FaultKind, Node, accessibility_faults};
    let mut editor = Node::Editor {
        options: Box::default(),
        key: "composer".into(),
        placeholder: String::new(),
        label: None,
        document: view_wire::editor_document::EditorDocumentRef {
            document: "draft".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
        width: None,
        height: None,
        min_height: None,
        max_height: None,
    };
    let root = view_wire::kit::column("page", [editor.clone(), view_wire::kit::text("a", "")]);
    let faults = accessibility_faults(&root);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0].kind, FaultKind::UnlabeledInput);
    assert!(epoch_8_cannot_carry(&root, &faults[0]));
    // an unlabelled input could always be named: epoch 8 answers for it
    let Node::Editor { label, .. } = &mut editor else {
        unreachable!()
    };
    *label = Some("Message".into());
    let input = Node::Input {
        options: Default::default(),
        key: "search".into(),
        placeholder: String::new(),
        value: String::new(),
        on_input: 1,
        on_submit: None,
        width: None,
        secure: false,
        style: Box::default(),
    };
    let root = view_wire::kit::column("page", [editor, input]);
    let faults = accessibility_faults(&root);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert!(!epoch_8_cannot_carry(&root, &faults[0]));
}
