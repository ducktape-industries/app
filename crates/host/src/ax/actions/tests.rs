//! Regression tests for the AX door's field path: `perform_by_id` on a
//! nested composer field must reach the view as the change it is; an
//! action a view advertises reaches the view; and a press on a view's
//! control is that control's click, wherever it is drawn.

use super::*;
use crate::render::ViewTree;
use gpui_kit::{px, size};
use view_wire as wire;

fn named_id(key: &str) -> wire::ElementIdWire {
    wire::ElementIdWire::Name(key.into())
}

fn container(key: &str, children: Vec<wire::Node>) -> wire::Node {
    use gpui_kit::Styled as _;
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: crate::render::test_style(gpui_kit::div().size_full().style().clone()),
        interactivity: Default::default(),
        children,
    })
}

fn chat_shaped_field() -> wire::Node {
    let field = wire::Node::Field {
        id: named_id("draft-general/editor"),
        multiline: true,
        value: String::new(),
        cursor: Default::default(),
        generation: 1,
        revision: 0,
        tokens: Default::default(),
        claims: Default::default(),
        options: Box::new(wire::InputOptions {
            label: "Message #general".into(),
            ..Default::default()
        }),
        placeholder: String::new(),
        secure: false,
        on_change: Some(1),
        on_key: Some(2),
        on_submit: None,
        style: crate::render::plain_style(),
    };
    container(
        "chat-viewport",
        vec![container(
            "chat-root",
            vec![container(
                "chat-panes",
                vec![container(
                    "chat-room",
                    vec![container("draft-general", vec![field])],
                )],
            )],
        )],
    )
}

fn ax_action_reaches_the_views_field(cx: &mut gpui_kit::TestAppContext, action: &str) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| {
        ViewTree::new(chat_shaped_field())
    });
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = heard.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::Text { handler, change } = event {
                seen.borrow_mut().push((*handler, change.text.clone()));
            }
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    let performed = native.update(|window, cx| {
        perform_by_id("t", window, cx, "t:draft-general/editor", action, "hello")
    });
    assert!(performed, "the composer's AX field must be in the tree");
    native.update(|window, cx| {
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
    });
    native.run_until_parked();
    assert_eq!(
        heard.borrow().last(),
        Some(&(1, "hello".to_owned())),
        "AX `{action}` on the composer's field must reach the view as its change"
    );
}

#[gpui_kit::test]
fn ax_type_on_a_nested_editor_reaches_the_views_document(cx: &mut gpui_kit::TestAppContext) {
    ax_action_reaches_the_views_field(cx, "type");
}

#[gpui_kit::test]
fn ax_set_value_on_a_nested_editor_reaches_the_views_document(cx: &mut gpui_kit::TestAppContext) {
    ax_action_reaches_the_views_field(cx, "set_value");
}

#[gpui_kit::test]
fn an_action_a_view_advertises_reaches_the_view(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::accesskit::Action as A;
    cx.update(gpui_kit::init);
    let mut stepper = container("stepper", Vec::new());
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut stepper {
        let interactivity = interactivity.get_or_insert_default();
        interactivity.role = Some(Role::SpinButton);
        interactivity.aria.label = Some("Count".into());
        interactivity.aria.numeric_value = Some(3.);
        interactivity.aria.actions =
            vec![(A::Increment, 7), (A::Decrement, 8), (A::CustomAction, 9)];
        interactivity.aria.custom_actions = vec![(4, "Reset".into())];
    }
    let root = container("chat-viewport", vec![stepper]);
    let window = cx.open_window(size(px(200.), px(120.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::A11yAction { handler, data } = event {
                seen.borrow_mut().push((*handler, data.clone()));
            }
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let node = snapshot("t", window, false)
            .into_iter()
            .find(|node| node.role == "SpinButton")
            .expect("the stepper is in the tree");
        assert_eq!(node.actions, ["increment", "decrement"]);
        for word in ["increment", "decrement"] {
            assert!(perform_by_id("t", window, cx, &node.id, word, ""));
            crate::render::draw_frame(window, cx);
        }
        window.dispatch_a11y_action(
            ActionRequest {
                action: A::CustomAction,
                target_tree: TreeId::ROOT,
                target_node: node.node,
                data: Some(ActionData::CustomAction(4)),
            },
            cx,
        );
    });
    assert_eq!(
        *events.borrow(),
        [(7, None), (8, None), (9, Some(ActionData::CustomAction(4)))]
    );
}

fn pressable(
    key: &str,
    role: Role,
    name: String,
    style: gpui_kit::StyleRefinement,
    click: u32,
) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id(key)),
        style: crate::render::test_style(style),
        interactivity: Some(Box::new(wire::Interactivity {
            role: Some(role),
            aria: wire::Aria {
                label: Some(name.into()),
                ..Default::default()
            },
            on_click: Some(click),
            ..Default::default()
        })),
        children: Vec::new(),
    })
}

