//! A rich text's links are one Tab stop: the arrows pick one, Enter presses
//! it as a pointer would, Tab moves on.
use super::*;

fn rich() -> wire::Node {
    wire::Node::RichText {
        id: Some(named_id("rich")),
        style: Default::default(),
        text: "Read the docs or the code".into(),
        runs: wire::RichTextRuns::Highlights(Vec::new()),
        font_family_overrides: Vec::new(),
        clickable_ranges: vec![5..13, 17..25],
        on_click: Some(72),
        on_hover: None,
        tooltip: None,
    }
}

/// A Tab stop after the text, a button called `name`.
fn button(name: &str) -> wire::Node {
    let mut node = container_with_style("next", div().w(px(80.)).h(px(24.)).style().clone(), []);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        interactivity.role = Some(gpui_kit::Role::Button);
        interactivity.aria.label = Some(name.into());
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
    }
    node
}

/// The name of the node assistive technology sees focused after `stroke`.
/// Tab is the app's binding to `focus_next`; a bare window has none.
fn press(native: &mut gpui_kit::VisualTestContext, stroke: &str) -> Option<String> {
    native.update(|window, cx| {
        match stroke {
            "tab" => window.focus_next(cx),
            _ => {
                window.dispatch_keystroke(Keystroke::parse(stroke).unwrap(), cx);
            }
        }
        window.render_frame(cx);
        let tree = window.a11y_tree().unwrap();
        tree.nodes
            .iter()
            .find(|(id, _)| *id == tree.focus)
            .and_then(|(_, node)| node.label().map(str::to_owned))
    })
}

/// Tab lands on the text with its first link picked; the arrows, Home and
/// End move the pick along the links without wrapping; a press from
/// assistive technology on the picked link is its range's click, no
/// gesture; Enter is the picked range's click, a real gesture; Tab leaves.
#[gpui_kit::test]
fn tab_reaches_the_links_the_arrows_pick_one_and_enter_presses_it(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let root = axis_container("root", Axis::Column, [rich(), button("Next")]);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            observed.borrow_mut().push(event.clone());
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
    });
    assert_eq!(press(&mut native, "tab").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "right").as_deref(), Some("the code"));
    assert_eq!(press(&mut native, "right").as_deref(), Some("the code"));
    assert_eq!(press(&mut native, "home").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "end").as_deref(), Some("the code"));
    assert_eq!(press(&mut native, "left").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "left").as_deref(), Some("the docs"));
    assert_eq!(press(&mut native, "down").as_deref(), Some("the code"));
    native.update(|window, cx| {
        use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
        let focus = window.a11y_tree().unwrap().focus;
        window.dispatch_a11y_action(
            ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: focus,
                data: None,
            },
            cx,
        );
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
        assert!(tree.take_user_activation(&code).is_none());
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
            tree.take_user_activation(&pressed[0]).is_some(),
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

/// The view is drawn again on the frame the keys arrive at the text and on
/// the one they leave: the box is drawn from whether it has the keys, and a
/// cached view replays paint until something asks for a redraw. A focus
/// change refreshes the window, which redraws every cached view.
#[gpui_kit::test]
fn the_keys_arriving_at_the_links_redraw_a_cached_view(cx: &mut gpui_kit::TestAppContext) {
    struct Cached(Entity<ViewTree>);
    impl Render for Cached {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.0
                    .clone()
                    .cached(gpui_kit::StyleRefinement::default().size_full()),
            )
        }
    }
    cx.update(gpui_kit::init);
    let root = axis_container("root", Axis::Column, [rich(), button("Next")]);
    let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
        Cached(cx.new(|_| ViewTree::new(root)))
    });
    let seat = window.root(cx).unwrap();
    let tree = seat.read_with(cx, |cached, _| cached.0.clone());
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    // the desk draws its seats again each frame; the view replays unless
    // something in it asked to be drawn
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

/// A rich text without links is no Tab stop.
#[gpui_kit::test]
fn a_rich_text_without_links_is_no_tab_stop(cx: &mut gpui_kit::TestAppContext) {
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
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(plain));
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
    });
    assert_eq!(press(&mut native, "tab"), None);
    assert!(native.update(|window, cx| window.focused(cx)).is_none());
}

/// Space on a text's links is theirs: it presses nothing, and it does not
/// bubble to a composite around the text (a message list whose Space
/// presses its active message), which hears every other key the box does
/// not take.
#[gpui_kit::test]
fn space_on_a_link_box_does_not_reach_the_composite_around_it(cx: &mut gpui_kit::TestAppContext) {
    const KEYS: u32 = 9;
    cx.update(gpui_kit::init);
    let mut grid = axis_container("messages", Axis::Column, [rich()]);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut grid {
        interactivity.role = Some(gpui_kit::Role::Grid);
        interactivity.aria.label = Some("Messages".into());
        interactivity.focusable = true;
        interactivity.tab_stop = Some(true);
        interactivity.on_key_down = Some(KEYS);
    }
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(grid));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = heard.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::KeyDown {
                handler: KEYS,
                event,
                ..
            } = event
            {
                seen.borrow_mut()
                    .push(event.clone().into_gpui().keystroke.key);
            }
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
    });
    assert_eq!(press(&mut native, "tab").as_deref(), Some("Messages"));
    assert_eq!(press(&mut native, "tab").as_deref(), Some("the docs"));
    press(&mut native, "space");
    press(&mut native, "x");
    assert_eq!(*heard.borrow(), ["x"], "Space stayed in the box");
}
