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
        ViewTree::new(screen(false))
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
        let tree = cx.new(|_| ViewTree::new(screen(false)));
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

/// A screen whose dialog the view wraps around it only once it opens
/// (chat's Create channel, chat-view `ui/mod.rs` `with_create`): the
/// opener, a button with no guest focus handle, moves under the overlay,
/// so its path changes — and its host focus handle dies — as it opens.
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
        style: div().size_full().style().clone(),
    };
    sized("root", overlay, Some(fill()), Some(fill()))
}

/// Whether the keys are in the one open dialog (its trap's handle contains
/// the focused element).
fn inside(tree: &gpui_kit::Entity<ViewTree>, native: &mut gpui_kit::VisualTestContext) -> bool {
    let entry = tree.read_with(native, |tree, _| {
        tree.dialogs.values().next().map(|(entry, _)| entry.clone())
    });
    native.update(|window, cx| entry.is_some_and(|entry| entry.contains_focused(window, cx)))
}

/// In a window whose root takes the keys back when the focused element
/// vanishes (as the app's `WindowRoot::focus_lost` does, shell/layers/root.rs),
/// a dialog wrapped around its opener as it opens still takes the keyboard:
/// it opens on its first control and Tab goes round it.
#[gpui_kit::test]
fn a_dialog_wrapped_around_its_opener_takes_the_keys(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::AppContext as _;
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(200.)), |window, cx| {
        let tree = cx.new(|_| ViewTree::new(wrapped(false)));
        gpui_kit::component::Root::new(tree, window, cx)
    });
    let tree = window
        .read_with(cx, |root, _| root.view().clone())
        .unwrap()
        .downcast::<ViewTree>()
        .unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
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
    tree.update(&mut native, |tree, cx| tree.replace(wrapped(true), cx));
    native.update(|window, cx| window.render_frame(cx));
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
