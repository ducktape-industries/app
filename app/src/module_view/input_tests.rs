//! Actual staged Chat and Pages views in native GPUI windows.
use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{self as gpui, Entity, TestAppContext, VisualTestContext};

struct NativeDragHarness {
    guest: Entity<NativeModuleView>,
}

impl gpui::Render for NativeDragHarness {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{
            InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _,
            Styled as _,
        };
        use gpui_kit::TestSupportExt as _;

        gpui::div()
            .id("native-drag-harness")
            .size_full()
            .p(gpui::px(32.))
            .relative()
            .flex()
            .flex_col()
            .child(
                gpui::div()
                    .id("native-guest-slot")
                    // a node with bounds the test door can resolve an id to
                    .role(gpui::Role::Group)
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.guest.clone()),
            )
            .child(
                gpui::div()
                    .id("native-sibling")
                    .test_support()
                    .absolute()
                    .top(gpui::px(0.))
                    .left(gpui::px(0.))
                    .w_full()
                    .h(gpui::px(40.))
                    .debug_selector(|| "native-sibling".to_owned())
                    .capture_any_mouse_down(|_, _, cx| cx.stop_propagation()),
            )
    }
}

fn seated(opened: &[&str]) -> Arc<Mutex<Mounted>> {
    tests::can_the_chat_room();
    tests::can_reads([(
        "channels",
        serde_json::json!({
            "channels": {
                "channels": [{
                    "id": "channel-a", "name": "general", "created_at": 1,
                    "post_policy": "open", "owner": "acct:7", "archived": false,
                    "hooks": [], "huddle": [], "head_seq": 1
                }],
                "has_more": false, "next_after": null
            }
        }),
    )]);
    let path = tests::staged("chat").expect("build current chat view first");
    let mut guest = Guest::load_from("chat", &path).expect("build current chat view first");
    let props = tests::chat_facts();
    guest.redraw(&None);
    settle(&mut guest, &props);
    for label in opened {
        guest
            .pending
            .push(wire::Event::Message(tests::button_message(&guest, label)));
        settle(&mut guest, &props);
    }
    assert!(guest.fault.is_none());
    let seat = Arc::new(Mutex::new(Mounted {
        changes: tokio::sync::watch::channel(()).0,
        slot: Slot::Ready(Box::new(guest)),
        props,
        generation: 1,
        hash: None,
        in_flight: false,
        wanted: None,
        tasting: None,
        waiting_since: None,
        replacement: Replacement::Preserve,
        retry: None,
        shown: None,
    }));
    registry().lock().unwrap().insert("chat", seat.clone());
    seat
}
fn settle(guest: &mut Guest, props: &Option<Vec<u8>>) {
    for _ in 0..32 {
        if !guest.redraw(props) {
            return;
        }
    }
    panic!("view did not settle: {:?}", guest.fault);
}
fn open(cx: &mut TestAppContext) -> (Entity<NativeModuleView>, VisualTestContext) {
    cx.update(gpui_kit::init);
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |_, _| {
        NativeModuleView::new("chat")
    });
    let view = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    (view, native)
}

fn open_console(
    cx: &mut TestAppContext,
    module: &'static str,
) -> (Entity<crate::shell::DesktopWindow>, VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.update(crate::editor::wire::init_notion);
    let mut state = crate::Ducktape::initial_state();
    state.connected = true;
    state.connected_rpc = "http://127.0.0.1:1".into();
    state.network_name = "testnet".into();
    state.network_chain_id = "testnet#abcd".into();
    state.status = "Live".into();
    state.block_height = 84_912;
    state.account_number = "7".into();
    state.account_exists = true;
    state.account_name = "mallard".into();
    state.settings_user_key = "aa".into();
    state.active_channel = "channel-a".into();
    state.active_channel_name = "general".into();
    state.shell_tab = crate::ShellTab::View(module);
    let mut presenter = None;
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |window, cx| {
        let view = crate::shell::test_window(state, crate::shell::WindowKind::Console, window, cx);
        presenter = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    let presenter = presenter.unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    native.update(|window, cx| window.render_frame(cx));
    (presenter, native)
}

fn console_module(
    presenter: &Entity<crate::shell::DesktopWindow>,
    native: &VisualTestContext,
) -> Entity<NativeModuleView> {
    presenter
        .read_with(native, |presenter, _| presenter.test_module())
        .expect("console module")
}

fn assert_chat_fixture_geometry(
    view: &Entity<NativeModuleView>,
    native: &VisualTestContext,
    nodes: &[crate::ax_door::AxNode],
) {
    let bounds = view.read_with(native, |view, cx| {
        view.content.as_ref().map(|content| {
            [
                "ChatView/@sensor:906",
                "ChatView/chat/press-area",
                "ChatView",
                "ChatView/chat",
                "ChatView/chat/room",
                "ChatView/chat/message-stream",
                "ChatView/chat/composer-room",
            ]
            .map(|key| (key, content.read(cx).measured_bounds(key)))
        })
    });
    assert!(
        bounds
            .as_ref()
            .and_then(|bounds| {
                bounds
                    .iter()
                    .find(|(key, _)| *key == "ChatView/chat/message-stream")
                    .and_then(|(_, bounds)| *bounds)
            })
            .is_some_and(|bounds| {
                bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.)
            }),
        "Chat message-stream needs a positive production-sized viewport: {bounds:?}"
    );
    assert!(
        nodes.iter().any(|node| {
            node.name.contains("first light")
                || node
                    .value
                    .as_deref()
                    .is_some_and(|value| value.contains("first light"))
                || node
                    .description
                    .as_deref()
                    .is_some_and(|description| description.contains("first light"))
        }),
        "the initial committed message row is missing from AX: {nodes:?}"
    );
    assert!(
        nodes.iter().any(|node| {
            node.name == "mallard"
                || node.value.as_deref() == Some("mallard")
                || node.description.as_deref() == Some("mallard")
        }),
        "the initial message author is missing from AX: {nodes:?}"
    );
}

