//! Regression tests for the AX door's actions on view controls.

use super::super::tree::look;
use super::*;
use crate::render::ViewTree;
use ducktape_a11y_platform::headless::Harness;
use gpui_kit::{px, size};
use view_wire as wire;

mod dispatch;

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

fn act<V: 'static>(harness: &Harness<V>, id: &str, action: &str, value: &str) -> Reply {
    let request = Request::Act(Act {
        id: id.to_owned(),
        action: action.to_owned(),
        value: Some(value.to_owned()),
        deadline_ms: None,
    });
    crate::ax::ask(harness, harness.root.into(), "t", request, &|_, _| None)
}

fn ax_action_reaches_the_views_field(action: &str) {
    let harness = Harness::open_with(gpui_kit::init, size(px(400.), px(300.)), |_, _| {
        ViewTree::new(chat_shaped_field())
    });
    let tree = harness.root();
    let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = heard.clone();
    let _subscription = harness.app.update(|cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::Text { handler, change } = event {
                seen.borrow_mut().push((*handler, change.text.clone()));
            }
        })
    });
    look(&harness, "t", false);
    let acted = act(&harness, "t:draft-general/editor", action, "hello");
    assert_eq!(
        acted.status, 200,
        "the composer's AX field must be in the tree: {}",
        acted.body
    );
    harness.run_until_parked();
    assert_eq!(
        heard.borrow().last(),
        Some(&(1, "hello".to_owned())),
        "AX `{action}` on the composer's field must reach the view as its change"
    );
}

#[test]
fn ax_type_on_a_nested_editor_reaches_the_views_document() {
    ax_action_reaches_the_views_field("type");
}

#[test]
fn ax_set_value_on_a_nested_editor_reaches_the_views_document() {
    ax_action_reaches_the_views_field("set_value");
}

#[test]
fn an_action_a_view_advertises_reaches_the_view() {
    use gpui_kit::accesskit::Action as A;
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
    let harness = Harness::open_with(gpui_kit::init, size(px(200.), px(120.)), |_, _| {
        ViewTree::new(root)
    });
    let tree = harness.root();
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = harness.app.update(|cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            if let wire::Event::A11yAction { handler, data } = event {
                seen.borrow_mut().push((*handler, data.clone()));
            }
        })
    });
    let node = look(&harness, "t", false)
        .into_iter()
        .find(|node| node.role == "SpinButton")
        .expect("the stepper is in the tree");
    assert_eq!(node.actions, ["increment", "decrement"]);
    for word in ["increment", "decrement"] {
        assert_eq!(act(&harness, &node.id, word, "").status, 200, "{word}");
    }
    harness.act(ActionRequest {
        action: A::CustomAction,
        target_tree: TreeId::ROOT,
        target_node: node.node,
        data: Some(ActionData::CustomAction(4)),
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

#[test]
fn a_press_on_a_view_row_scrolled_out_of_its_list_is_that_rows_click() {
    use gpui_kit::Styled as _;
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
    let harness = Harness::open_with(gpui_kit::init, size(px(300.), px(300.)), |_, _| {
        ViewTree::new(root)
    });
    let tree = harness.root();
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    let _subscription = harness.app.update(|cx| {
        cx.subscribe(&tree, move |_, event: &wire::Event, _| {
            seen.borrow_mut().push(event.clone());
        })
    });
    let nodes = look(&harness, "t", true);
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
    assert_eq!(act(&harness, &row.id, "press", "").status, 200);
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
    harness.app.update(|cx| {
        assert!(
            tree.read(cx).take_activation().is_some(),
            "the reader's press is a gesture, as a pointer's is"
        );
    });
}
