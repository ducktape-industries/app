//! Actual staged Chat and Pages views in native GPUI windows.
use super::*;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{self as gpui, Entity, TestAppContext, VisualTestContext};

fn seated(opened: &[&str]) -> Arc<Mutex<Mounted>> {
    seated_as("chat", opened)
}
/// The staged chat view seated under `module`: a second copy makes a second layer.
fn seated_as(module: &'static str, opened: &[&str]) -> Arc<Mutex<Mounted>> {
    tests::can_the_chat_room();
    let path = tests::staged("chat").expect("build current chat view first");
    let mut guest = Guest::load_from(module, &path).expect("build current chat view first");
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
    registry().lock().unwrap().insert(module, seat.clone());
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
fn shell_tab_switches_hide_and_restore_the_retained_guest() {
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
    assert!(
        !visible(),
        "the previous tab remains hidden while another tab is rendered"
    );
    presenter.update(&mut cx, |view, cx| {
        view.test_dispatch(
            crate::AppMessage::SelectShellTab(crate::ShellTab::View("chat")),
            cx,
        )
    });
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(visible());
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

/// Guests painted as absolutely placed layers, later ones on top: the OS-mode
/// shell's workspace without its chrome.
struct Layers {
    views: Vec<(Entity<NativeModuleView>, gpui::Bounds<gpui::Pixels>)>,
}
impl gpui::Render for Layers {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        gpui::div()
            .size_full()
            .relative()
            .children(self.views.iter().map(|(view, bounds)| {
                gpui::div()
                    .absolute()
                    .left(bounds.origin.x)
                    .top(bounds.origin.y)
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .child(view.clone())
            }))
    }
}
fn layer(x: f32, y: f32) -> gpui::Bounds<gpui::Pixels> {
    gpui::Bounds {
        origin: gpui::point(gpui::px(x), gpui::px(y)),
        size: gpui::size(gpui::px(400.), gpui::px(400.)),
    }
}
fn open_layers(
    cx: &mut TestAppContext,
    placed: &[(&'static str, gpui::Bounds<gpui::Pixels>)],
) -> (Vec<Entity<NativeModuleView>>, VisualTestContext) {
    cx.update(gpui_kit::init);
    let views: Vec<_> = placed
        .iter()
        .map(|(module, bounds)| {
            seated_as(module, &[]);
            (cx.new(|_| NativeModuleView::new(module)), *bounds)
        })
        .collect();
    let window = cx.open_window(gpui::size(gpui::px(1200.), gpui::px(800.)), |_, _| Layers {
        views: views.clone(),
    });
    let mut native = VisualTestContext::from_window(window.into(), cx);
    native.update(|window, cx| {
        window.render_frame(cx);
        observe_mouse();
    });
    (views.into_iter().map(|(view, _)| view).collect(), native)
}
/// Both layers' modules, so a test can re-arm them by name.
const LAYERS: [&str; 2] = ["chat", "chat-b"];
/// Chat never asks for mouse observations and every redraw replaces the
/// guest's frame, so the harness opts in after each draw and before the
/// deferred delivery that follows it.
fn observe_mouse() {
    for module in LAYERS {
        let Some(seat) = registry().lock().unwrap().get(module).cloned() else {
            continue;
        };
        let mut locked = seat.lock().unwrap();
        if let Slot::Ready(guest) = &mut locked.slot {
            guest.frame.mouse_interest = true;
        }
    }
}
fn pointer(native: &mut VisualTestContext, x: f32, y: f32, event: &str) {
    let position = gpui::point(gpui::px(x), gpui::px(y));
    let modifiers = Default::default();
    native.update(|window, cx| {
        let event = match event {
            "move" => gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }),
            "drag" => gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers,
            }),
            "down" => gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position,
                button: gpui::MouseButton::Left,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            "up" => gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position,
                button: gpui::MouseButton::Left,
                modifiers,
                click_count: 1,
            }),
            _ => unreachable!(),
        };
        window.dispatch_event(event, cx);
        observe_mouse();
    });
}
fn mouse_events_of(
    recorded: &[(&'static str, wire::Event)],
    module: &str,
) -> Vec<wire::mouse::Event> {
    recorded
        .iter()
        .filter(|(guest, _)| *guest == module)
        .filter_map(|(_, event)| match event {
            wire::Event::Mouse { event, .. } => Some(*event),
            _ => None,
        })
        .collect()
}
#[gpui_kit::test]
fn side_by_side_layers_each_take_only_their_own_click(cx: &mut TestAppContext) {
    use wire::mouse::{Button::Left, Event as M};
    let _turn = tests::blocking_connection_turn();
    let (_, mut native) = open_layers(
        cx,
        &[("chat", layer(100., 100.)), ("chat-b", layer(600., 100.))],
    );
    input::record_inputs();
    pointer(&mut native, 150., 160., "move");
    pointer(&mut native, 150., 160., "down");
    pointer(&mut native, 150., 160., "up");
    pointer(&mut native, 700., 130., "move");
    pointer(&mut native, 700., 130., "down");
    pointer(&mut native, 700., 130., "up");
    let recorded = input::recorded_inputs_by_guest();
    assert_eq!(
        mouse_events_of(&recorded, "chat"),
        vec![
            M::CursorEntered,
            M::CursorMoved { x: 50., y: 60. },
            M::ButtonPressed(Left),
            M::ButtonReleased(Left),
            M::CursorLeft,
        ]
    );
    assert_eq!(
        mouse_events_of(&recorded, "chat-b"),
        vec![
            M::CursorEntered,
            M::CursorMoved { x: 100., y: 30. },
            M::ButtonPressed(Left),
            M::ButtonReleased(Left),
        ]
    );
}
#[gpui_kit::test]
fn a_click_on_overlapping_layers_reaches_only_the_top_one(cx: &mut TestAppContext) {
    use wire::mouse::{Button::Left, Event as M};
    let _turn = tests::blocking_connection_turn();
    let (_, mut native) = open_layers(
        cx,
        &[("chat", layer(100., 100.)), ("chat-b", layer(300., 300.))],
    );
    input::record_inputs();
    pointer(&mut native, 350., 350., "move");
    pointer(&mut native, 350., 350., "down");
    pointer(&mut native, 350., 350., "up");
    let recorded = input::recorded_inputs_by_guest();
    assert_eq!(mouse_events_of(&recorded, "chat"), vec![]);
    assert_eq!(
        mouse_events_of(&recorded, "chat-b"),
        vec![
            M::CursorEntered,
            M::CursorMoved { x: 50., y: 50. },
            M::ButtonPressed(Left),
            M::ButtonReleased(Left),
        ]
    );
}
#[gpui_kit::test]
fn a_drag_keeps_its_moves_and_release_in_the_layer_it_began_in(cx: &mut TestAppContext) {
    use wire::mouse::{Button::Left, Event as M};
    let _turn = tests::blocking_connection_turn();
    let (_, mut native) = open_layers(
        cx,
        &[("chat", layer(100., 100.)), ("chat-b", layer(600., 100.))],
    );
    input::record_inputs();
    pointer(&mut native, 200., 200., "move");
    pointer(&mut native, 200., 200., "down");
    pointer(&mut native, 250., 250., "drag");
    pointer(&mut native, 700., 300., "drag");
    pointer(&mut native, 700., 300., "up");
    pointer(&mut native, 50., 50., "move");
    let recorded = input::recorded_inputs_by_guest();
    assert_eq!(
        mouse_events_of(&recorded, "chat"),
        vec![
            M::CursorEntered,
            M::CursorMoved { x: 100., y: 100. },
            M::ButtonPressed(Left),
            M::CursorMoved { x: 150., y: 150. },
            M::CursorLeft,
            M::CursorMoved { x: 600., y: 200. },
            M::ButtonReleased(Left),
        ]
    );
    assert_eq!(
        mouse_events_of(&recorded, "chat-b"),
        vec![M::CursorEntered, M::CursorLeft]
    );
}
#[gpui_kit::test]
fn keys_reach_only_the_focused_layer(cx: &mut TestAppContext) {
    let _turn = tests::blocking_connection_turn();
    let (views, mut native) = open_layers(
        cx,
        &[("chat", layer(100., 100.)), ("chat-b", layer(600., 100.))],
    );
    // The shell focuses the active layer; here the layer's first keyed
    // container stands in for it.
    let target = {
        let seat = registry().lock().unwrap()["chat-b"].clone();
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            unreachable!()
        };
        let mut root = guest.frame.root.clone().unwrap();
        let mut target = None;
        root.for_each_mut(&mut |node| {
            if target.is_none()
                && matches!(
                    node,
                    wire::Node::Container { .. } | wire::Node::Linear { .. }
                )
            {
                target = node.key().map(str::to_owned);
            }
        });
        target.expect("a keyed container")
    };
    let focused = native.update(|window, cx| {
        let content = views[1].read(cx).content.clone().unwrap();
        content.update(cx, |tree, cx| {
            tree.execute_widget_command(
                wire::WidgetCommand::Focus {
                    target: target.clone(),
                },
                window,
                cx,
            )
            .unwrap();
            tree.execute_widget_command(wire::WidgetCommand::Focused { target }, window, cx)
        })
    });
    assert!(wire::decode::<bool>(&focused.unwrap()).unwrap());
    native.update(|window, cx| window.render_frame(cx));
    input::record_inputs();
    native.update(|window, cx| {
        let mut stroke = gpui::Keystroke::parse("a").unwrap();
        stroke.key_char = Some("a".into());
        window.dispatch_keystroke(stroke, cx);
    });
    let keyed: Vec<_> = input::recorded_inputs_by_guest()
        .into_iter()
        .filter(|(_, event)| matches!(event, wire::Event::Keyboard { .. }))
        .map(|(module, _)| module)
        .collect();
    assert!(!keyed.is_empty(), "the focused layer takes the key");
    assert!(keyed.iter().all(|module| *module == "chat-b"), "{keyed:?}");
}
