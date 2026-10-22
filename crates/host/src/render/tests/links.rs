//! A rich text's links are one Tab stop: the arrows pick one, Enter presses
//! it as a pointer would, Tab moves on.
use super::*;
use ducktape_a11y_platform::headless::Harness;

fn press(native: &mut Harness<ViewTree>, stroke: &str) -> Option<String> {
    native.update(|window, cx| {
        match stroke {
            "tab" => window.focus_next(cx),
            _ => {
                window.dispatch_keystroke(Keystroke::parse(stroke).unwrap(), cx);
            }
        }
        crate::render::draw_frame(window, cx);
        let tree = &native.tree();
        tree.nodes
            .iter()
            .find(|(id, _)| *id == tree.focus)
            .and_then(|(_, node)| node.label().map(str::to_owned))
    })
}

#[test]
fn tab_reaches_the_links_the_arrows_pick_one_and_enter_presses_it() {
    let root = axis_container("root", Axis::Column, [rich(), button("next", "Next")]);
    let mut native = Harness::open_with(gpui_kit::init, size(px(400.), px(300.)), |_, _| {
        ViewTree::new(root)
    });
    let tree = native.root();
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        native.activate_in(window);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    assert_eq!(press(&mut native, "tab").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "right").as_deref(), Some("the code"));
    assert_eq!(press(&mut native, "right").as_deref(), Some("the code"));
    assert_eq!(press(&mut native, "home").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "end").as_deref(), Some("the code"));
    assert_eq!(press(&mut native, "left").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "left").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "down").as_deref(), Some("the code"));
    native.act(gpui_kit::accesskit::ActionRequest {
        action: gpui_kit::accesskit::Action::Click,
        target_tree: gpui_kit::accesskit::TreeId::ROOT,
        target_node: native.tree().focus,
        data: None,
    });
    let code = wire::Event::Select {
        handler: 72,
        index: 1,
    };
    assert_eq!(
        events.borrow_mut().drain(..).collect::<Vec<_>>(),
        std::slice::from_ref(&code)
    );
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "the reader's press is a gesture, as a pointer's is"
        );
    });
    assert_eq!(press(&mut native, "up").as_deref(), Some("the docs"));
    assert!(events.borrow().is_empty(), "picking presses nothing");
    press(&mut native, "enter");
    let pressed = events.borrow();
    assert_eq!(
        pressed.as_slice(),
        [wire::Event::Select {
            handler: 72,
            index: 0
        }]
    );
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "Enter is a real gesture, as a click is"
        );
    });
    drop(pressed);
    assert_eq!(
        press(&mut native, "tab").as_deref(),
        Some("Next"),
        "one Tab stop, not one per link: Tab leaves the text"
    );
}

#[gpui_kit::test]
fn the_keys_arriving_at_the_links_redraw_a_cached_view(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let root = axis_container("root", Axis::Column, [rich(), button("next", "Next")]);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        Seat(cx.new(|_| ViewTree::new(root)))
    });
    let seat = window.root(cx).unwrap();
    let tree = seat.read_with(cx, |seat, _| seat.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let settle = |native: &mut gpui_kit::VisualTestContext| {
        for _ in 0..3 {
            native.update(|window, cx| {
                seat.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            });
            native.run_until_parked();
        }
        tree.read_with(native, |tree, _| tree.renders)
    };
    let still = settle(&mut native);
    assert_eq!(settle(&mut native), still, "a still view replays");
    native.update(|window, cx| window.focus_next(cx));
    let arrived = settle(&mut native);
    assert!(arrived > still, "the keys arriving redraw");
    assert_eq!(settle(&mut native), arrived, "then the view is still again");
    native.update(|window, cx| window.focus_next(cx));
    assert!(settle(&mut native) > arrived, "the keys leaving redraw");
}

#[test]
fn a_rich_text_without_links_is_no_tab_stop() {
    let wire::Node::RichText {
        id,
        style,
        text,
        runs,
        font_family_overrides,
        ..
    } = rich()
    else {
        unreachable!()
    };
    let plain = wire::Node::RichText {
        id,
        style,
        text,
        runs,
        font_family_overrides,
        clickable_ranges: Vec::new(),
        on_click: None,
        on_hover: None,
        tooltip: None,
    };
    let mut native = Harness::open_with(gpui_kit::init, size(px(400.), px(300.)), |_, _| {
        ViewTree::new(plain)
    });
    native.update(|window, cx| {
        native.activate_in(window);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    assert_eq!(press(&mut native, "tab"), None);
    assert!(native.update(|window, cx| window.focused(cx)).is_none());
}

#[test]
fn space_on_a_link_box_presses_its_link_and_does_not_reach_the_composite_around_it() {
    const KEYS: u32 = 9;
    let mut grid = axis_container("messages", Axis::Column, [rich()]);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut grid {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(gpui_kit::Role::Grid);
        interactivity.aria.label = Some("Messages".into());
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
        interactivity.on_key_down = Some(KEYS);
    }
    let mut native = Harness::open_with(gpui_kit::init, size(px(400.), px(300.)), |_, _| {
        ViewTree::new(grid)
    });
    let tree = native.root();
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(|window, cx| {
        native.activate_in(window);
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    assert_eq!(press(&mut native, "tab").as_deref(), Some("Messages"));
    assert_eq!(press(&mut native, "tab").as_deref(), Some("the docs"));
    press(&mut native, "space");
    press(&mut native, "x");
    let heard: Vec<String> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            wire::Event::KeyDown {
                handler: KEYS,
                event,
                ..
            } => Some(event.clone().into_gpui().keystroke.key),
            wire::Event::Select { handler: 72, index } => Some(format!("link {index}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        heard,
        ["link 0", "x"],
        "Space pressed the link and stayed in the box"
    );
}