fn door_tree_for(native: &mut VisualTestContext, scope: &str) -> Vec<crate::ax_door::AxNode> {
    native.update(|window, cx| {
        window.activate_a11y();
        let mut seen = crate::ax_door::Seen::default();
        crate::ax_door::current(scope, window, cx, false, &mut seen)
    })
}

fn door_tree(native: &mut VisualTestContext) -> Vec<crate::ax_door::AxNode> {
    door_tree_for(native, "chat")
}

fn composer_is_cleared(nodes: &[crate::ax_door::AxNode]) -> bool {
    nodes
        .iter()
        .any(|node| node.name.starts_with("Message #") && node.value.as_deref() == Some(""))
}

fn send_is_disabled(nodes: &[crate::ax_door::AxNode]) -> bool {
    nodes
        .iter()
        .any(|node| node.name == "Send" && node.state.contains(&"disabled"))
}

fn wake_chat_live(seat: &Arc<Mutex<Mounted>>) {
    let mut locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &mut locked.slot else {
        panic!("seated view");
    };
    let live_ids: Vec<_> = guest
        .live_subscriptions
        .iter()
        .filter(|(_, plane)| plane == "chat")
        .map(|(id, _)| *id)
        .collect();
    assert!(
        !live_ids.is_empty(),
        "Chat fixture has no chat live subscription"
    );
    for id in live_ids {
        guest.pending.push(wire::Event::Response {
            id,
            result: Ok(b"{}".to_vec()),
            done: false,
        });
    }
}

#[gpui_kit::test]
fn door_observation_advances_chat_after_an_async_host_answer(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    tests::can_bytes("host.id", b"message-committed".to_vec());
    tests::can_reads([("op.submit", serde_json::json!(1))]);
    let seat = seated(&[]);
    let (presenter, mut native) = open_console(cx, "chat");
    let view = console_module(&presenter, &native);
    settle_native_documents(&mut native, &seat);
    let initial = door_tree(&mut native);
    assert_chat_fixture_geometry(&view, &native, &initial);
    let field = initial
        .iter()
        .find(|node| node.name.starts_with("Message #") && node.actions.contains(&"type"))
        .unwrap_or_else(|| panic!("Chat composer input: {initial:?}"))
        .id
        .clone();
    native.update(|window, cx| {
        assert!(crate::ax_door::perform_by_id(
            "chat",
            window,
            cx,
            &field,
            "type",
            "door regression"
        ));
    });
    native.run_until_parked();
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    let ready = door_tree(&mut native);
    let send = ready
        .iter()
        .find(|node| node.name == "Send" && node.actions.contains(&"press"))
        .unwrap_or_else(|| panic!("enabled Chat Send: {ready:?}"))
        .id
        .clone();
    native.update(|window, cx| {
        assert!(crate::ax_door::perform_by_id(
            "chat", window, cx, &send, "press", ""
        ));
    });
    let mut committed = tests::chat_row_of(2, "door regression", None);
    committed["message_id"] = serde_json::json!("message-committed");
    tests::can_reads([
        (
            "channel",
            serde_json::json!({ "channel": {
                "id": "channel-a", "name": "general", "created_at": 1,
                "post_policy": "open", "owner": "acct:7", "archived": false,
                "hooks": [], "huddle": [], "head_seq": 2
            }}),
        ),
        (
            "roots",
            serde_json::json!({
                "roots": {
                    "roots": [
                        tests::chat_row(1),
                        committed,
                    ],
                    "has_more": false
                }
            }),
        ),
    ]);
    native.update(|window, cx| {
        assert!(window.simulate_next_frame(cx) > 0, "Send requested a frame");
    });
    native.run_until_parked();
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    native.run_until_parked();
    wake_chat_live(&seat);
    native.update(|window, cx| {
        window.render_frame(cx);
    });
    settle_native_documents(&mut native, &seat);
    let observed = door_tree(&mut native);
    assert!(
        composer_is_cleared(&observed),
        "accepted Send did not clear the composer"
    );
    assert!(
        send_is_disabled(&observed),
        "accepted Send did not disable the control"
    );
    assert!(
        observed.iter().any(|node| {
            node.name.contains("door regression")
                || node.value.as_deref() == Some("door regression")
                || node.description.as_deref() == Some("door regression")
        }),
        "door missed the committed message text: {observed:?}"
    );
    assert!(
        observed.iter().any(|node| {
            node.name.contains("mallard")
                || node.value.as_deref() == Some("mallard")
                || node.description.as_deref() == Some("mallard")
        }),
        "door missed the committed message author: {observed:?}"
    );
}

fn open_drag_harness(cx: &mut TestAppContext) -> (Entity<NativeModuleView>, VisualTestContext) {
    cx.update(gpui_kit::init);
    let view = cx.new(|_| NativeModuleView::new("chat"));
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |_, _| {
        NativeDragHarness {
            guest: view.clone(),
        }
    });
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    (view, native)
}
fn button(seat: &Arc<Mutex<Mounted>>, label: &str) -> String {
    fn shows(node: &wire::Node, label: &str) -> bool {
        matches!(node, wire::Node::Text { content, .. } if content == label)
            || node.children().iter().any(|child| shows(child, label))
    }
    let locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &locked.slot else {
        panic!("live guest")
    };
    let mut root = guest.frame.root.clone().unwrap();
    let mut visible_label = None;
    root.for_each_mut(&mut |node| {
        if let wire::Node::Button {
            key,
            content: wire::ButtonContent::Child(child),
            on_press: Some(_),
            ..
        } = node
            && shows(child, label)
        {
            visible_label = Some(key.clone());
        }
    });
    visible_label.unwrap_or_else(|| tests::button_key(guest, label))
}
fn click_before_frame(native: &mut VisualTestContext, key: String) {
    native.update(|window, cx| {
        let position = window.find(key).bounds().center();
        window.dispatch_event(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position,
                button: gpui::MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position,
                button: gpui::MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }),
            cx,
        );
    });
}
#[test]
fn layer_minimize_hides_and_restore_shows_the_retained_guest() {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&[]);
    let mut cx = crate::frame_probe::headless_context();
    let mut state = crate::Ducktape::initial_state();
    state.shell_tab = crate::ShellTab::View("chat");
    let mut presenter = None;
    let window = cx
        .open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |window, cx| {
            let view =
                crate::shell::test_window(state, crate::shell::WindowKind::Console, window, cx);
            presenter = Some(view.clone());
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .unwrap();
    let presenter = presenter.unwrap();
    let visible = || {
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            panic!("live guest");
        };
        guest.visible
    };
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(visible());
    presenter.update(&mut cx, |view, cx| {
        view.test_dispatch(
            crate::AppMessage::SelectShellTab(crate::ShellTab::View("files")),
            cx,
        )
    });
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    // OS mode: another tab opens another layer; the first stays on screen.
    assert!(visible(), "a second layer leaves the first visible");
    presenter.update(&mut cx, |view, cx| {
        view.layer_action("chat", crate::shell::LayerAction::Minimize, cx)
    });
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(
        !visible(),
        "a minimized layer's guest hears host.visible=false"
    );
    presenter.update(&mut cx, |view, cx| view.focus_workspace_layer("chat", cx));
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(visible(), "restoring the layer shows the same guest again");
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
    drop(presenter);
    cx.run_until_parked();
    assert!(!visible(), "closing retires the tab presentation");
}

