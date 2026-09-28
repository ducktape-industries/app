use super::*;

#[test]
fn a_modal_below_a_hidden_ancestor_does_not_replace_the_visible_modal() {
    let (root_id, visible_id, hidden_id, hidden_modal_id) =
        (NodeId(1), NodeId(2), NodeId(3), NodeId(4));
    let mut root = gpui_kit::accesskit::Node::new(Role::Window);
    root.set_children([visible_id, hidden_id]);
    let mut visible = gpui_kit::accesskit::Node::new(Role::Dialog);
    visible.set_modal();
    let mut hidden = gpui_kit::accesskit::Node::new(Role::GenericContainer);
    hidden.set_hidden();
    hidden.set_children([hidden_modal_id]);
    let mut hidden_modal = gpui_kit::accesskit::Node::new(Role::Dialog);
    hidden_modal.set_modal();
    let nodes = HashMap::from([
        (root_id, &root),
        (visible_id, &visible),
        (hidden_id, &hidden),
        (hidden_modal_id, &hidden_modal),
    ]);

    assert_eq!(topmost_modal(root_id, &nodes), Some(visible_id));
}
