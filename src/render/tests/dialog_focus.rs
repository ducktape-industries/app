//! A view dialog gives the keyboard back to what opened it (AX-120).
use super::*;

/// A focusable button Tab reaches.
fn button(key: &str, name: &str) -> wire::Node {
    let mut node = container_with_style(key, div().w(px(80.)).h(px(24.)).style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.aria.label = Some(name.into());
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
    }
    node
}

/// The opener, which the guest names by focus handle 1, beside an overlay
/// whose dialog, a Save button, shows when `open`.
fn screen(open: bool) -> wire::Node {
    let mut opener = button("open", "Rename");
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut opener {
        interactivity.focus_handle = Some(1);
    }
    let mut children = vec![container("base", [opener])];
    if open {
        children.push(container("sheet", [button("save", "Save")]));
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