#[gpui_kit::test]
fn native_presenter_reports_hidden_and_visible_lifecycle(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&[]);
    let (view, mut native) = open(cx);
    let visible = || {
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            panic!("live guest");
        };
        guest.visible
    };
    assert!(visible());
    view.update(&mut native, |view, _| {
        let _ = view.hide();
        assert!(!visible(), "hidden before the presenter leaves");
    });
    native.update(|window, cx| window.render_frame(cx));
    assert!(visible());
}

#[gpui_kit::test]
fn chat_native_overlays_are_visible_and_route_menu_and_emoji_presses(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    for (opened, label, reacted) in [
        (&["More message actions"][..], "Add reaction", false),
        (&["Manage reactions"][..], "🦆", true),
    ] {
        let seat = seated(&[]);
        let (view, mut native) = open(cx);
        for label in opened {
            let key = button(&seat, label);
            // The message actions float over the card while the pointer is on it.
            let hover = format!("{}/hover", key.rsplit_once('/').expect("scoped key").0);
            native.update(|window, cx| window.hover(hover, cx));
            native.update(|window, cx| window.render_frame(cx));
            click_before_frame(&mut native, key);
            native.update(|window, cx| window.render_frame(cx));
        }
        let focus = if reacted { "reaction" } else { "action" };
        native.update(|window, cx| {
            let content = view.read(cx).content.clone().unwrap();
            content.update(cx, |tree, cx| {
                let reply = tree
                    .execute_widget_command(
                        wire::WidgetCommand::Focused {
                            target: format!("ChatView/chat/message-{focus}-focus"),
                        },
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(
                    wire::decode::<bool>(&reply).unwrap(),
                    "guest {focus} menu requests real native focus; queued commands: {:?}",
                    match &seat.lock().unwrap().slot {
                        Slot::Ready(guest) => guest.widget_commands.clone(),
                        _ => Vec::new(),
                    }
                );
            });
        });
        let key = button(&seat, label);
        native.update(|window, _| {
            assert!(
                window.find(key.clone()).visible(),
                "native popup {label:?} at {key:?} is visible: {:?}",
                window.find(key.clone()).bounds()
            )
        });
        {
            let mut locked = seat.lock().unwrap();
            let Slot::Ready(guest) = &mut locked.slot else {
                unreachable!()
            };
            guest.frame.mouse_interest = true;
        }
        input::record_inputs();
        click_before_frame(&mut native, key);
        let delivered = input::recorded_inputs();
        {
            // GPUI flushes dirty test windows before update returns. Observe
            // admitted events, not a queue that the real guest already drained.
            let routed = delivered
                .iter()
                .position(|event| matches!(event, wire::Event::Message(_)))
                .unwrap_or_else(|| panic!("popup {label:?} routes: {delivered:?}"));
            let observed = delivered
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        wire::Event::Mouse {
                            event: wire::mouse::Event::ButtonReleased(wire::mouse::Button::Left),
                            captured: true
                        }
                    )
                })
                .unwrap_or_else(|| panic!("captured release observed: {delivered:?}"));
            assert!(routed < observed, "widget output precedes its observation");
        }
        native.update(|window, cx| window.render_frame(cx));
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            unreachable!()
        };
        let texts = tests::texts(guest);
        if reacted {
            assert!(texts.windows(2).any(|pair| pair == ["🦆", "1"]));
        } else {
            assert!(texts.iter().any(|text| text == "🦆"));
        }
    }
}
#[gpui_kit::test]
fn candidate_preparation_and_rejection_keep_the_seated_native_input(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&[]);
    let (view, mut native) = open(cx);
    let input = || {
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            panic!("seated Chat")
        };
        let mut found = None;
        guest.frame.root.clone().unwrap().for_each_mut(&mut |node| {
            if let wire::Node::Input {
                key,
                value,
                options,
                ..
            } = node
                && options.label == "Search messages"
            {
                found = Some((key.clone(), value.clone()));
            }
        });
        found.expect("Chat search input")
    };
    let key = input().0;
    native.update(|window, cx| window.click(key.clone(), cx));
    let content = view.read_with(&native, |view, _| view.content.clone().unwrap());
    let attempt = seat.lock().unwrap().start(Some([7; 32]));

    // A real OS key may arrive before the first repaint after a deployment
    // check. TestWindowExt::input paints first, which would hide that race.
    native.update(|window, cx| {
        let mut key = gpui::Keystroke::parse("x").unwrap();
        key.key_char = Some("x".into());
        window.dispatch_keystroke(key, cx);
    });
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        input().1,
        "x",
        "candidate preparation lost the accepted key"
    );
    assert_eq!(
        view.read_with(&native, |view, _| view
            .content
            .as_ref()
            .unwrap()
            .entity_id()),
        content.entity_id(),
        "an uninstalled candidate must not replace native controls"
    );
    {
        let mut locked = seat.lock().unwrap();
        assert_eq!(locked.generation, attempt);
        locked.in_flight = false;
        locked.retry = Some(Retry::after(None, Some([7; 32])));
    }
    native.update(|window, cx| {
        let mut key = gpui::Keystroke::parse("y").unwrap();
        key.key_char = Some("y".into());
        window.dispatch_keystroke(key, cx);
    });
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        input().1,
        "xy",
        "rejected candidate disabled the seated input"
    );
    assert_eq!(
        view.read_with(&native, |view, _| view
            .content
            .as_ref()
            .unwrap()
            .entity_id()),
        content.entity_id()
    );
}

