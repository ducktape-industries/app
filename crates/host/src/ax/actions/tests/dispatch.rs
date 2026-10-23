use super::*;
use std::cell::RefCell;

#[test]
fn an_unqueued_action_is_refused_without_typing() {
    for action in ["focus", "set_value", "type"] {
        let harness = Harness::open_with(gpui_kit::init, size(px(400.), px(300.)), |_, _| {
            ViewTree::new(chat_shaped_field())
        });
        let node = look(&harness, "t", false)
            .into_iter()
            .find(|node| node.id == "t:draft-general/editor")
            .expect("the field is in the tree");
        assert!(node.actions.contains(&action), "{action}");
        let keys = Rc::new(RefCell::new(Vec::new()));
        let heard = keys.clone();
        let _subscription = harness.app.update(|cx| {
            cx.observe_keystrokes(move |event, _, _| {
                heard.borrow_mut().push(event.keystroke.key.clone());
            })
        });
        let window: AnyWindowHandle = harness.root.into();
        let key = crate::runtime::WindowKey::unique();
        let readable = RefCell::new(Some(harness.a11y.clone()));
        let disconnected = Rc::new(WindowA11y::default());
        let reply = harness.run(async move |cx| {
            let windows = move |_: &App| vec![("t".to_owned(), key, window)];
            let reach = move |_: AnyWindowHandle| {
                Some(
                    readable
                        .borrow_mut()
                        .take()
                        .unwrap_or_else(|| disconnected.clone()),
                )
            };
            let door = crate::ax::Door {
                windows: &windows,
                settle: &|_| true,
                keep: &|_, _| None,
                chords: &Vec::new,
                views: &|_| Vec::new(),
                reach: &reach,
                stopped: &|_| Vec::new(),
            };
            let request = Request::Act(Act {
                id: node.id,
                action: action.to_owned(),
                value: Some("not typed".to_owned()),
                deadline_ms: None,
            });
            crate::ax::answer(request, &door, &mut Seen::default(), cx).await
        });
        assert_eq!(reply.status, 409, "{action}: {}", reply.body);
        harness.run_until_parked();
        assert!(keys.borrow().is_empty(), "{action} sent keys after refusal");
    }
}