#[gpui_kit::test]
fn a_press_on_a_view_row_scrolled_out_of_its_list_is_that_rows_click(
    cx: &mut gpui_kit::TestAppContext,
) {
    use gpui_kit::Styled as _;
    cx.update(gpui_kit::init);
    let rows = (0..10).map(|n| {
        pressable(
            &format!("row-{n}"),
            Role::ListBoxOption,
            format!("Row {n}"),
            gpui_kit::div().h(px(40.)).flex_shrink_0().style().clone(),
            10 + n,
        )
    });
    let mut scrolls = gpui_kit::div()
        .flex()
        .flex_col()
        .w(px(200.))
        .h(px(100.))
        .style()
        .clone();
    scrolls.overflow.y = Some(gpui_kit::Overflow::Scroll);
    let list = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("list")),
        style: crate::render::test_style(scrolls),
        interactivity: Default::default(),
        children: rows.collect(),
    });
    let over = pressable(
        "over",
        Role::Button,
        "Over".into(),
        gpui_kit::div()
            .absolute()
            .top(px(200.))
            .left(px(0.))
            .w(px(200.))
            .h(px(40.))
            .style()
            .clone(),
        99,
    );
    let root = wire::Node::Container(view_wire::ContainerNode {
        id: Some(named_id("root")),
        style: crate::render::test_style(gpui_kit::div().relative().size(px(300.)).style().clone()),
        interactivity: Default::default(),
        children: vec![list, over],
    });
    let window = cx.open_window(size(px(300.), px(300.)), |_, _| ViewTree::new(root));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = native.update(|_, cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            seen.borrow_mut().push(event.clone());
        })
    });
    native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let nodes = snapshot("t", window, true);
        let row = nodes
            .iter()
            .find(|node| node.name == "Row 5")
            .expect("the door shows the row scrolled out of its list");
        let over = nodes
            .iter()
            .find(|node| node.name == "Over")
            .expect("the button is in the tree");
        assert!(row.actions.contains(&"press"));
        assert_eq!(row.bounds, Some([0, 200, 200, 240]));
        assert_eq!(over.bounds, row.bounds);
        assert!(perform_by_id("t", window, cx, &row.id, "press", ""));
    });
    let events = events.borrow();
    let [
        wire::Event::Click {
            handler: 15,
            event: wire::click::Click::Keyboard { button, bounds },
        },
    ] = &events[..]
    else {
        panic!("row 5's click alone, not {events:?}");
    };
    assert_eq!(*button, wire::click::KeyboardButton::Enter);
    assert_eq!(
        *bounds,
        gpui_kit::Bounds::new(gpui_kit::point(px(0.), px(200.)), size(px(200.), px(40.)))
    );
    tree.read_with(&native, |tree, _| {
        assert!(
            tree.take_activation().is_some(),
            "the reader's press is a gesture, as a pointer's is"
        );
    });
}
