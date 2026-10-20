//! The Overlay node's layer: a press outside what it shows is its own.
use super::*;

const DISMISS: u32 = 7;
const ROW: u32 = 42;
const ITEM: u32 = 43;

fn clicking(key: &str, name: &str, handler: u32) -> wire::Node {
    let mut node = button(key, name);
    if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut node {
        interactivity.get_or_insert_default().on_click = Some(handler);
    }
    node
}

fn page(open: bool) -> wire::Node {
    let mut children = vec![container("base", [clicking("row", "Message", ROW)])];
    if open {
        let mut menu = container("menu", [clicking("item", "Copy", ITEM)]);
        if let wire::Node::Container(view_wire::ContainerNode { interactivity, .. }) = &mut menu {
            interactivity.get_or_insert_default().occlude = true;
        }
        children.push(wire::Node::Deferred {
            priority: 1,
            content: Box::new(wire::Node::Anchored {
                anchor: wire::Anchor::TopLeft,
                fit: wire::AnchoredFitMode::SnapToWindow,
                position: Some([200., 150.]),
                position_mode: wire::AnchoredPositionMode::Window,
                offset: None,
                children: vec![menu],
            }),
        });
    }
    let overlay = wire::Node::Overlay {
        id: named_id("actions"),
        label: None,
        on_dismiss: Some(DISMISS),
        children,
        style: crate::render::test_style(div().size_full().style().clone()),
    };
    sized("root", overlay, Some(fill()), Some(fill()))
}

fn press(native: &mut gpui_kit::VisualTestContext, x: f32, y: f32) {
    let at = point(px(x), px(y));
    native.simulate_mouse_move(at, None, Default::default());
    native.simulate_mouse_down(at, MouseButton::Left, Default::default());
    native.simulate_mouse_up(at, MouseButton::Left, Default::default());
}

fn heard(events: &RefCell<Vec<wire::Event>>) -> Vec<String> {
    events
        .borrow_mut()
        .drain(..)
        .filter_map(|event| match event {
            wire::Event::Click { handler, .. } => Some(format!("click {handler}")),
            wire::Event::Message(DISMISS) => Some("dismiss".into()),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn a_press_outside_a_menu_dismisses_it_and_reaches_nothing_under_it(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(size(px(400.), px(300.)), |_, _| ViewTree::new(page(false)));
    let tree = window.root(cx).unwrap();
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (events, _subscription) = emitted(&tree, &mut native);
    native.update(crate::render::draw_frame);
    press(&mut native, 20., 12.);
    assert_eq!(
        heard(&events),
        [format!("click {ROW}")],
        "shut, the press clicks the row"
    );

    tree.update(&mut native, |tree, cx| tree.replace(page(true), &[], cx));
    native.update(crate::render::draw_frame);
    press(&mut native, 20., 12.);
    assert_eq!(heard(&events), ["dismiss"], "open, it only closes the menu");
    press(&mut native, 210., 160.);
    assert_eq!(heard(&events), [format!("click {ITEM}")], "the item's own");
}
