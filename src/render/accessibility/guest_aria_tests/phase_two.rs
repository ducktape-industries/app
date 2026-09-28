//! What phase 2 added to the one mapper: the aria gpui has no setter for,
//! through one `a11y::Patch`; a Status's words as its value; a List and a
//! ResizeHandle carrying a view's interactivity.
use super::*;

/// Every phase-2 field a view sets that gpui has no setter for, on a
/// roled, named node.
fn phase_two(role: Role) -> wire::Interactivity {
    wire::Interactivity {
        role: Some(role),
        aria: wire::Aria {
            author_id: Some("child".into()),
            label: Some("Phase two".into()),
            live: Some(accesskit::Live::Polite),
            busy: true,
            required: true,
            read_only: true,
            invalid: Some(accesskit::Invalid::Spelling),
            has_popup: Some(accesskit::HasPopup::Menu),
            current: Some(accesskit::AriaCurrent::Page),
            custom_actions: vec![(3, "Pin".into())],
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The node `root` builds for author id `child`, once the tree is on.
fn built(cx: &mut gpui_kit::TestAppContext, root: wire::Node) -> Option<accesskit::Node> {
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        update
            .nodes
            .iter()
            .find(|(_, node)| node.author_id() == Some("child"))
            .map(|(_, node)| node.clone())
    })
}

/// Each phase-2 aria field reaches the node, all of them through one
/// patch (a second closure would replace the first), on every renderer
/// that carries a view's interactivity: a ResizeHandle and a List too.
#[gpui_kit::test]
fn every_renderer_puts_the_phase_two_aria_on_its_node(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for site in [
        Site::Container,
        Site::Image,
        Site::UniformListRow,
        Site::ResizeHandle,
        Site::List,
    ] {
        let node = built(cx, child(&site, phase_two(Role::Button)))
            .unwrap_or_else(|| panic!("{site:?} has a node"));
        assert_eq!(node.role(), Role::Button);
        assert_eq!(node.label(), Some("Phase two"));
        assert_eq!(node.live(), Some(accesskit::Live::Polite));
        assert!(node.is_busy());
        assert!(node.is_required());
        assert!(node.is_read_only());
        assert_eq!(node.invalid(), Some(accesskit::Invalid::Spelling));
        assert_eq!(node.has_popup(), Some(accesskit::HasPopup::Menu));
        assert_eq!(node.aria_current(), Some(accesskit::AriaCurrent::Page));
        assert_eq!(
            node.custom_actions()
                .iter()
                .map(|action| (action.id, &*action.description))
                .collect::<Vec<_>>(),
            [(3, "Pin")]
        );
    }
}

/// A list the view left plain is gpui's list alone: no box, no node.
#[gpui_kit::test]
fn a_plain_list_has_no_node(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let mut plain = wire::Interactivity::default();
    let node = built(cx, child(&Site::List, plain.clone()));
    assert!(node.is_none());
    plain.aria.author_id = Some("child".into());
    plain.role = Some(Role::List);
    assert_eq!(
        built(cx, child(&Site::List, plain)).map(|node| node.role()),
        Some(Role::List)
    );
}

/// A Status or Alert the view did not give a value speaks the words it
/// draws as its value as well as its name: macOS reads the value.
#[gpui_kit::test]
fn a_status_speaks_its_drawn_words_as_its_value(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for role in [Role::Status, Role::Alert] {
        let status = wire::Interactivity {
            role: Some(role),
            aria: wire::Aria {
                author_id: Some("child".into()),
                live: Some(accesskit::Live::Polite),
                ..Default::default()
            },
            ..Default::default()
        };
        let node = built(cx, child(&Site::Container, status)).expect("the status has a node");
        assert_eq!(node.label(), Some(WORDS));
        assert_eq!(node.value(), Some(WORDS));
    }
}

/// A divider as the SDK builds it is a named Splitter Tab reaches, and a
/// key pressed on it goes to the view.
#[gpui_kit::test]
fn a_resize_handle_takes_focus_and_keys(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let divider = wire::Interactivity {
        role: Some(Role::Splitter),
        focusable: true,
        tab_stop: Some(true),
        on_key_down: Some(9),
        aria: wire::Aria {
            author_id: Some("child".into()),
            label: Some("Resize the list".into()),
            orientation: Some(accesskit::Orientation::Vertical),
            ..Default::default()
        },
        ..Default::default()
    };
    let root = child(&Site::ResizeHandle, divider);
    let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            seen.borrow_mut().push(event.clone())
        })
    });
    // Tab is the app's binding to `focus_next`; a bare window has none
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.focus_next(cx);
    });
    native.simulate_keystrokes("left");
    native.update(|window, cx| {
        window.render_frame(cx);
        let update = window.a11y_tree().expect("an a11y tree once activated");
        let (id, node) = update
            .nodes
            .iter()
            .find(|(_, node)| node.author_id() == Some("child"))
            .expect("the divider has a node");
        assert_eq!(node.role(), Role::Splitter);
        assert_eq!(node.label(), Some("Resize the list"));
        assert_eq!(update.focus, *id, "Tab reaches the divider");
    });
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, wire::Event::KeyDown { handler: 9, .. })),
        "{:?}",
        events.borrow()
    );
}

/// A handle a view names on a divider or a list outlives the frame that
/// made it: a new frame keeps focus where it was.
#[gpui_kit::test]
fn a_guest_focus_handle_on_a_divider_or_a_list_survives_a_frame(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    for site in [Site::ResizeHandle, Site::List] {
        let focused = wire::Interactivity {
            focus_handle: Some(PARENT_FOCUS),
            ..phase_two(Role::Button)
        };
        let root = child(&site, focused);
        let again = root.clone();
        let window = cx.open_window(size(px(300.), px(200.)), |_, _| ViewTree::new(root));
        let tree = window.root(cx).unwrap();
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        native.update(|window, cx| {
            window.render_frame(cx);
            let handle = tree.read(cx).guest_focus_targets[&PARENT_FOCUS].clone();
            tree.update(cx, |tree, cx| tree.replace(again, cx));
            window.render_frame(cx);
            assert!(
                tree.read(cx).guest_focus_targets[&PARENT_FOCUS] == handle,
                "{site:?} made its handle anew"
            );
        });
    }
}