#[gpui_kit::test]
fn a_retained_overlay_cannot_send_a_press_to_a_replacement_instance(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&["More message actions"]);
    let (view, mut native) = open(cx);
    let content = view.read_with(&native, |view, _| view.content.clone().unwrap());
    let message = {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        let message = tests::button_message(guest, "Manage reactions");
        guest.pending.clear();
        guest.alive = Arc::new(());
        message
    };
    // The old native entity's real subscription remains live until the next frame.
    content.update(&mut native, |_, cx| cx.emit(wire::Event::Message(message)));
    let locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &locked.slot else {
        unreachable!()
    };
    assert!(
        guest.pending.is_empty(),
        "retired overlay routed into its replacement"
    );
}
#[gpui_kit::test]
fn a_retained_control_cannot_address_a_new_frames_handler_table(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&["More message actions"]);
    let (view, mut native) = open(cx);
    let content = view.read_with(&native, |view, _| view.content.clone().unwrap());
    let message = {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        let message = tests::button_message(guest, "Manage reactions");
        guest.pending.clear();
        guest.frame_rev += 1;
        message
    };
    content.update(&mut native, |_, cx| cx.emit(wire::Event::Message(message)));
    let locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &locked.slot else {
        unreachable!()
    };
    assert!(
        guest.pending.is_empty(),
        "old frame's handler index reached a new table"
    );
}
fn thread_width(seat: &Arc<Mutex<Mounted>>) -> f32 {
    let locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &locked.slot else {
        unreachable!()
    };
    let mut root = guest.frame.root.clone().unwrap();
    let mut width = None;
    root.for_each_mut(&mut |node| {
        if let wire::Node::Container {
            key,
            width: Some(wire::Length::Fixed(value)),
            ..
        }
        | wire::Node::Linear {
            key,
            width: Some(wire::Length::Fixed(value)),
            ..
        } = node
            && key.ends_with("/thread-pane")
        {
            width = Some(*value);
        }
    });
    width.expect("thread pane")
}
#[gpui_kit::test]
fn a_native_pointer_drag_resizes_the_thread_and_release_ends_it(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&["Open thread"]);
    let (_, mut native) = open(cx);
    let key = {
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            unreachable!()
        };
        let mut root = guest.frame.root.clone().unwrap();
        let mut key = None;
        root.for_each_mut(&mut |node| {
            if node
                .key()
                .is_some_and(|key| key.ends_with("/thread-resize"))
            {
                key = node.key().map(str::to_owned);
            }
        });
        key.unwrap()
    };
    let bounds = native.update(|window, _| window.find(key.clone()).bounds());
    assert!(
        bounds.size.width >= gpui::px(10.) && bounds.size.height > gpui::px(100.),
        "native divider fills its pane: {bounds:?}"
    );
    assert_eq!(thread_width(&seat), 330.);
    let start = bounds.center();
    let end = start - gpui::point(gpui::px(100.), gpui::px(0.));
    native.update(|window, cx| window.drag(start, end, cx));
    assert_eq!(thread_width(&seat), 430.);
    native.simulate_mouse_move(start, None, Default::default());
    native.update(|window, cx| window.render_frame(cx));
    assert_eq!(thread_width(&seat), 430., "release ends the grab");
}
/// A door drag with the guest slot's id: local coordinates are offset by
/// the slot's painted origin before they reach the window, so the seated
/// guest sees them back as the same local coordinates.
#[gpui_kit::test]
fn door_drag_by_node_id_reaches_the_guest_in_its_own_coordinates(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&[]);
    let (_, mut native) = open_drag_harness(cx);
    native.update(|window, cx| {
        window.activate_a11y();
        window.render_frame(cx);
    });
    let slot = native.update(|window, _| {
        crate::ax_door::snapshot("harness", window, true)
            .into_iter()
            .find(|node| node.id == "harness:native-guest-slot")
            .expect("the guest slot is a door node")
    });
    let [x, y, ..] = slot.bounds.expect("painted bounds");
    assert!(
        x > 0 && y > 0,
        "the guest is painted at a non-zero origin: {slot:?}"
    );
    let origin = gpui::point(gpui::px(x as f32), gpui::px(y as f32));

    // Prime the host's pointer-inside state while interest is off, so the
    // drag sequence starts with its press instead of a CursorEntered event.
    native.simulate_mouse_move(
        origin + gpui::point(gpui::px(100.), gpui::px(100.)),
        None,
        Default::default(),
    );
    {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        guest.frame.mouse_interest = true;
    }
    input::record_inputs();
    let drag = serde_json::from_value::<crate::ax_door::Drag>(serde_json::json!({
        "id": "harness:native-guest-slot",
        "from": [100, 100],
        "to": [145, 160],
        "steps": 3,
    }))
    .unwrap();
    let reply =
        native.update(|window, cx| crate::ax_door::drag_by_id("harness", window, cx, &drag));
    assert_eq!(
        input::recorded_inputs(),
        vec![
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonPressed(wire::mouse::Button::Left),
                captured: false,
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::CursorMoved { x: 145., y: 160. },
                captured: false,
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonReleased(wire::mouse::Button::Left),
                captured: false,
            },
        ],
        "the guest reads the drag back in the local coordinates the door was asked for"
    );
    assert_eq!(reply.status(), 200, "{}", reply.body());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(reply.body()).unwrap(),
        serde_json::json!({
            "from": [100. + x as f32, 100. + y as f32],
            "to": [145. + x as f32, 160. + y as f32],
            "steps": 3,
        }),
        "the window coordinates actually sent"
    );
}

