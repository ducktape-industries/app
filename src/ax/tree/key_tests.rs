//! The keys a node carries beyond role, name, value, states and actions.
use super::*;
use crate::a11y::Patch;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
};

/// A modal dialog holding a list whose one row has every key.
struct Keys;

impl Render for Keys {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = Patch {
            live: Some(Live::Polite),
            invalid: Some(Invalid::True),
            required: true,
            has_popup: Some(HasPopup::Listbox),
            ..Default::default()
        }
        .on(div()
            .id("row")
            .size(px(20.))
            .focusable()
            .tab_stop(true)
            .role(Role::ListBoxOption)
            .aria_label("One")
            .aria_selected(false)
            .aria_level(2)
            .aria_placeholder("hint")
            .aria_keyshortcuts("Ctrl+1")
            .aria_position_in_set(1)
            .aria_size_of_set(3));
        let list = div()
            .id("list")
            .size(px(100.))
            .role(Role::ListBox)
            .aria_label("Rows")
            .child(row);
        crate::a11y::modal(
            div()
                .id("dialog")
                .size(px(200.))
                .role(Role::Dialog)
                .aria_label("Pick")
                .child(list),
        )
    }
}

/// Every key the door adds, each from what the node carries; the parent
/// from the walk; the composite's active descendant from where focus is.
#[gpui_kit::test]
fn the_door_says_what_else_a_node_carries(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(300.), px(300.)), |_, _| Keys);
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let nodes = native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.focus_next(cx);
        window.render_frame(cx);
        window.render_frame(cx);
        snapshot("t", window, false)
    });
    let json = |id: &str| {
        let node = nodes.iter().find(|node| node.id == id).expect(id);
        serde_json::to_value(node).unwrap()
    };
    assert_eq!(
        json("t:dialog"),
        serde_json::json!({
            "id": "t:dialog", "role": "Dialog", "name": "Pick", "state": [], "actions": [],
            "modal": true, "in": "t",
        })
    );
    assert_eq!(
        json("t:list"),
        serde_json::json!({
            "id": "t:list", "role": "ListBox", "name": "Rows", "state": [], "actions": [],
            "active_descendant": "t:row", "parent": "t:dialog", "in": "t",
        })
    );
    assert_eq!(
        json("t:row"),
        serde_json::json!({
            "id": "t:row", "role": "ListBoxOption", "name": "One",
            "state": ["focused", "unselected"], "actions": ["focus"],
            "live": "polite", "level": 2, "placeholder": "hint",
            "keyboard_shortcut": "Ctrl+1", "position_in_set": 1, "size_of_set": 3,
            "invalid": "true", "required": true, "has_popup": "listbox",
            "parent": "t:list", "in": "t",
        })
    );
}
