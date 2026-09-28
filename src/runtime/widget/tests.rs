use super::*;

/// `EditorStore` keys every editor by the `AuthoredPath` walked from the
/// guest's OWN root (`guest/requests.rs`'s `tick`, never wrapped). The
/// native widget tree is built from `native_root(root)` instead — the
/// wrapper `native_root` adds around that same root so an unsized guest
/// root still fills the seat. If that wrapper carried an id, every
/// descendant's `AuthoredPath` as walked from the RENDERED tree would
/// carry one extra leading segment the store never indexed under, and a
/// native editor field could never find its `EditorStore` entry: it would
/// keep typing locally (GPUI's own default text handling on an
/// editable-by-default field) while the guest's document — and everything
/// gated on it, like a claimed Enter or the Send button — never moved.
/// Reproduces that class of bug directly against the two real tree walks,
/// with no gpui window needed.
#[test]
fn native_root_does_not_shift_the_authored_path_editor_store_indexes_by() {
    let editor = wire::Node::Editor {
        options: Box::new(wire::EditorOptions::default()),
        id: wire::ElementIdWire::Name("editor".into()),
        style: Default::default(),
        placeholder: String::new(),
        label: None,
        document: wire::editor_document::EditorDocumentRef {
            document: "doc".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: wire::EditorCursor::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
    };
    let panel = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("panel".into())),
        style: Default::default(),
        interactivity: Default::default(),
        children: vec![editor],
    });

    // The guest's own root, unwrapped: what `EditorStore::replace` indexes,
    // exactly as `guest/requests.rs`'s `tick` calls it.
    let store = EditorStore::new(0);
    store
        .replace(&panel)
        .expect("a valid editor tree validates");

    // The tree the renderer actually walks to mount native widgets — the
    // one and only tree `native_root` ever produces for it.
    let rendered = native_root(panel);
    let mounted_key = editor_authored_path(&rendered).expect("the editor is still in the tree");

    assert!(
        store.projection(&mounted_key).is_some(),
        "a native editor's own mounted path must resolve in the EditorStore \
         the guest's unwrapped tree populated; native_root must add no identity"
    );
}

/// The `AuthoredPath` to the first `Editor` node in `root`, walked the same
/// way `crate::render`'s node lowering (and `EditorStore::collect`) do: by
/// `crate::render::enter_scope`, which is what decides whether a node
/// contributes a path segment at all.
fn editor_authored_path(root: &wire::Node) -> Option<crate::render::AuthoredPath> {
    fn walk(
        node: &wire::Node,
        path: &mut crate::render::AuthoredPath,
    ) -> Option<crate::render::AuthoredPath> {
        let entered = crate::render::enter_scope(node, path);
        let found = matches!(node, wire::Node::Editor { .. })
            .then(|| path.clone())
            .or_else(|| node.children().iter().find_map(|child| walk(child, path)));
        if entered {
            path.pop();
        }
        found
    }
    walk(root, &mut crate::render::AuthoredPath::new())
}

#[gpui_kit::test]
fn same_module_instances_receive_independent_props(cx: &mut gpui_kit::TestAppContext) {
    let first = cx.new(|_| NativeModuleView::new("independent-props-test"));
    let second = cx.new(|_| NativeModuleView::new("independent-props-test"));
    first.update(cx, |view, cx| view.set_props(b"channel-one".to_vec(), cx));
    second.update(cx, |view, cx| view.set_props(b"channel-two".to_vec(), cx));
    cx.update(|cx| {
        let first = first.read(cx);
        let second = second.read(cx);
        assert_ne!(first.instance, second.instance);
        assert!(!Arc::ptr_eq(&first.seat, &second.seat));
        assert_eq!(
            first.seat.lock().unwrap().props.as_deref(),
            Some(b"channel-one".as_slice())
        );
        assert_eq!(
            second.seat.lock().unwrap().props.as_deref(),
            Some(b"channel-two".as_slice())
        );
    });
    first.update(cx, |view, cx| view.set_props(b"channel-three".to_vec(), cx));
    cx.update(|cx| {
        assert_eq!(
            second.read(cx).seat.lock().unwrap().props.as_deref(),
            Some(b"channel-two".as_slice())
        );
    });
}
