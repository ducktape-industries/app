//! A view dialog gives the keyboard back to what opened it (AX-120).
use super::*;

/// `button`, which the guest names by focus `handle`.
fn handled(key: &str, name: &str, handle: u64) -> wire::Node {
    let mut node = button(key, name);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        interactivity.focus_handle = Some(handle);
    }
    node
}

/// The opener (focus handle 1) beside an overlay whose dialog, Save (2)
/// and Cancel (3), shows when `open`.
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
        style: div().size_full().style().clone(),
    };
    sized("root", overlay, Some(fill()), Some(fill()))
}

/// The dialog takes the keyboard as it opens, and once it closes the
/// button that opened it has it again, not nothing.
#[gpui_kit::test]
fn a_closed_dialog_gives_focus_back_to_its_opener(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(screen(false)).with_keys_grant(true)
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let opener = tree
        .read_with(&native, |tree, _| tree.guest_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    let show = |native: &mut gpui_kit::VisualTestContext, open: bool| {
        tree.update(native, |tree, cx| tree.replace(screen(open), cx));
        native.update(|window, cx| window.render_frame(cx));
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
    assert_eq!(
        show(&mut native, false).as_ref(),
        Some(&opener),
        "the opener has the keyboard back"
    );
}

/// A dialog whose guest may not move the keys (`ViewTree::keys_grant`
/// false: a background pane, or under Spotlight, with no gesture) opens
/// without taking them: the keys stay where they were.
#[gpui_kit::test]
fn a_dialog_takes_no_keys_without_the_grant(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| {
        ViewTree::new(screen(false))
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    let opener = tree
        .read_with(&native, |tree, _| tree.guest_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    native.update(|window, cx| opener.focus(window, cx));
    tree.update(&mut native, |tree, cx| tree.replace(screen(true), cx));
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    assert_eq!(
        native.update(|window, cx| window.focused(cx)).as_ref(),
        Some(&opener),
        "the dialog took the keyboard with no grant"
    );
}

/// The guest focus handle that has the keys, if one has.
fn holder(
    tree: &gpui_kit::Entity<ViewTree>,
    native: &mut gpui_kit::VisualTestContext,
) -> Option<u64> {
    let focused = native.update(|window, cx| window.focused(cx))?;
    tree.read_with(native, |tree, _| {
        tree.guest_focus_targets
            .iter()
            .find_map(|(handle, focus)| (*focus == focused).then_some(*handle))
    })
}

/// Under the kit's root (what answers Tab), a view dialog opens with the
/// keys on its first control, and Tab and Shift+Tab go round its controls,
/// never out to the opener behind it.
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
    native.update(|window, cx| window.render_frame(cx));
    let opener = tree
        .read_with(&native, |tree, _| tree.guest_focus_targets.get(&1).cloned())
        .expect("the opener is drawn");
    native.update(|window, cx| opener.focus(window, cx));
    tree.update(&mut native, |tree, cx| tree.replace(screen(true), cx));
    native.update(|window, cx| window.render_frame(cx));
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
