use super::*;

mod node_methods;

/// A guest holding every capability, its code doing nothing.
pub(super) fn guest() -> Guest {
    let code = wasmtime::Module::new(
        super::super::guest::engine(),
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0)
            (func (export "snapshot") (result i64) i64.const 0)
            (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#,
    )
    .unwrap();
    let mut guest = Guest::instantiate("request-test", &code, "request test").unwrap();
    guest.capabilities = Capability::ALL.to_vec();
    guest
}

/// One request off `stream` to a fake node: its request line, and the
/// body its Content-Length gives.
pub(super) fn read_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
    use std::io::{BufRead as _, Read as _};
    let mut reader = std::io::BufReader::new(stream);
    let mut request = String::new();
    reader.read_line(&mut request).unwrap();
    let mut line = String::new();
    let mut length = 0;
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    (request, body)
}

/// A fake node's answer on `stream`: `status`, then `body`.
pub(super) fn respond(stream: &mut std::net::TcpStream, status: &str, body: &[u8]) {
    use std::io::Write as _;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

/// The reason `kind` is refused for, asked by a guest declaring `declared`;
/// `None` if it is not refused at once.
fn refused_with(declared: &[Capability], kind: &str) -> Option<String> {
    let mut guest = guest();
    guest.capabilities = declared.to_vec();
    let payload = methods::encode(&methods::Call {
        target: "registry".into(),
        body: Vec::new(),
    });
    for id in [1, 2] {
        let request = wire::Request {
            id,
            kind: kind.into(),
            payload: payload.clone(),
        };
        guest.answer(request, &None);
    }
    assert!(guest.undeclared_logged.len() <= 1, "{kind} logged twice");
    refusal_code(&mut guest)
}

/// The code the guest's last pending event refuses with; `None` if that
/// event is not a refusal.
fn refusal_code(guest: &mut Guest) -> Option<String> {
    match guest.pending.pop() {
        Some(wire::Event::Response {
            result: Err(refusal),
            ..
        }) => Some(refusal.code),
        _ => None,
    }
}

/// Every method of every capability family: refused when its capability is
/// the one the manifest leaves out, answered past the gate when it is the
/// only one declared.
#[test]
fn a_method_is_reached_only_through_its_declared_capability() {
    for kind in methods::ALL {
        let family = Capability::of_kind(kind).unwrap().0;
        let others: Vec<Capability> = Capability::ALL
            .into_iter()
            .filter(|c| *c != family)
            .collect();
        assert_eq!(
            refused_with(&others, kind).as_deref(),
            Some(refusal::UNDECLARED_CAPABILITY),
            "{kind} undeclared"
        );
        assert!(
            !matches!(
                refused_with(&[family], kind).as_deref(),
                Some(refusal::UNDECLARED_CAPABILITY | refusal::UNKNOWN_REQUEST)
            ),
            "{kind} declared"
        );
    }
    // an unknown kind is still unknown, declared or not
    assert_eq!(
        refused_with(&[], "chat.props").as_deref(),
        Some(refusal::UNKNOWN_REQUEST)
    );
}

#[test]
fn unknown_kinds_finish_with_a_typed_refusal() {
    let mut guest = guest();
    for (id, kind) in [
        "missing",
        "rpc.unknown",
        "rpc.view",
        "rpc.query_bytes",
        "op.submit_bytes",
        "chat.props",
        "picture.load",
    ]
    .into_iter()
    .enumerate()
    {
        guest.answer(
            wire::Request {
                id: id as u64,
                kind: kind.into(),
                payload: Vec::new(),
            },
            &None,
        );
        let Some(wire::Event::Response {
            id: answered,
            result: Err(refusal),
            done,
        }) = guest.pending.pop()
        else {
            panic!("{kind} must answer a refusal");
        };
        assert_eq!(answered, id as u64);
        assert!(done);
        assert_eq!(refusal.code, refusal::UNKNOWN_REQUEST);
        assert!(refusal.message.contains(kind));
    }
    assert!(guest.pending.is_empty());
}

/// Every kind in `methods::ALL` has a handler on this side: none of them is
/// `unknown_request`, whatever else a bare guest with no node refuses it
/// for. A method added to the list without a handler fails here.
#[test]
fn every_method_is_answered() {
    for (id, kind) in methods::ALL.iter().enumerate() {
        let mut guest = guest();
        guest.answer(
            wire::Request {
                id: id as u64,
                kind: (*kind).into(),
                payload: methods::encode(&methods::Call {
                    target: "registry".into(),
                    body: Vec::new(),
                }),
            },
            &None,
        );
        assert_ne!(
            refusal_code(&mut guest).as_deref(),
            Some(refusal::UNKNOWN_REQUEST),
            "{kind} has no handler"
        );
    }
}

/// A node method routes to the node handler, which answers for the missing
/// node before it reads the request; what the request says is judged by
/// `query` itself, below.
#[test]
fn node_methods_answer_for_the_missing_node_first() {
    let mut guest = guest();
    assert!(answer(
        &mut guest,
        Capability::Module,
        "query",
        7,
        b"not borsh"
    ));
    assert!(matches!(guest.pending.pop(), Some(wire::Event::Response {
        id: 7, result: Err(refusal), done: true
    }) if refusal.code == refusal::NOT_CONNECTED));
}
/// `link.open` opens `duck://` and `https://` and refuses every other
/// scheme at the method, before the app is asked.
#[test]
fn open_link_refuses_any_scheme_but_duck_and_https() {
    let open = |link: &str| {
        let mut guest = guest();
        guest.user_activation = Some(());
        guest.answer(
            wire::Request {
                id: 5,
                kind: "link.open".into(),
                payload: methods::encode(&link.to_owned()),
            },
            &None,
        );
        (refusal_code(&mut guest), guest.intents.len())
    };
    for link in ["duck://chat/general", "https://example.com/a"] {
        assert_eq!(open(link), (None, 1), "{link}");
    }
    for link in [
        "http://example.com",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mailto:a@b.c",
        "duck://",
        "",
    ] {
        assert_eq!(
            open(link),
            (Some(refusal::MALFORMED_REQUEST.into()), 0),
            "{link}"
        );
    }
}
/// A `link.open` request as a view sends it.
fn link_request(id: u64, link: &str) -> wire::Request {
    wire::Request {
        id,
        kind: "link.open".into(),
        payload: methods::encode(&link.to_owned()),
    }
}

/// `link.open` needs the redraw's gesture, and one gesture admits one
/// link: a view on a clock, or one handed a route by an OS `duck://`
/// link, opens nothing.
#[test]
fn link_open_needs_a_gesture_and_one_gesture_admits_one_link() {
    let mut guest = guest();
    guest.answer(link_request(1, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("needs_gesture"));
    // a route the OS handed the view (`Windows::open_seat`) is no gesture
    guest.route_subscriptions.push(2);
    crate::runtime::route_to("request-test", "tx/1".into());
    guest.sync_route();
    guest.answer(link_request(3, "duck://request-test/tx/1"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("needs_gesture"));
    assert!(guest.intents.is_empty(), "a link opened with no gesture");
    guest.user_activation = Some(());
    guest.answer(link_request(4, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut guest), None);
    guest.answer(link_request(5, "https://example.com/b"), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some("needs_gesture"),
        "a second link on the same gesture"
    );
    assert_eq!(guest.intents.len(), 1);
}

/// Four links a minute per view, each on its own gesture; the fifth is
/// refused `link_limit` until the window moves on.
#[test]
fn link_open_refuses_the_fifth_link_in_a_minute() {
    let mut guest = guest();
    for id in 1..=4 {
        guest.user_activation = Some(());
        guest.link_opened = false;
        guest.answer(link_request(id, "https://example.com/"), &None);
        assert_eq!(refusal_code(&mut guest), None, "link {id}");
    }
    guest.user_activation = Some(());
    guest.link_opened = false;
    guest.answer(link_request(5, "https://example.com/"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("link_limit"));
    assert_eq!(guest.intents.len(), 4);
    // a minute on, the oldest is forgotten and one more goes through
    guest.links[0] -= std::time::Duration::from_secs(61);
    guest.user_activation = Some(());
    guest.link_opened = false;
    guest.answer(link_request(6, "https://example.com/"), &None);
    assert_eq!(refusal_code(&mut guest), None);
    assert_eq!(guest.intents.len(), 5);
}

/// The clipboard is read and written only on a gesture.
#[test]
fn the_clipboard_needs_a_gesture() {
    let request = |id: u64, kind: &str| wire::Request {
        id,
        kind: kind.into(),
        payload: methods::encode(&"copied".to_owned()),
    };
    let mut guest = guest();
    for (id, kind) in [(1, "clipboard.read"), (2, "clipboard.write")] {
        guest.answer(request(id, kind), &None);
        assert_eq!(
            refusal_code(&mut guest).as_deref(),
            Some("needs_gesture"),
            "{kind}"
        );
    }
    guest.user_activation = Some(());
    for (id, kind) in [(3, "clipboard.read"), (4, "clipboard.write")] {
        guest.answer(request(id, kind), &None);
        assert_eq!(refusal_code(&mut guest), None, "{kind} with a gesture");
    }
}

fn container(id: &str, children: Vec<wire::Node>) -> wire::Node {
    wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name(id.into())),
        style: Default::default(),
        interactivity: Default::default(),
        children,
    })
}

/// An editor whose document the guest has not sent yet (`inputs.pending()`
/// until `seed_editor_text`), with a binding so a key reaches the guest.
fn editor_root() -> wire::Node {
    container(
        "root",
        vec![wire::Node::Editor {
            binding: Some(Box::new(view_wire::EditorBinding {
                claims: Vec::new(),
                on_request: 1,
                on_event: 2,
            })),
            id: wire::ElementIdWire::Name("composer".into()),
            style: Default::default(),
            placeholder: String::new(),
            label: None,
            document: wire::editor_document::EditorDocumentRef {
                document: "draft".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: wire::EditorCursor::default(),
                byte_len: 5,
            },
            on_document: 0,
            editable: true,
        }],
    )
}

/// The composer's authored path, as the store keys it.
fn composer() -> Vec<wire::ElementIdWire> {
    vec![
        wire::ElementIdWire::Name("root".into()),
        wire::ElementIdWire::Name("composer".into()),
    ]
}

/// A `host.widget` command runs a frame after it was asked: it carries
/// its own redraw's gesture to the seat's gate, never a later one's.
#[test]
fn a_widget_command_carries_its_redraws_gesture() {
    let mut guest = guest();
    guest.frame.root = Some(container("box", Vec::new()));
    let focus = wire::WidgetCommand::Focus {
        target: vec![wire::ElementIdWire::Name("box".into())],
    };
    guest.widget_request(1, &wire::encode(&focus));
    guest.user_activation = Some(());
    guest.widget_request(2, &wire::encode(&focus));
    assert!(
        guest.gesture_used,
        "a gestured command spends the redraw's gesture"
    );
    let gestured: Vec<_> = guest
        .widget_commands
        .iter()
        .map(|(id, _, gestured)| (*id, *gestured))
        .collect();
    assert_eq!(gestured, [(1, false), (2, true)]);
    let mut seen = Vec::new();
    guest.execute_widget_commands(|_, gestured| {
        seen.push(gestured);
        Ok(Vec::new())
    });
    assert_eq!(seen, [false, true]);
}

/// A view keeps its editor's document mid-flight (never acknowledges the
/// commit) so the commands queued on one press are held, and releases
/// them once the person is typing elsewhere: a held command's gesture is
/// gone, so it is judged by the keys being free when it finally runs.
#[test]
fn a_widget_command_held_past_its_frame_loses_its_gesture() {
    let mut guest = guest();
    let root = editor_root();
    guest.inputs.replace(&root).unwrap();
    guest.frame.root = Some(root);
    crate::editor::wire::seed_editor_text(&guest.inputs, "words");
    guest.pending.clear();
    // a key the binding is deciding on: the document is mid-flight
    guest.inputs.key_for_test(&composer(), command_v());
    let id = deliver(&mut guest).expect("the key reached the binding");
    assert!(guest.inputs.pending(), "the document is mid-flight");
    guest.pending.clear();
    guest.spend_gesture();
    guest.user_activation = Some(());
    for (id, command) in [
        (1, wire::WidgetCommand::CursorEnd { target: composer() }),
        (2, wire::WidgetCommand::FocusHandle { handle: 7 }),
    ] {
        guest.widget_request(id, &wire::encode(&command));
    }
    assert_eq!(
        guest.runnable_widget_commands(),
        0,
        "held behind the document"
    );
    guest.execute_widget_commands(|_, _| panic!("nothing runs while held"));
    assert!(
        guest
            .widget_commands
            .iter()
            .all(|(_, _, gestured)| !gestured),
        "a held command kept its gesture"
    );
    // the view decides and acknowledges (a frame whose editor carries the
    // committed reference): the commands run ungestured
    decide_noop(&mut guest, id);
    guest.pending.extend(guest.inputs.drain());
    let after = guest
        .pending
        .iter()
        .find_map(|event| match event {
            wire::Event::EditorTransaction {
                event: view_wire::EditorTransactionEvent::Commit { after, .. },
                ..
            } => Some(after.clone()),
            _ => None,
        })
        .expect("the key committed");
    let mut acknowledged = editor_root();
    if let wire::Node::Container(view_wire::ContainerNode { children, .. }) = &mut acknowledged
        && let Some(wire::Node::Editor { document, .. }) = children.first_mut()
    {
        *document = after;
    }
    guest.inputs.replace(&acknowledged).unwrap();
    guest.inputs.frame(&wire::Frame::default()).unwrap();
    assert!(!guest.inputs.pending(), "the document settled");
    let mut seen = Vec::new();
    guest.execute_widget_commands(|_, gestured| {
        seen.push(gestured);
        Ok(Vec::new())
    });
    assert_eq!(seen, [false, false]);
}

/// ⌘V as a `KeyState`.
fn command_v() -> view_wire::keyboard::KeyState {
    use view_wire::keyboard::{Key, KeyState, Location, NativeCode, Physical};
    KeyState {
        key: Key::Character("v".into()),
        modified_key: Key::Character("v".into()),
        physical_key: Physical::Unidentified(NativeCode::Unidentified),
        location: Location::Standard,
        modifiers: gpui_kit::Modifiers {
            platform: true,
            ..Default::default()
        },
    }
}

/// The key's delivery, as `redraw` takes it in: the store drained into
/// `pending`, the gesture taken from it; answers the request's id.
fn deliver(guest: &mut Guest) -> Option<view_wire::EditorTransactionId> {
    guest.pending.extend(guest.inputs.drain());
    guest.take_in_key_gesture();
    guest.pending.iter().find_map(|event| match event {
        wire::Event::EditorRequest { request, .. } => Some(request.id.clone()),
        _ => None,
    })
}

/// The binding's decision on the key (`Noop`: the composer acts on the
/// commit, not the key), committed by the host with the key as origin.
fn decide_noop(guest: &mut Guest, id: view_wire::EditorTransactionId) {
    guest
        .inputs
        .frame(&wire::Frame {
            editor_decisions: vec![view_wire::EditorResponse {
                id,
                decision: view_wire::EditorDecision::Noop,
            }],
            ..Default::default()
        })
        .unwrap();
}

fn clipboard_read(guest: &mut Guest, id: u64) -> Option<String> {
    guest.answer(
        wire::Request {
            id,
            kind: "clipboard.read".into(),
            payload: Vec::new(),
        },
        &None,
    );
    refusal_code(guest)
}

/// Chat's paste: ⌘V is a key the composer claims, so the host hands it to
/// the binding (one redraw), which decides `Noop`, and the guest acts on
/// the commit the host sends back (the next redraw) with `clipboard.read`.
/// The key's gesture, taken in at its delivery and not spent there, is
/// still the commit's: the read is admitted. Through the real
/// `take_in_key_gesture`/`spend_gesture` path `redraw` runs.
#[test]
fn a_pasted_keys_gesture_reaches_the_clipboard_on_its_commit() {
    let mut guest = guest();
    let root = editor_root();
    guest.inputs.replace(&root).unwrap();
    guest.frame.root = Some(root);
    crate::editor::wire::seed_editor_text(&guest.inputs, "words");
    guest.pending.clear();
    guest.inputs.key_for_test(&composer(), command_v());
    let id = deliver(&mut guest).expect("the key reached the binding");
    assert!(
        guest.user_activation.is_some(),
        "the key is a gesture at its delivery"
    );
    guest.pending.clear();
    guest.spend_gesture();
    assert!(guest.user_activation.is_none(), "the redraw spent the mark");
    decide_noop(&mut guest, id);
    assert!(deliver(&mut guest).is_none(), "no second request");
    assert!(
        guest.pending.iter().any(|event| matches!(
            event,
            wire::Event::EditorTransaction {
                event: view_wire::EditorTransactionEvent::Commit {
                    origin: Some(view_wire::EditorRequestInput::Key { .. }),
                    ..
                },
                ..
            }
        )),
        "the commit carries the key as its origin: {:?}",
        guest.pending
    );
    assert_eq!(clipboard_read(&mut guest, 9), None, "the paste was refused");
    guest.spend_gesture();
}

/// One key admits one redraw's requests: a view that reads the clipboard
/// as the key is delivered gets nothing more on its commit, and a key the
/// view held back (its document mid-flight) past a second grants nothing.
#[test]
fn a_key_admits_one_redraw_and_a_stale_key_none() {
    let mut guest = guest();
    let root = editor_root();
    guest.inputs.replace(&root).unwrap();
    guest.frame.root = Some(root);
    crate::editor::wire::seed_editor_text(&guest.inputs, "words");
    guest.pending.clear();
    guest.inputs.key_for_test(&composer(), command_v());
    let id = deliver(&mut guest).expect("the key reached the binding");
    assert_eq!(clipboard_read(&mut guest, 1), None, "read at delivery");
    guest.pending.clear();
    guest.spend_gesture();
    decide_noop(&mut guest, id);
    deliver(&mut guest);
    assert_eq!(
        clipboard_read(&mut guest, 2).as_deref(),
        Some("needs_gesture"),
        "the same key admitted a second redraw"
    );
    guest.pending.clear();
    guest.spend_gesture();
    // a key held for five seconds: stale
    let stale = wire::Event::EditorRequest {
        handler: 1,
        request: view_wire::EditorRequest {
            id: view_wire::EditorTransactionId {
                instance: 1,
                document: "draft".into(),
                reset: 1,
                sequence: 1,
                attempt: 0,
                text_revision: 0,
                revision: 0,
            },
            state: wire::editor_document::EditorDocumentRef {
                document: "draft".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: wire::EditorCursor::default(),
                byte_len: 5,
            },
            input: view_wire::EditorRequestInput::Key {
                key: command_v(),
                repeat: false,
            },
            input_time_ms: guest.inputs.now_ms().saturating_sub(5_000),
        },
    };
    guest.pending.push(stale);
    guest.take_in_key_gesture();
    assert!(
        guest.user_activation.is_none(),
        "a stale key granted a gesture"
    );
    // Escape, fresh, is no gesture either
    let mut escape = command_v();
    escape.key = view_wire::keyboard::Key::Named(view_wire::keyboard::Named::Escape);
    escape.modifiers = Default::default();
    guest.pending.clear();
    guest.inputs.key_for_test(&composer(), escape);
    deliver(&mut guest);
    assert!(guest.user_activation.is_none(), "Escape granted a gesture");
}

#[test]
fn system_kinds_route_to_the_node_handler() {
    for (capability, operation) in [
        (Capability::Chain, "status"),
        (Capability::Invite, "create"),
    ] {
        let mut guest = guest();
        assert!(answer(&mut guest, capability, operation, 19, b"invalid"));
        assert!(matches!(guest.pending.pop(), Some(wire::Event::Response {
            id: 19, result: Err(refusal), done: true
        }) if refusal.code == refusal::NOT_CONNECTED));
    }
}

#[test]
fn host_session_is_program_independent_and_tracks_updates() {
    let mut guest = guest();
    let props = Some(super::super::props(
        true,
        true,
        "test-network",
        "abcd",
        None,
        "http://127.0.0.1:19001",
    ));
    guest.answer(
        wire::Request {
            id: 22,
            kind: "host.session".into(),
            payload: Vec::new(),
        },
        &props,
    );
    assert_eq!(guest.props_subscription, Some(22));
    let Some(wire::Event::Response {
        result: Ok(bytes),
        done: false,
        ..
    }) = guest.pending.pop()
    else {
        panic!("props must stay live")
    };
    let decoded: methods::Session = methods::decode(&bytes).unwrap();
    assert_eq!(decoded.endpoint, "http://127.0.0.1:19001");
    assert_eq!((decoded.signer.as_str(), decoded.account), ("abcd", None));
    assert!(decoded.dark);
    // the key's account resolves: the same subscription hears it
    let changed = Some(super::super::props(
        false,
        true,
        "test-network",
        "abcd",
        Some(7),
        "http://127.0.0.1:19001",
    ));
    guest.sync_props(&changed);
    let Some(wire::Event::Response {
        id: 22,
        result: Ok(bytes),
        done: false,
    }) = guest.pending.pop()
    else {
        panic!("a changed session is pushed")
    };
    let decoded: methods::Session = methods::decode(&bytes).unwrap();
    assert_eq!(decoded.account, Some(7));
}

#[test]
fn host_offset_hands_the_readers_utc_offset_and_its_moves() {
    let mut guest = guest();
    guest.answer(
        wire::Request {
            id: 23,
            kind: "host.offset".into(),
            payload: Vec::new(),
        },
        &None,
    );
    let Some(wire::Event::Response {
        id: 23,
        result: Ok(bytes),
        done: false,
    }) = guest.pending.pop()
    else {
        panic!("the offset subscription stays live")
    };
    let minutes: i32 = methods::decode(&bytes).unwrap();
    assert_eq!(minutes, super::offset_minutes());
    assert!((-14 * 60..=14 * 60).contains(&minutes));
    // the same offset is not sent twice; a moved one (DST) is
    guest.sync_offset(minutes);
    assert!(guest.pending.is_empty());
    guest.sync_offset(minutes + 60);
    let Some(wire::Event::Response {
        id: 23,
        result: Ok(bytes),
        done: false,
    }) = guest.pending.pop()
    else {
        panic!("a moved offset is pushed")
    };
    assert_eq!(methods::decode::<i32>(&bytes).unwrap(), minutes + 60);
    // a subscriber that arrives after a move the others have not heard
    // brings them up to date with it
    guest.answer(
        wire::Request {
            id: 24,
            kind: "host.offset".into(),
            payload: Vec::new(),
        },
        &None,
    );
    let heard: Vec<_> = guest
        .pending
        .drain(..)
        .map(|event| match event {
            wire::Event::Response {
                id,
                result: Ok(bytes),
                done: false,
            } => (id, methods::decode::<i32>(&bytes).unwrap()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(heard, [(23, minutes), (24, minutes)]);
}

/// `host.id` replies a borsh `String` like every other method
/// (`HostId::decode_reply` is `methods::decode::<String>`): raw UTF-8 with
/// no length prefix builds fine here and fails only in the view's decode.
#[test]
fn host_id_answers_a_borsh_string_a_view_can_decode() {
    let mut guest = guest();
    guest.answer(
        wire::Request {
            id: 9,
            kind: "host.id".into(),
            payload: methods::encode(&"channel".to_string()),
        },
        &None,
    );
    let Some(wire::Event::Response {
        id: 9,
        result: Ok(bytes),
        done: true,
    }) = guest.pending.pop()
    else {
        panic!("host.id must answer once");
    };
    let minted: String =
        methods::decode(&bytes).expect("a view decodes `host.id`'s reply as one borsh String");
    assert!(minted.starts_with("channel-"), "{minted}");
}

/// The kinds this host routes, read from its own `("<cap>", "<op>")` match
/// arms under `src/runtime`, are exactly `methods::ALL`: `every_method_is_answered`
/// is the one direction, this the other — nothing is served that is not a method.
#[test]
fn the_routed_kinds_are_exactly_the_methods() {
    let mut served = std::collections::BTreeSet::new();
    let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && !path.to_string_lossy().contains("test")
            {
                for line in std::fs::read_to_string(&path).unwrap().lines() {
                    served.extend(routed_kind(line));
                }
            }
        }
    }
    let all: std::collections::BTreeSet<String> =
        methods::ALL.iter().map(|kind| (*kind).to_owned()).collect();
    let unserved: Vec<_> = all.difference(&served).collect();
    let unknown: Vec<_> = served.difference(&all).collect();
    assert!(
        unserved.is_empty() && unknown.is_empty(),
        "methods not routed: {unserved:?}; routed kinds that are not methods: {unknown:?}"
    );
}

/// `(Capability::Host, "badge")` on a line → `host.badge`.
fn routed_kind(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("(Capability::")?;
    let (variant, rest) = rest.split_once(", \"")?;
    let (operation, _) = rest.split_once("\")")?;
    let capability = Capability::ALL
        .into_iter()
        .find(|c| format!("{c:?}") == variant)?;
    let word = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    word(operation).then(|| format!("{}.{operation}", capability.as_str()))
}