#[gpui_kit::test]
fn native_pointer_drag_delivers_the_host_contract_through_window_listeners(
    cx: &mut TestAppContext,
) {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&[]);
    let (_, mut native) = open_drag_harness(cx);
    let bounds = gpui::Bounds {
        origin: gpui::point(gpui::px(32.), gpui::px(32.)),
        size: gpui::size(gpui::px(1136.), gpui::px(736.)),
    };
    assert!(
        bounds.origin.x > gpui::px(0.) && bounds.origin.y > gpui::px(0.),
        "the guest must be painted at a non-zero origin: {bounds:?}"
    );

    let local_start = gpui::point(gpui::px(100.), gpui::px(100.));
    let start = bounds.origin + local_start;
    let local_moves = [
        gpui::point(gpui::px(110.), gpui::px(120.)),
        gpui::point(gpui::px(125.), gpui::px(140.)),
        gpui::point(gpui::px(145.), gpui::px(160.)),
    ];
    let moves: Vec<_> = local_moves
        .iter()
        .map(|local| bounds.origin + *local)
        .collect();
    // Prime the host's pointer-inside state while interest is off, so the
    // drag sequence starts with its press instead of a CursorEntered event.
    native.simulate_mouse_move(start, None, Default::default());
    {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        guest.frame.mouse_interest = true;
    }
    input::record_inputs();
    native.update(|window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position: start,
                button: gpui::MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        for position in moves.iter().copied() {
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position,
                    pressed_button: Some(gpui::MouseButton::Left),
                    modifiers: Default::default(),
                }),
                cx,
            );
        }
        window.dispatch_event(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position: *moves.last().unwrap(),
                button: gpui::MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }),
            cx,
        );
    });
    let delivered = input::recorded_inputs();
    assert_eq!(
        delivered,
        vec![
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonPressed(wire::mouse::Button::Left),
                captured: false,
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::CursorMoved { x: 145., y: 160. },
                captured: false,
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonReleased(wire::mouse::Button::Left),
                captured: false,
            },
        ],
        "press/release survive while undrained moves coalesce to the newest local position"
    );

    // The guest's frame drains the first drag. Chat does not opt in itself,
    // so restore the per-frame interest bit before the next native sequence.
    native.update(|window, cx| window.render_frame(cx));
    {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        guest.frame.mouse_interest = true;
    }
    let outside = gpui::point(
        bounds.origin.x - gpui::px(12.),
        bounds.origin.y + bounds.size.height + gpui::px(12.),
    );
    assert!(outside.x > gpui::px(0.) && outside.y < gpui::px(800.));
    input::record_inputs();
    native.update(|window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position: start,
                button: gpui::MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: outside,
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position: outside,
                button: gpui::MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
            }),
            cx,
        );
    });
    assert_eq!(
        input::recorded_inputs(),
        vec![
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonPressed(wire::mouse::Button::Left),
                captured: false,
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::CursorMoved {
                    x: -12.,
                    y: f32::from(bounds.size.height) + 12.,
                },
                captured: false,
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonReleased(wire::mouse::Button::Left),
                captured: false,
            },
        ],
        "window-level listeners keep the drag alive outside the guest bounds"
    );

    native.update(|window, cx| window.render_frame(cx));
    {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        guest.frame.mouse_interest = true;
    }
    input::record_inputs();
    native.update(|window, cx| {
        window.dispatch_event(
            gpui::PlatformInput::MouseExited(gpui::MouseExitEvent {
                position: outside,
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        );
    });
    assert_eq!(
        input::recorded_inputs(),
        vec![wire::Event::Mouse {
            event: wire::mouse::Event::CursorLeft,
            captured: false,
        }],
        "a window exit reports CursorLeft and does not synthesize a release"
    );

    // A native sibling consumes its press before the window listener's bubble
    // phase. `captured: true` therefore describes native control consumption;
    // guest-painted points above remain false.
    let sibling = native.update(|window, _| {
        let bounds = window.find("native-sibling").bounds();
        assert!(bounds.size.height > gpui::px(0.));
        bounds.center()
    });
    {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            unreachable!()
        };
        guest.frame.mouse_interest = true;
    }
    input::record_inputs();
    native.simulate_mouse_down(sibling, gpui::MouseButton::Left, Default::default());
    let sibling_events = input::recorded_inputs();
    assert!(
        sibling_events.iter().any(|event| {
            matches!(
                event,
                wire::Event::Mouse {
                    event: wire::mouse::Event::ButtonPressed(wire::mouse::Button::Left),
                    captured: true,
                }
            )
        }),
        "native sibling events: {sibling_events:?}"
    );
}

