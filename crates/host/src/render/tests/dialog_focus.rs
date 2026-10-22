//! A view dialog gives the keyboard back to what opened it (AX-120).
use super::*;
use ducktape_a11y_platform::headless::Harness;

fn handled(key: &str, name: &str, handle: u64) -> wire::Node {
    let mut node = button(key, name);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.focus_handle = Some(handle);
    }
    node
}

fn screen(open: bool) -> wire::Node {
    let mut children = vec![container("base", [handled("open", "Rename", 1)])];
    if open {
        children.push(container(
            "sheet",
            [handled("save", "Save", 2), handled("cancel", "Cancel", 3)],
        ));
    }
    let overlay = wire::Node::Overlay {
        id: named_id("rename"),
        label: Some("Rename channel".into()),
        on_dismiss: None,
        children,
        style: crate::render::test_style(div().size_full().style().clone()),
    };
    sized("root", overlay, Some(fill()), Some(fill()))
}

fn close_gives_focus_back_to_the_opener(cx: &mut gpui_kit::TestAppContext, fallback: bool) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(screen(false)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let root = native.update(|_, cx| cx.focus_handle());
    let _fallback = fallback.then(|| {
        native.update(|window, cx| {
            let root = root.clone();
            tree.update(cx, |_, cx| {
                cx.on_focus_lost(window, move |_, window, cx| window.focus(&root, cx))
            })
        })
    });
    let opener = tree
        .read_with(&native, |tree, _| tree.view_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    let show = |native: &mut gpui_kit::VisualTestContext, open: bool| {
        tree.update(native, |tree, cx| tree.replace(screen(open), &[], cx));
        native.update(crate::render::draw_frame);
        native.run_until_parked();
        native.update(|window, cx| window.focused(cx))
    };
    native.update(|window, cx| opener.focus(window, cx));
    let inside = show(&mut native, true);
    assert!(
        inside.is_some() && inside.as_ref() != Some(&opener),
        "the dialog takes the keyboard"
    );
    drop(inside);
    let after = show(&mut native, false);
    assert!(
        after.as_ref() != Some(&root),
        "the window's root took the keyboard"
    );
    assert_eq!(
        after.as_ref(),
        Some(&opener),
        "the opener has the keyboard back"
    );
}

#[gpui_kit::test]
fn a_closed_dialog_gives_focus_back_to_its_opener(cx: &mut gpui_kit::TestAppContext) {
    close_gives_focus_back_to_the_opener(cx, false);
}

#[gpui_kit::test]
fn a_closed_dialog_gives_focus_back_under_a_focus_lost_fallback(cx: &mut gpui_kit::TestAppContext) {
    close_gives_focus_back_to_the_opener(cx, true);
}

#[gpui_kit::test]
fn a_dialog_takes_no_keys_without_the_grant(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(screen(false))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let opener = tree
        .read_with(&native, |tree, _| tree.view_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    native.update(|window, cx| opener.focus(window, cx));
    tree.update(&mut native, |tree, cx| tree.replace(screen(true), &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    assert_eq!(
        native.update(|window, cx| window.focused(cx)).as_ref(),
        Some(&opener),
        "the dialog took the keyboard with no grant"
    );
}

fn holder(
    tree: &gpui_kit::Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
) -> Option<u64> {
    let focused = native.update(|window, cx| window.focused(cx))?;
    tree.read_with(native, |tree, _| {
        tree.view_focus_targets
            .iter()
            .find_map(|(handle, focus)| (*focus == focused).then_some(*handle))
    })
}

#[gpui_kit::test]
fn tab_goes_round_a_view_dialog_and_never_out(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::AppContext as _;
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(screen(false)).with_keys_grant(true));
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = window
        .read_with(cx, |root, _| root.view().clone())
        .unwrap()
        .downcast::<ViewTree>()
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let opener = tree
        .read_with(&native, |tree, _| tree.view_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    native.update(|window, cx| opener.focus(window, cx));
    tree.update(&mut native, |tree, cx| tree.replace(screen(true), &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    assert_eq!(
        holder(&tree, &mut native),
        Some(2),
        "the dialog opens on Save"
    );
    let walk: Vec<_> = ["tab", "tab", "tab", "shift-tab", "shift-tab"]
        .into_iter()
        .map(|key| {
            native.simulate_keystrokes(key);
            holder(&tree, &mut native)
        })
        .collect();
    assert_eq!(
        walk,
        [Some(3), Some(2), Some(3), Some(2), Some(3)],
        "Save, Cancel and round again: the keys stay in the dialog"
    );
}

#[gpui_kit::test]
fn under_touch_a_view_dialog_opens_on_no_control(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::AppContext as _;
    crate::contract::testing::touch_first_for_test(true);
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(screen(false)).with_keys_grant(true));
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = window
        .read_with(cx, |root, _| root.view().clone())
        .unwrap()
        .downcast::<ViewTree>()
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    tree.update(&mut native, |tree, cx| tree.replace(screen(true), &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    assert!(inside(&tree, &mut native), "the dialog has the keys");
    assert_eq!(holder(&tree, &mut native), None, "no control has them");
    native.simulate_keystrokes("tab");
    assert_eq!(holder(&tree, &mut native), Some(2), "Tab goes in, to Save");
    crate::contract::testing::touch_first_for_test(false);
}

fn wrapped(open: bool) -> wire::Node {
    let base = container("base", [button("open", "New channel")]);
    if !open {
        return sized("root", base, Some(fill()), Some(fill()));
    }
    let overlay = wire::Node::Overlay {
        id: named_id("create"),
        label: Some("Create channel".into()),
        on_dismiss: None,
        children: vec![
            base,
            container(
                "sheet",
                [
                    input("Name", false, false),
                    handled("save", "Save", 2),
                    handled("cancel", "Cancel", 3),
                ],
            ),
        ],
        style: crate::render::test_style(div().size_full().style().clone()),
    };
    sized("root", overlay, Some(fill()), Some(fill()))
}

fn inside(tree: &gpui_kit::Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext) -> bool {
    let entry = tree.read_with(native, |tree, _| {
        tree.dialogs.values().next().map(|(entry, _)| entry.clone())
    });
    native.update(|window, cx| entry.is_some_and(|entry| entry.contains_focused(window, cx)))
}

#[gpui_kit::test]
fn a_dialog_wrapped_around_its_opener_takes_the_keys(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::AppContext as _;
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(wrapped(false)).with_keys_grant(true));
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = window
        .read_with(cx, |root, _| root.view().clone())
        .unwrap()
        .downcast::<ViewTree>()
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let root = native.update(|_, cx| cx.focus_handle());
    let fallback = native.update(|window, cx| {
        let root = root.clone();
        tree.update(cx, |_, cx| {
            cx.on_focus_lost(window, move |_, window, cx| window.focus(&root, cx))
        })
    });
    native.simulate_click(
        gpui_kit::point(px(10.), px(10.)),
        gpui_kit::Modifiers::none(),
    );
    let opener = native.update(|window, cx| window.focused(cx));
    assert!(opener.is_some(), "a press gave the opener the keys");
    tree.update(&mut native, |tree, cx| tree.replace(wrapped(true), &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    let now = native.update(|window, cx| window.focused(cx));
    assert!(
        inside(&tree, &mut native),
        "the dialog opened with the keys inside it, not on the window's root: focused={now:?} root={root:?}"
    );
    let walk: Vec<_> = ["tab", "tab", "tab", "shift-tab"]
        .into_iter()
        .map(|key| {
            native.simulate_keystrokes(key);
            (holder(&tree, &mut native), inside(&tree, &mut native))
        })
        .collect();
    assert_eq!(
        walk,
        [
            (Some(2), true),
            (Some(3), true),
            (None, true),
            (Some(3), true)
        ],
        "Name, Save, Cancel, round to Name and back: the keys stay in the dialog"
    );
    drop(fallback);
}

fn popover(open: bool) -> wire::Node {
    let mut children = vec![handled("open", "Finish", 1)];
    if open {
        children.push(wire::Node::Deferred {
            priority: 1,
            content: Box::new(wire::Node::Anchored {
                anchor: wire::Anchor::TopRight,
                fit: wire::AnchoredFitMode::SnapToWindow,
                position: Some([0., 0.]),
                position_mode: wire::AnchoredPositionMode::Local,
                offset: None,
                children: vec![container(
                    "form",
                    [
                        handled("submit", "Submit", 2),
                        handled("cancel", "Cancel", 3),
                    ],
                )],
            }),
        });
    }
    let overlay = wire::Node::Overlay {
        id: named_id("finish"),
        label: Some("Finish your review".into()),
        on_dismiss: Some(7),
        children,
        style: crate::render::plain_style(),
    };
    let page = container("page", [overlay, handled("diff", "Diff", 4)]);
    sized("root", page, Some(fill()), Some(fill()))
}

#[gpui_kit::test]
fn a_popover_dialog_takes_the_keys_and_keeps_them(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(popover(false)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let opener = tree
        .read_with(&native, |tree, _| tree.view_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    native.update(|window, cx| opener.focus(window, cx));
    tree.update(&mut native, |tree, cx| tree.replace(popover(true), &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    assert_eq!(
        holder(&tree, &mut native),
        Some(2),
        "the dialog opens on Submit"
    );
    let mut walk = Vec::new();
    for command in [
        wire::WidgetCommand::FocusNext,
        wire::WidgetCommand::FocusNext,
        wire::WidgetCommand::FocusPrevious,
    ] {
        native
            .update(|window, cx| {
                tree.update(cx, |tree, cx| {
                    tree.execute_widget_command(command, window, cx)
                })
            })
            .unwrap();
        walk.push(holder(&tree, &mut native));
    }
    assert_eq!(
        walk,
        [Some(3), Some(2), Some(3)],
        "Cancel, round to Submit and back: never out to the pane"
    );
}

fn held_by_the_dialog(native: &mut Harness<ViewTree>) -> Vec<String> {
    native.update(|window, cx| {
        native.activate_in(window);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let update = &native.tree();
        let nodes: std::collections::HashMap<_, _> =
            update.nodes.iter().map(|(id, node)| (*id, node)).collect();
        let (dialog, _) = update
            .nodes
            .iter()
            .find(|(_, node)| node.role() == gpui_kit::Role::Dialog)
            .expect("a dialog");
        let mut names = Vec::new();
        let mut stack = vec![*dialog];
        while let Some(id) = stack.pop() {
            let node = nodes[&id];
            names.extend(node.label().map(str::to_owned));
            stack.extend(node.children().iter().copied());
        }
        names
    })
}

#[test]
fn a_popover_dialog_holds_its_controls_for_assistive_technology() {
    let mut native = Harness::open_with(gpui_kit::init, size(px(300.), px(200.)), |_, _| {
        ViewTree::new(popover(true))
    });
    let held = held_by_the_dialog(&mut native);
    assert!(
        ["Submit", "Cancel"]
            .iter()
            .all(|name| held.iter().any(|held| held == name)),
        "the dialog holds {held:?}"
    );
}

#[gpui_kit::test]
fn tab_and_escape_hold_in_a_popover_dialog(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::AppContext as _;
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(popover(false)).with_keys_grant(true));
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = window
        .read_with(cx, |root, _| root.view().clone())
        .unwrap()
        .downcast::<ViewTree>()
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    let (events, _subscription) = super::emitted(&tree, &mut native);
    tree.update(&mut native, |tree, cx| tree.replace(popover(true), &[], cx));
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    assert_eq!(
        holder(&tree, &mut native),
        Some(2),
        "the dialog opens on Submit"
    );
    let walk: Vec<_> = ["tab", "tab", "shift-tab", "shift-tab"]
        .into_iter()
        .map(|key| {
            native.simulate_keystrokes(key);
            holder(&tree, &mut native)
        })
        .collect();
    assert_eq!(
        walk,
        [Some(3), Some(2), Some(3), Some(2)],
        "Submit, Cancel and round: the keys stay in the dialog"
    );
    native.simulate_keystrokes("escape");
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, wire::Event::Message(7))),
        "Escape dismissed it"
    );
}

fn people() -> wire::Node {
    listed("people", handled("alice", "Alice", 3))
}

fn listed(key: &str, row: wire::Node) -> wire::Node {
    wire::Node::Deferred {
        priority: 1,
        content: Box::new(wire::Node::Anchored {
            anchor: wire::Anchor::TopRight,
            fit: wire::AnchoredFitMode::SnapToWindow,
            position: Some([0., 0.]),
            position_mode: wire::AnchoredPositionMode::Local,
            offset: None,
            children: vec![container(key, [row])],
        }),
    }
}

fn picking(open: bool, beside: bool) -> wire::Node {
    let mut children = vec![handled("open", "Pick", 1)];
    if open {
        children.push(container("card", [handled("name", "Name", 2), people()]));
    }
    let overlay = wire::Node::Overlay {
        id: named_id("pick"),
        label: Some("Pick a reviewer".into()),
        on_dismiss: Some(7),
        children,
        style: crate::render::plain_style(),
    };
    let mut page = vec![overlay];
    if beside {
        page.push(listed("beside", handled("bob", "Bob", 4)));
    }
    let page = container("page", page);
    sized("root", page, Some(fill()), Some(fill()))
}

#[test]
fn a_dialogs_own_deferred_draw_sits_under_its_node() {
    let mut native = Harness::open_with(gpui_kit::init, size(px(300.), px(200.)), |_, _| {
        ViewTree::new(picking(true, true))
    });
    let held = held_by_the_dialog(&mut native);
    assert!(
        ["Name", "Alice"]
            .iter()
            .all(|name| held.iter().any(|held| held == name)),
        "the dialog holds {held:?}"
    );
    beside_and_out_of_it(&native, &held);
}

fn beside_and_out_of_it(native: &Harness<ViewTree>, held: &[String]) {
    let drawn = native
        .tree()
        .nodes
        .iter()
        .filter_map(|(_, node)| node.label().map(str::to_owned))
        .collect::<Vec<_>>();
    assert!(
        drawn.iter().any(|drawn| drawn == "Bob") && !held.iter().any(|held| held == "Bob"),
        "the draw beside the dialog is drawn, out of it: {drawn:?}, the dialog {held:?}"
    );
}

#[gpui_kit::test]
fn tab_reaches_a_dialogs_own_deferred_draw_and_stays_in_the_dialog(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::AppContext as _;
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(picking(false, false)).with_keys_grant(true));
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = window
        .read_with(cx, |root, _| root.view().clone())
        .unwrap()
        .downcast::<ViewTree>()
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(crate::render::draw_frame);
    native.simulate_click(
        gpui_kit::point(px(10.), px(10.)),
        gpui_kit::Modifiers::none(),
    );
    assert_eq!(
        holder(&tree, &mut native),
        Some(1),
        "the press gave the opener the keys"
    );
    tree.update(&mut native, |tree, cx| {
        tree.replace(picking(true, true), &[], cx)
    });
    native.update(crate::render::draw_frame);
    native.run_until_parked();
    assert_eq!(
        holder(&tree, &mut native),
        Some(2),
        "the dialog opens on Name"
    );
    let walk: Vec<_> = ["tab", "tab", "shift-tab"]
        .into_iter()
        .map(|key| {
            native.simulate_keystrokes(key);
            holder(&tree, &mut native)
        })
        .collect();
    assert_eq!(
        walk,
        [Some(3), Some(2), Some(3)],
        "Name, Alice and round: the keys reach the dialog's menu and stay in the dialog"
    );
}

#[test]
fn a_sheets_own_deferred_draw_sits_under_its_node() {
    let sheet = wire::Node::Sheet {
        id: named_id("actions"),
        label: Some("Message actions".into()),
        style: crate::render::plain_style(),
        detents: Vec::new(),
        on_dismiss: Some(7),
        child: Box::new(container(
            "rows",
            [handled("copy", "Copy text", 2), people()],
        )),
    };
    let beside = listed("beside", handled("bob", "Bob", 4));
    let page = container("page", [button("open", "Actions"), sheet, beside]);
    let root = sized("root", page, Some(fill()), Some(fill()));
    let mut native = Harness::open_with(gpui_kit::init, size(px(300.), px(200.)), move |_, _| {
        ViewTree::new(root)
    });
    let held = held_by_the_dialog(&mut native);
    assert!(
        ["Copy text", "Alice"]
            .iter()
            .all(|name| held.iter().any(|held| held == name)),
        "the sheet holds {held:?}"
    );
    beside_and_out_of_it(&native, &held);
}
