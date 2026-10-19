//! Files the OS drags onto the view: a node that listens for a drop hears
//! it with its paths, which the seat opens for the view.
use super::*;

/// A drop on a node with `on_file_drop` is emitted with its route and the
/// paths dropped, for the seat to open and hold.
#[gpui_kit::test]
fn files_dropped_on_a_listening_node_reach_the_seat_with_its_route(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let mut root = sized(
        "drop",
        text("hint", "Drop files"),
        Some(fill()),
        Some(fill()),
    );
    if let wire::Node::Container(node) = &mut root {
        node.interactivity = Some(Box::new(wire::Interactivity {
            on_file_drop: Some(3),
            ..Default::default()
        }));
    }
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let dropped = Rc::new(RefCell::new(Vec::new()));
    let _subscription = native.update(|_, cx| {
        let dropped = dropped.clone();
        cx.subscribe(&tree, move |_, event: &crate::render::Dropped, _| {
            dropped
                .borrow_mut()
                .push((event.handler, event.paths.clone()));
        })
    });
    native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    let at = gpui_kit::point(px(200.), px(150.));
    let file = std::path::PathBuf::from("/tmp/a.txt");
    let paths = gpui_kit::ExternalPaths(vec![file.clone()].into());
    native.simulate_event(gpui_kit::FileDropEvent::Entered {
        position: at,
        paths,
    });
    native.simulate_event(gpui_kit::FileDropEvent::Submit { position: at });
    assert_eq!(*dropped.borrow(), [(3, vec![file])]);
}