#[test]
fn opted_in_mouse_moves_are_local_coalesced_and_keep_button_order() {
    let _turn = tests::blocking_connection_turn();
    let seat = seated(&[]);
    let mut locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &mut locked.slot else {
        unreachable!()
    };
    let movement = |x, y| wire::mouse::Event::CursorMoved { x, y };
    assert!(!input::mouse(guest, movement(5., 5.), false));
    guest.frame.mouse_interest = true;
    input::mouse(guest, movement(5., 5.), false);
    input::mouse(
        guest,
        wire::mouse::Event::ButtonPressed(wire::mouse::Button::Left),
        true,
    );
    input::mouse(guest, movement(35., 35.), false);
    input::mouse(guest, movement(40., 40.), false);
    input::mouse(
        guest,
        wire::mouse::Event::ButtonReleased(wire::mouse::Button::Left),
        true,
    );
    assert_eq!(
        guest.pending,
        vec![
            wire::Event::Mouse {
                event: movement(5., 5.),
                captured: false
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonPressed(wire::mouse::Button::Left),
                captured: true
            },
            wire::Event::Mouse {
                event: movement(40., 40.),
                captured: false
            },
            wire::Event::Mouse {
                event: wire::mouse::Event::ButtonReleased(wire::mouse::Button::Left),
                captured: true
            },
        ]
    );
    assert!(!input::mouse(guest, movement(f32::NAN, 0.), false));
}
#[test]
fn ime_observations_keep_unicode_selection_and_commit_order() {
    use wire::events::{Event as E, InputMethod as I};
    let mut previous = None;
    let events = input::ime_events(&mut previous, "a🦆한", Some(1..4), 8, 5..8);
    assert_eq!(
        events,
        vec![
            wire::Event::Observation {
                event: E::InputMethod(I::Opened),
                captured: true
            },
            wire::Event::Observation {
                event: E::InputMethod(I::Preedit {
                    content: "🦆한".into(),
                    selection: Some((4, 7))
                }),
                captured: true
            },
        ]
    );
    let events = input::ime_events(&mut previous, "a🦆한", None, 8, 8..8);
    assert_eq!(
        events,
        vec![
            wire::Event::Observation {
                event: E::InputMethod(I::Commit("🦆한".into())),
                captured: true
            },
            wire::Event::Observation {
                event: E::InputMethod(I::Closed),
                captured: true
            },
        ]
    );
}

#[gpui_kit::test]
fn pages_wasm_owns_native_menu_and_input_rules(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    tests::can_a_commented_page();
    tests::can_reads([
        ("model", serde_json::json!({"model": {"agents": []}})),
        ("op.submit", serde_json::json!(1)),
    ]);
    let props = tests::pages_facts();
    let path = tests::staged("pages").expect("build current Pages view first");
    let mut guest = Guest::load_from("pages", &path).expect("build current Pages view first");
    tests::settle_documents(&mut guest, &props);
    let seat = Arc::new(Mutex::new(Mounted {
        changes: tokio::sync::watch::channel(()).0,
        slot: Slot::Ready(Box::new(guest)),
        props,
        generation: 1,
        hash: None,
        in_flight: false,
        wanted: None,
        tasting: None,
        waiting_since: None,
        replacement: Replacement::Preserve,
        retry: None,
        shown: None,
    }));
    registry().lock().unwrap().insert("pages", seat.clone());
    cx.update(gpui_kit::init);
    cx.update(crate::editor::wire::init_notion);
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |_, _| {
        NativeModuleView::new("pages")
    });
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    settle_native_documents(&mut native, &seat);
    native.update(|window, cx| {
        window.render_frame(cx);
        window.click(("block", 3usize), cx);
        window.press("end", cx);
        window.input(" ", cx);
    });
    settle_native_documents(&mut native, &seat);
    native.update(|window, cx| window.input("@", cx));
    settle_native_documents(&mut native, &seat);
    let projection = || {
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            panic!("live Pages view");
        };
        assert!(guest.fault.is_none(), "{:?}", guest.fault);
        let mut result = None;
        guest.frame.root.clone().unwrap().for_each_mut(&mut |node| {
            if let wire::Node::Editor {
                options, editable, ..
            } = node
            {
                assert!(
                    *editable,
                    "the fixture remains editable: {:?}",
                    tests::texts(guest)
                );
                result = Some((
                    options.rich.clone().unwrap(),
                    options.presentation.clone().unwrap(),
                ));
            }
        });
        result.expect("Pages document editor")
    };
    let (rich, paint) = projection();
    let menu = paint.affordances.menu.unwrap_or_else(|| {
        panic!(
            "the WASM opens the mention menu; first blocks: {:?}, cursor: {:?}",
            &rich.document.blocks[..3],
            rich.document.cursor
        )
    });
    let row = menu
        .items
        .iter()
        .find(|item| item.label.contains("Ada Lovelace"))
        .map(|item| gpui_kit::SharedString::from(format!("application-suggestion/{}", item.tag)))
        .expect("the WASM supplies the account directory");
    native.update(|window, cx| {
        window.render_frame(cx);
        window.click(row, cx);
    });
    settle_native_documents(&mut native, &seat);
    let (rich, paint) = projection();
    assert!(
        paint.affordances.menu.is_none(),
        "the guest closes the committed menu"
    );
    assert!(
        rich.document
            .blocks
            .iter()
            .any(|block| block.text.ends_with(" @Ada Lovelace ")),
        "the guest replaces the mention in its document: {:?}",
        rich.document
    );
    native.update(|window, cx| {
        assert!(
            window.focused(cx).is_some(),
            "menu selection preserves keyboard focus"
        )
    });
    for (source, kind, text) in [
        ("# Heading", "heading", "Heading"),
        ("**bold** plain", "paragraph", "bold plain"),
        ("(c)", "paragraph", "©"),
    ] {
        native.update(|window, cx| window.press("enter", cx));
        settle_native_documents(&mut native, &seat);
        for character in source.chars() {
            native.update(|window, cx| window.input(&character.to_string(), cx));
            settle_native_documents(&mut native, &seat);
        }
        let (rich, _) = projection();
        let block = &rich.document.blocks[rich.document.cursor.position.line as usize];
        assert_eq!(
            block.kind, kind,
            "guest interpretation of {source:?}: {block:?}"
        );
        assert_eq!(block.text, text, "guest interpretation of {source:?}");
        if source.starts_with("**") {
            assert_eq!(
                block
                    .marks
                    .iter()
                    .filter(|mark| mark.kind == "bold")
                    .map(|mark| (mark.start, mark.end))
                    .collect::<Vec<_>>(),
                vec![(0, 4)],
                "text after the completed delimiter stays plain"
            );
        }
    }
}

