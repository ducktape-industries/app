//! What the door offers a node, and that `/act` performs each offer.
use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AccessibleAction, Context, InteractiveElement as _, IntoElement, Render,
    StatefulInteractiveElement as _, Styled as _, VisualTestContext, div, px, size,
};

/// A node that advertises an action `/act` has no arm for.
struct Scroller;

impl Render for Scroller {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("scroller")
            .size(px(100.))
            .role(Role::ScrollView)
            .aria_label("Rows")
            .on_a11y_action(AccessibleAction::ScrollDown, |_, _, _| {})
    }
}

/// The door offers only what `/act` performs: a node advertising
/// ScrollDown is offered nothing for it.
#[gpui_kit::test]
fn the_door_offers_no_action_it_cannot_perform(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(200.), px(200.)), |_, _| Scroller);
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let actions = native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        snapshot("t", window, false)
            .into_iter()
            .find(|node| node.role == "ScrollView")
            .expect("the scroller is in the tree")
            .actions
    });
    assert!(actions.is_empty(), "{actions:?}");
}

/// Every action the door has a word for beyond press, focus and
/// set_value, and what each performed.
const MORE: [(AccessibleAction, &str); 6] = [
    (AccessibleAction::Increment, "increment"),
    (AccessibleAction::Decrement, "decrement"),
    (AccessibleAction::Expand, "expand"),
    (AccessibleAction::Collapse, "collapse"),
    (AccessibleAction::ShowContextMenu, "context_menu"),
    (AccessibleAction::ScrollIntoView, "scroll_into_view"),
];

/// A node that handles every action in [`MORE`], writing down each one.
struct Handles(std::rc::Rc<std::cell::RefCell<Vec<AccessibleAction>>>);

impl Render for Handles {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        MORE.iter().fold(
            div()
                .id("stepper")
                .size(px(100.))
                .role(Role::SpinButton)
                .aria_label("Count"),
            |element, (action, _)| {
                let done = self.0.clone();
                let action = *action;
                element.on_a11y_action(action, move |_, _, _| done.borrow_mut().push(action))
            },
        )
    }
}

/// A node that handles increment, decrement, expand, collapse, a context
/// menu or scrolling into view is offered each, only while it does, and
/// `/act` performs each on it (AX-116).
#[gpui_kit::test]
fn the_door_offers_and_performs_each_action_a_node_handles(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let done = std::rc::Rc::default();
    let seen = std::rc::Rc::clone(&done);
    let window = cx.open_window(size(px(200.), px(200.)), move |_, _| Handles(seen));
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
        window.render_frame(cx);
        let node = snapshot("t", window, false)
            .into_iter()
            .find(|node| node.role == "SpinButton")
            .expect("the stepper is in the tree");
        let words: Vec<_> = MORE.iter().map(|(_, word)| *word).collect();
        assert_eq!(node.actions, words);
        for word in &words {
            assert!(super::super::actions::perform_by_id(
                "t", window, cx, &node.id, word, ""
            ));
            window.render_frame(cx);
        }
    });
    let performed: Vec<_> = MORE.iter().map(|(action, _)| *action).collect();
    assert_eq!(*done.borrow(), performed);
}