fn settle_native_documents(native: &mut VisualTestContext, seat: &Arc<Mutex<Mounted>>) {
    loop {
        native.run_until_parked();
        let ticks = {
            let locked = seat.lock().unwrap();
            let Slot::Ready(guest) = &locked.slot else {
                panic!("seated view");
            };
            assert!(guest.fault.is_none(), "{:?}", guest.fault);
            let pending = guest.frame.busy
                || guest.inputs.pending()
                || !guest.pending.is_empty()
                || guest.inputs.ready() == Ok(false);
            if !pending {
                return;
            }
            guest.ticks
        };
        native.update(|window, cx| window.render_frame(cx));
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            panic!("seated view");
        };
        assert!(
            guest.ticks > ticks,
            "a requested native frame must advance the guest"
        );
    }
}

#[gpui_kit::test]
fn call_panel_renders_staged_wasm_and_routes_native_control_clicks(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let path = tests::staged("call").expect("build current Call view first");
    let mut guest = Guest::load_from("call", &path).expect("build current Call view first");
    let props = Some(br#"{"panel":{"status":"live","muted":false}}"#.to_vec());
    guest.redraw(&None);
    settle(&mut guest, &props);
    assert!(guest.fault.is_none(), "{:?}", guest.fault);
    let seat = Arc::new(Mutex::new(Mounted {
        changes: tokio::sync::watch::channel(()).0,
        slot: Slot::Ready(Box::new(guest)),
        props,
        generation: 1,
        hash: None,
        in_flight: false,
        wanted: None,
        tasting: None,
        waiting_since: None,
        replacement: Replacement::Preserve,
        retry: None,
        shown: None,
    }));
    registry().lock().unwrap().insert("call", seat.clone());
    cx.update(gpui_kit::init);
    let window = cx.open_window(gpui::size(gpui::px(560.), gpui::px(600.)), |_, _| {
        NativeModuleView::new("call")
    });
    let view = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    native.update(|_, cx| {
        let events = events.clone();
        cx.subscribe(&view, move |_, event: &ModuleViewEvent, _| {
            events.borrow_mut().push(event.kind.clone());
        })
        .detach();
    });
    native.update(|window, cx| window.render_frame(cx));
    for (label, intent) in [
        ("Mute", "mute"),
        ("Camera", "camera"),
        ("Share screen", "screen"),
        ("Go to channel", "channel"),
        ("Leave huddle", "leave"),
    ] {
        click_before_frame(&mut native, button(&seat, label));
        native.update(|window, cx| window.render_frame(cx));
        native.run_until_parked();
        assert_eq!(events.borrow_mut().drain(..).collect::<Vec<_>>(), [intent]);
    }
}

/// Driven by node-bin's real Gateway/Git fixture. Resolve the deployed view;
/// only service.json is supplied here. Queries, merge and writes use the host.
#[gpui::test]
#[ignore = "run with node-bin's compiled_wasm_merge_updates_the_real_forge_branch fixture"]
fn forge_wasm_merges_through_the_real_service(cx: &mut TestAppContext) {
    // Real socket replies wake the presenter from the kernel runtime thread.
    cx.executor().allow_parking();
    fn has_key(node: &wire::Node, wanted: &str) -> bool {
        node.key().is_some_and(|key| key.ends_with(wanted))
            || node.children().iter().any(|child| has_key(child, wanted))
    }
    fn until(
        seat: &Arc<Mutex<Mounted>>,
        live: &tokio::sync::watch::Sender<()>,
        ready: impl Fn(&Guest) -> bool,
    ) {
        loop {
            let mut live = live.subscribe();
            let mut locked = seat.lock().unwrap();
            let props = locked.props.clone();
            let Slot::Ready(guest) = &mut locked.slot else {
                panic!("live Forge guest")
            };
            let mut replies = guest.replies.changes();
            let again = guest.redraw(&props);
            assert!(guest.fault.is_none(), "{:?}", guest.fault);
            if guest
                .frame
                .root
                .as_ref()
                .is_some_and(|root| has_key(root, "forge/error"))
            {
                panic!("Forge view refused the request: {:?}", guest.frame.root);
            }
            if ready(guest) {
                return;
            }
            drop(locked);
            if !again {
                runtime().block_on(async {
                    tokio::select! {
                        result = replies.changed() => result.expect("Forge reply event"),
                        result = live.changed() => result.expect("Forge module event"),
                    }
                });
            }
        }
    }
    let _turn = tests::blocking_connection_turn();
    let rpc = std::env::var("DUCK_FORGE_RPC").expect("node fixture RPC");
    let key = std::env::var("DUCK_FORGE_KEY").expect("fixture user key");
    let account: u64 = std::env::var("DUCK_FORGE_ACCOUNT")
        .unwrap()
        .parse()
        .unwrap();
    runtime()
        .block_on(crate::backend::seat_signer(
            key.into(),
            zeroize::Zeroizing::new("forge-test-password".into()),
        ))
        .unwrap();
    let client = crate::backend::rpc_client(&rpc).unwrap();
    connection().lock().unwrap().client = Some(client.clone());
    let source = runtime()
        .block_on(crate::backend::view_source::resolve(
            &client,
            "forge",
            None,
            &mut crate::backend::view_source::Asked::default(),
        ))
        .expect("resolve deployed Forge view");
    let crate::backend::view_source::ViewSource::Ready {
        hash, component, ..
    } = source
    else {
        panic!("Forge fixture must deploy its view");
    };
    let path = tests::staged("forge").expect("build current Forge view first");
    assert_eq!(
        component,
        std::fs::read(path).unwrap(),
        "fixture deploys current Forge WASM"
    );
    let mut guest = Guest::from_bytes("forge", &component, "deployed Forge").unwrap();
    guest.assets = Arc::new(
        [(
            "service.json".into(),
            serde_json::to_vec(&serde_json::json!({"account":account,"route":"git"})).unwrap(),
        )]
        .into(),
    );
    let props = Some(
        forge_view(
            false,
            true,
            "",
            "",
            "",
            &rpc,
            "duck://forge/wasm-merge/1",
            1,
        )
        .props,
    );
    let seat = Arc::new(Mutex::new(Mounted {
        changes: tokio::sync::watch::channel(()).0,
        slot: Slot::Ready(Box::new(guest)),
        props,
        generation: 1,
        hash: Some(hash),
        in_flight: false,
        wanted: None,
        tasting: None,
        waiting_since: None,
        replacement: Replacement::Preserve,
        retry: None,
        shown: None,
    }));
    registry().lock().unwrap().insert("forge", seat.clone());
    struct LivePump(tokio::task::JoinHandle<()>);
    impl Drop for LivePump {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let (ready, listening) = tokio::sync::oneshot::channel();
    let origin = rpc.clone();
    let live = tokio::sync::watch::channel(()).0;
    let signal = live.clone();
    let _live = LivePump(runtime().spawn(async move {
        use futures::StreamExt as _;
        let mut events = crate::backend::live_events(origin);
        let mut ready = Some(ready);
        let mut serial = 0;
        while let Some(event) = events.next().await {
            serial = view_live_hit(&event.module, serial);
            signal.send_replace(());
            let became_ready = event.kind == crate::LiveKind::Ready;
            if became_ready && let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
        }
    }));
    runtime()
        .block_on(listening)
        .expect("real module subscription ready");
    until(&seat, &live, |guest| {
        guest
            .frame
            .root
            .as_ref()
            .is_some_and(|root| has_key(root, "forge/merge"))
    });
    cx.update(gpui_kit::init);
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(900.)), |_, _| {
        NativeModuleView::new("forge")
    });
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    click_before_frame(&mut native, button(&seat, "Merge pull request"));
    native.update(|window, cx| window.render_frame(cx));
    native.run_until_parked();
    until(&seat, &live, |guest| {
        guest
            .frame
            .root
            .as_ref()
            .is_some_and(|root| has_key(root, "forge/merged"))
    });
    native.update(|window, cx| window.render_frame(cx));
}

/// The editor the canvas draws for the note being written, once its frame
/// has one: its key and the document it projects.
fn note_editor(seat: &Arc<Mutex<Mounted>>) -> Option<(String, String)> {
    fn find(node: &wire::Node) -> Option<(String, String)> {
        match node {
            wire::Node::Editor { key, document, .. } if key.starts_with("boards/editor/") => {
                Some((key.clone(), document.document.clone()))
            }
            node => node.children().iter().find_map(find),
        }
    }
    let locked = seat.lock().unwrap();
    let Slot::Ready(guest) = &locked.slot else {
        panic!("seated canvas");
    };
    assert!(guest.fault.is_none(), "{:?}", guest.fault);
    find(guest.frame.root.as_ref()?)
}

/// Canvas opens a note's editor in the frame that draws the note, while the
/// note's Create is still in flight, and asks for focus on it as soon as it
/// is shown — before the host holds the note's document. The writer is
/// already typing: the keys are the note's first words, so they have to land
/// in its editor, in order, and not reach the view as keys pressed on the
/// board, which a view with an editor open does not type into. The keys that
/// come before the document itself are the store's to keep
/// (`keys_typed_before_the_document_arrives_reach_the_guest_in_order`).
#[gpui_kit::test]
fn keys_typed_into_a_note_the_view_just_opened_reach_it_in_order(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    tests::can_reads([
        (
            "rpc.query",
            serde_json::json!({"list": {"room": "Planning"}}),
        ),
        (
            "get",
            serde_json::json!({"board": {
                "title": "Planning", "owner": "owner", "revision": 0, "shapes": {}
            }}),
        ),
    ]);
    tests::hold("op.submit");
    let props = Some(br#"{"connected":true,"dark":false,"chain":"test"}"#.to_vec());
    let path = tests::staged("canvas").expect("build current canvas view first");
    let mut guest = Guest::load_from("canvas", &path).expect("build current canvas view first");
    tests::settle_documents(&mut guest, &props);
    let seat = Arc::new(Mutex::new(Mounted {
        changes: tokio::sync::watch::channel(()).0,
        slot: Slot::Ready(Box::new(guest)),
        props,
        generation: 1,
        hash: None,
        in_flight: false,
        wanted: None,
        tasting: None,
        waiting_since: None,
        replacement: Replacement::Preserve,
        retry: None,
        shown: None,
    }));
    registry().lock().unwrap().insert("canvas", seat.clone());
    cx.update(gpui_kit::init);
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |_, _| {
        NativeModuleView::new("canvas")
    });
    let view = window.root(cx).unwrap();
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| window.render_frame(cx));
    settle_native_documents(&mut native, &seat);

    native.update(|window, cx| window.click("boards/first-note", cx));
    let mut frames = 0;
    let (editor, document) = loop {
        native.run_until_parked();
        if let Some(editor) = note_editor(&seat) {
            break editor;
        }
        frames += 1;
        assert!(frames < 32, "Add a note opened no editor");
        native.update(|window, cx| window.render_frame(cx));
    };
    let focused = native.update(|window, cx| {
        let content = view.read(cx).content.clone().expect("a mounted tree");
        content.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focused {
                    target: editor.clone(),
                },
                window,
                cx,
            )
        })
    });
    assert!(
        wire::decode::<bool>(&focused.unwrap()).unwrap(),
        "the note's field does not hold focus yet, so the keys typed now are keys pressed on the board"
    );

    input::record_inputs();
    for key in "kiwi mix".chars() {
        native.update(|window, cx| {
            let text = key.to_string();
            let mut stroke = gpui::Keystroke::parse(&text).expect("a character is a keystroke");
            stroke.key_char = Some(text);
            window.dispatch_keystroke(stroke, cx);
        });
    }
    settle_native_documents(&mut native, &seat);
    let written: String = input::recorded_inputs()
        .into_iter()
        .filter_map(|event| match event {
            wire::Event::EditorTransaction {
                event:
                    wire::EditorTransactionEvent::Commit {
                        before, patches, ..
                    },
                ..
            } if before.document == document => Some(patches),
            _ => None,
        })
        .flatten()
        .map(|patch| patch.replacement)
        .collect();
    assert_eq!(
        written, "kiwi mix",
        "the note's editor did not get every key, in order"
    );
}
