use super::*;
use std::time::{Duration, Instant};

mod node_methods;

/// A guest holding every capability, its code doing nothing.
pub(in crate::runtime) fn guest() -> Guest {
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
    guest.targets = vec!["registry".into()];
    guest
}

/// The in-order queue `name` held by a job that waits for the returned
/// sender to send or drop, so every job queued behind it waits too.
pub(in crate::runtime) fn held(name: &str) -> std::sync::mpsc::Sender<()> {
    let (release, released) = std::sync::mpsc::channel::<()>();
    in_order(name, move || {
        let _ = released.recv();
    });
    release
}

/// Whether `future` is still waiting, polled once.
pub(in crate::runtime) fn waiting<F: Future>(future: std::pin::Pin<&mut F>) -> bool {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    future.poll(&mut cx).is_pending()
}

/// One request off `stream` to a fake node: its request line, and the
/// body its Content-Length gives.
pub(in crate::runtime) fn read_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
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
pub(in crate::runtime) fn respond(stream: &mut std::net::TcpStream, status: &str, body: &[u8]) {
    use std::io::Write as _;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

/// What a [`FakeNode`] does with a request on one route.
#[derive(Clone)]
pub(super) enum Mode {
    /// `200 OK` with the body, at once.
    Answer(Vec<u8>),
    /// `200 OK` with the body once the hold is over: a recovering node.
    After(std::time::Duration, Vec<u8>),
    /// Reads the request and never answers, until the client hangs up.
    Stall,
    /// Reads the request and closes the connection.
    Close,
    /// `307 Temporary Redirect` to the URL: a node sending the request on.
    Redirect(String),
    /// `200 OK` with a blob's borsh `Some(framed)` `len` bytes long, sent
    /// for as long as the client reads, its `Content-Length` declared or
    /// the body chunked. What got out goes to [`FakeNode::streamed`].
    Stream { len: usize, declared: bool },
}

/// A node on a local socket whose routes each answer one [`Mode`], one
/// thread per connection; a request line names its route by prefix.
pub(super) struct FakeNode {
    pub(super) node: node::Node,
    /// Connections taken so far.
    pub(super) accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// The bytes each [`Mode::Stream`] got out before the client hung up.
    pub(super) streamed: tokio::sync::mpsc::UnboundedReceiver<usize>,
}

pub(super) fn fake_node(routes: Vec<(&'static str, Mode)>) -> FakeNode {
    use std::sync::atomic::Ordering;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
    };
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (streamed, sent) = tokio::sync::mpsc::unbounded_channel();
    let counted = accepted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            let (routes, streamed) = (routes.clone(), streamed.clone());
            std::thread::spawn(move || serve(stream, &routes, &streamed));
        }
    });
    FakeNode {
        node,
        accepted,
        streamed: sent,
    }
}

/// One connection to a [`FakeNode`]: its route's mode, played out.
fn serve(
    mut stream: std::net::TcpStream,
    routes: &[(&'static str, Mode)],
    streamed: &tokio::sync::mpsc::UnboundedSender<usize>,
) {
    use std::io::{Read as _, Write as _};
    let (line, _) = read_request(&mut stream);
    let path = line.split(' ').nth(1).unwrap_or_default();
    let Some((_, mode)) = routes.iter().find(|(route, _)| path.starts_with(route)) else {
        panic!("no route for {line:?}");
    };
    // what a held connection waits on: the client hanging up
    let hung_up =
        |stream: &mut std::net::TcpStream| while matches!(stream.read(&mut [0; 64]), Ok(1..)) {};
    match mode {
        Mode::Answer(body) => respond(&mut stream, "200 OK", body),
        Mode::After(hold, body) => {
            std::thread::sleep(*hold);
            respond(&mut stream, "200 OK", body);
        }
        Mode::Stall => hung_up(&mut stream),
        Mode::Close => drop(stream),
        Mode::Redirect(to) => write!(
            stream,
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {to}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap(),
        Mode::Stream { len, declared } => {
            let head = match declared {
                true => format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n"),
                false => "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".into(),
            };
            let mut sent = 0;
            let mut body = vec![0; 64 << 10];
            body[0] = 1;
            body[1..5].copy_from_slice(&(*len as u32 - 5).to_le_bytes());
            let mut out = stream.write_all(head.as_bytes());
            while out.is_ok() && sent < *len {
                let chunk = &body[..body.len().min(len - sent)];
                out = match declared {
                    true => stream.write_all(chunk),
                    false => write!(stream, "{:x}\r\n", chunk.len())
                        .and_then(|()| stream.write_all(chunk))
                        .and_then(|()| stream.write_all(b"\r\n")),
                };
                if out.is_ok() {
                    sent += chunk.len();
                }
                body[..5].fill(0);
            }
            if out.is_ok() && !declared {
                let _ = stream.write_all(b"0\r\n\r\n");
            }
            let _ = streamed.send(sent);
        }
    }
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
    let want =
        Capability::of_kind(kind).map_or(0, |(family, _)| usize::from(!declared.contains(&family)));
    assert_eq!(
        guest.undeclared_logged.len(),
        want,
        "{kind} logged {want} time(s)"
    );
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

/// The one `connection()` is the process's: a test that sets it, or
/// counts on it being unset, holds this for its whole run.
fn connection_serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A node method routes to the node handler, which answers for the missing
/// node before it reads the request; chain and invite kinds route there
/// too. What the request says is judged by `query` itself, below. The one
/// exception is the envelope's target, read first (`targeted`): the
/// manifest gate comes before the node, as the capability gate does.
#[test]
fn node_methods_answer_for_the_missing_node_first() {
    let _connection = connection_serial();
    let to_registry = methods::encode(&methods::Call {
        target: "registry".into(),
        body: b"not borsh".to_vec(),
    });
    for (capability, operation, payload) in [
        (Capability::Module, "query", to_registry.as_slice()),
        (Capability::Chain, "status", b"not borsh"),
        (Capability::Invite, "create", b"not borsh"),
    ] {
        let mut guest = guest();
        assert!(answer(&mut guest, capability, operation, 7, payload, &None));
        assert!(
            matches!(guest.pending.pop(), Some(wire::Event::Response {
                id: 7, result: Err(refusal), done: true
            }) if refusal.code == refusal::NOT_CONNECTED),
            "{capability:?}.{operation}"
        );
    }
}
/// `link.open` opens `duck://` and `https://` and refuses every other
/// scheme at the method, before the app is asked.
#[test]
fn open_link_refuses_any_scheme_but_duck_and_https() {
    let open = |link: &str| {
        let mut guest = guest();
        guest.activation = Some(Instant::now());
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

/// `link.open` needs the view's activation and takes it: a view on a
/// clock, or one handed a route by an OS `duck://` link, opens nothing;
/// one press opens one link.
#[test]
fn link_open_needs_an_activation_and_takes_it() {
    let mut guest = guest();
    guest.answer(link_request(1, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("needs_gesture"));
    // a route the OS handed the view (`Windows::open_seat`) activates nothing
    guest.route_subscriptions.push(2);
    crate::runtime::route_to("request-test", "tx/1".into());
    guest.sync_route();
    guest.answer(link_request(3, "duck://request-test/tx/1"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("needs_gesture"));
    assert!(guest.intents.is_empty(), "a link opened with no activation");
    guest.activation = Some(Instant::now());
    guest.answer(link_request(4, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut guest), None);
    guest.answer(link_request(5, "https://example.com/b"), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some("needs_gesture"),
        "a second link on the same press"
    );
    assert_eq!(guest.intents.len(), 1);
}

/// A screen reader's press on a view's link is one gesture: AccessKit's
/// Click (what the AX door's `press` sends) is the link's own click, once,
/// with the one activation the host stamps for it, and the view's
/// `link.open` on that click goes out. No stamp refuses it `needs_gesture`;
/// a second click on the same press asks twice and the second is refused.
#[gpui_kit::test]
fn a_readers_press_on_a_link_opens_it_once(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::Styled as _;
    use gpui_kit::accesskit::{Action, ActionRequest, TreeId};
    use gpui_kit::test::TestWindowExt as _;
    cx.update(gpui_kit::init);
    let link = wire::Node::Container(view_wire::ContainerNode {
        id: Some(wire::ElementIdWire::Name("docs".into())),
        style: crate::render::test_style(gpui_kit::div().size_full().style().clone()),
        interactivity: Box::new(wire::Interactivity {
            role: Some(gpui_kit::Role::Link),
            aria: wire::Aria {
                label: Some("the docs".into()),
                ..Default::default()
            },
            on_click: Some(7),
            ..Default::default()
        }),
        children: Vec::new(),
    });
    let window = cx.open_window(
        gpui_kit::size(gpui_kit::px(200.), gpui_kit::px(40.)),
        |_, _| crate::render::ViewTree::new(link),
    );
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
        window.render_frame(cx);
        window.render_frame(cx);
        let target = window
            .a11y_tree()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("the docs"))
            .map(|(id, _)| *id)
            .expect("the link is in the tree");
        window.dispatch_a11y_action(
            ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: target,
                data: None,
            },
            cx,
        );
    });
    // the seat hands the tree's stamp to the guest at its next turn
    // (`a_trees_activation_reaches_its_guest_on_the_next_turn`), and the
    // view asks to open its link on each click it hears
    let mut guest = guest();
    guest.activation = tree.read_with(&native, |tree, _| tree.take_activation());
    let mut refusals = Vec::new();
    for (id, event) in events.borrow().iter().enumerate() {
        if let wire::Event::Click { handler: 7, .. } = event {
            guest.answer(
                link_request(id as u64 + 1, "https://example.com/docs"),
                &None,
            );
            refusals.push(refusal_code(&mut guest));
        }
    }
    assert_eq!(
        refusals,
        [None],
        "one click, its link admitted: {:?}",
        events.borrow()
    );
    assert_eq!(guest.intents.len(), 1);
}

/// Four links a minute per view, each on its own activation; the fifth is
/// refused `link_limit` until the window moves on.
#[test]
fn link_open_refuses_the_fifth_link_in_a_minute() {
    let mut guest = guest();
    for id in 1..=4 {
        guest.activation = Some(Instant::now());
        guest.answer(link_request(id, "https://example.com/"), &None);
        assert_eq!(refusal_code(&mut guest), None, "link {id}");
    }
    guest.activation = Some(Instant::now());
    guest.answer(link_request(5, "https://example.com/"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("link_limit"));
    assert_eq!(guest.intents.len(), 4);
    // a minute on, the oldest is forgotten and one more goes through
    guest.links[0] -= Duration::from_secs(61);
    guest.activation = Some(Instant::now());
    guest.answer(link_request(6, "https://example.com/"), &None);
    assert_eq!(refusal_code(&mut guest), None);
    assert_eq!(guest.intents.len(), 5);
}

/// One activation admits one gated call, whichever asks first; a stale
/// one (older than `ACTIVATION_EXPIRY`: a press the view sat on) none.
#[test]
fn one_activation_admits_one_gated_call_and_a_stale_one_none() {
    let write = |id: u64| wire::Request {
        id,
        kind: "clipboard.write".into(),
        payload: methods::encode(&"copied".to_owned()),
    };
    let mut guest = guest();
    assert_eq!(
        clipboard_read(&mut guest, 1).as_deref(),
        Some("needs_gesture")
    );
    guest.answer(write(2), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("needs_gesture"));
    guest.activation = Some(Instant::now());
    assert_eq!(clipboard_read(&mut guest, 3), None, "the read is admitted");
    guest.answer(write(4), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some("needs_gesture"),
        "the read took the activation"
    );
    guest.activation = Some(Instant::now());
    guest.answer(write(5), &None);
    assert_eq!(refusal_code(&mut guest), None, "the write is admitted");
    guest.answer(link_request(6, "https://example.com/"), &None);
    assert_eq!(refusal_code(&mut guest).as_deref(), Some("needs_gesture"));
    guest.activation =
        Some(Instant::now() - crate::runtime::guest::ACTIVATION_EXPIRY - Duration::from_secs(1));
    assert_eq!(
        clipboard_read(&mut guest, 7).as_deref(),
        Some("needs_gesture"),
        "a stale activation admitted a read"
    );
    assert!(guest.activation.is_none(), "a stale activation is gone");
}

#[test]
fn host_session_is_program_independent_and_tracks_updates() {
    let mut guest = guest();
    let props = Some(super::super::props(
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
    // the key's account resolves: the same subscription hears it
    let changed = Some(super::super::props(
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

/// The host's facts reach the guest as events, each once until it moves:
/// the reader's UTC offset (a DST change moves it) and the body the view
/// is laid out in. The latest of each wins the tick.
#[test]
fn the_hosts_facts_cross_once_and_again_when_they_move() {
    let mut guest = guest();
    guest.pending.clear();
    let minutes = super::offset_minutes();
    assert!((-14 * 60..=14 * 60).contains(&minutes));
    guest.sync_offset(minutes);
    guest.sync_viewport(766., 501.);
    let told = vec![
        wire::Event::Offset { minutes },
        wire::Event::Viewport {
            width: 766.,
            height: 501.,
        },
    ];
    assert_eq!(guest.pending, told);
    guest.sync_offset(minutes);
    guest.sync_viewport(766., 501.);
    assert_eq!(guest.pending, told, "the same facts are not sent twice");
    guest.sync_offset(minutes + 60);
    guest.sync_viewport(800., 501.);
    assert_eq!(
        guest.pending,
        [
            wire::Event::Offset {
                minutes: minutes + 60
            },
            wire::Event::Viewport {
                width: 800.,
                height: 501.,
            },
        ],
        "a moved fact replaces the one waiting"
    );
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
/// arms under `src/runtime`, are exactly `methods::ALL`:
/// `a_method_is_reached_only_through_its_declared_capability` is the one
/// direction (every method answered), this the other — nothing is served that is not a method.
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

/// A node method reaches only a program the view's manifest lists among
/// its targets: `op.submit`, `module.query` and `module.changes` to any
/// other are refused `undeclared_target` before anything is signed or
/// subscribed; to a listed one they pass the gate (and meet the missing
/// node next).
#[test]
fn a_node_method_reaches_only_a_declared_target() {
    let _connection = connection_serial();
    let call = |target: &str| {
        methods::encode(&methods::Call {
            target: target.into(),
            body: Vec::new(),
        })
    };
    let named = |program: &str| methods::encode(&program.to_owned());
    for (kind, to_identity, to_chat) in [
        ("op.submit", call("identity"), call("chat")),
        ("module.query", call("identity"), call("chat")),
        ("module.changes", named("identity"), named("chat")),
    ] {
        let mut guest = guest();
        guest.targets = vec!["chat".into()];
        let request = |id, payload| wire::Request {
            id,
            kind: kind.into(),
            payload,
        };
        guest.answer(request(1, to_identity), &None);
        assert_eq!(
            refusal_code(&mut guest).as_deref(),
            Some(refusal::UNDECLARED_TARGET),
            "{kind} to an unlisted program"
        );
        guest.answer(request(2, to_chat), &None);
        assert_eq!(
            refusal_code(&mut guest).as_deref(),
            Some(refusal::NOT_CONNECTED),
            "{kind} to a listed program passes the gate"
        );
    }
}

/// The reply a task delivered, waited for; `None` after two seconds.
fn awaited(guest: &mut Guest) -> Option<wire::Event> {
    for _ in 0..200 {
        let mut events = Vec::new();
        guest.replies.drain_into(&mut events).unwrap();
        if let Some(event) = events.pop() {
            return Some(event);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

/// An `op.submit` of an identity op that removes a key waits for the
/// person: nothing is signed or sent until they approve on the console,
/// Cancel refuses it `consent_refused`, a second ask while one waits is
/// refused at once, and with no key seated it is refused `session_locked`
/// before anyone is asked.
#[test]
fn an_identity_key_op_waits_for_the_person_and_cancel_refuses_it() {
    use super::super::consent;
    use commonware_codec::DecodeExt as _;
    let _seat = backend::seat_serial();
    let _connection = connection_serial();
    let remove = methods::encode(&methods::Call {
        target: identity::MODULE.into(),
        body: abi::encode(&identity::Op::RemoveKey {
            account: 7,
            key: vec![1, 2, 3],
        }),
    });
    let request = |id| wire::Request {
        id,
        kind: "op.submit".into(),
        payload: remove.clone(),
    };
    let mut guest = guest();
    guest.targets = vec![identity::MODULE.into()];
    // the card names the program, never the manifest's own word for itself
    guest.name = "Chat".into();
    // no press behind it: refused before anything is asked, even locked
    handle().block_on(backend::lock_signer());
    guest.answer(request(1), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some(refusal::NEEDS_GESTURE)
    );
    // locked: refused before anything is asked
    guest.activation = Some(Instant::now());
    guest.answer(request(1), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some(refusal::SESSION_LOCKED)
    );
    assert!(consent::front().is_none());
    // seated and connected: the ask waits, nothing reaches the node
    let key = commonware_cryptography::ed25519::PrivateKey::decode([7u8; 32].as_slice()).unwrap();
    handle().block_on(backend::seat_key(key));
    let (node, server) = node_methods::node_server(
        "POST /v1/get HTTP/1.1",
        vec![("200 OK", b"not a sequence".to_vec())],
    );
    *super::super::connection().lock().unwrap() = super::super::Connection {
        client: Some(node.client.clone()),
        network: node.network.clone(),
        chain: "test-network#1".into(),
        rev: guest.connection_rev,
    };
    // the props say the seated key holds account 7, the one the op names
    let props = Some(super::super::props(true, "test-network#1", "", Some(7), ""));
    guest.activation = Some(Instant::now());
    guest.answer(request(2), &props);
    assert_eq!(refusal_code(&mut guest), None, "the request waits");
    let (first, words) = consent::front().expect("an ask waits");
    assert!(
        words
            .said
            .starts_with("Program request-test asks to remove a key from your account"),
        "{words:?}"
    );
    assert!(guest.intents.contains(&Intent::Consent));
    // a second ask while the first waits
    guest.activation = Some(Instant::now());
    guest.answer(request(3), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some(refusal::CONSENT_REFUSED)
    );
    // Cancel: refused, and the node was never asked
    assert!(!consent::answer(first, false), "nothing waits behind it");
    match awaited(&mut guest) {
        Some(wire::Event::Response {
            id: 2,
            result: Err(refusal),
            ..
        }) => assert_eq!(refusal.code, refusal::CONSENT_REFUSED),
        other => panic!("{other:?}"),
    }
    // Approve: the task goes on to the node (its sequence read is the
    // fake node's one answer, which is no sequence, so the op ends there)
    // the in-flight budget spent: refused before the card goes up (the ask
    // is the request's own slot, so Approve could lead nowhere)
    let held: Vec<_> = (0..MAX_IN_FLIGHT)
        .map(|_| guest.replies.admit().expect("within budget"))
        .collect();
    guest.activation = Some(Instant::now());
    guest.answer(request(6), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some(refusal::IN_FLIGHT_LIMIT)
    );
    assert!(consent::front().is_none(), "nothing asked");
    drop(held);
    // resubmitted after Cancel with no new press: no second card
    guest.intents.clear();
    guest.answer(request(5), &None);
    assert_eq!(
        refusal_code(&mut guest).as_deref(),
        Some(refusal::NEEDS_GESTURE)
    );
    assert!(consent::front().is_none(), "nothing asked");
    assert!(!guest.intents.contains(&Intent::Consent));
    // with no account resolved the card does not call it the person's own
    guest.activation = Some(Instant::now());
    guest.answer(request(4), &None);
    let (second, words) = consent::front().expect("an ask waits");
    assert_ne!(first, second);
    assert!(
        words
            .said
            .starts_with("Program request-test asks to remove a key from agent #7"),
        "{words:?}"
    );
    assert!(
        words
            .shown
            .as_deref()
            .is_some_and(|shown| shown.starts_with("#7 · ")),
        "{words:?}"
    );
    assert!(!consent::answer(second, true));
    match awaited(&mut guest) {
        Some(wire::Event::Response {
            id: 4,
            result: Err(refusal),
            ..
        }) => assert_ne!(refusal.code, refusal::CONSENT_REFUSED, "{refusal:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        server.join().unwrap().len(),
        1,
        "one sequence read, after Approve"
    );
    // the view torn down with an ask waiting: the ask goes with it (its
    // task, aborted, lets go of the ask on the runtime's next turn)
    guest.activation = Some(Instant::now());
    guest.answer(request(7), &None);
    assert!(consent::front().is_some(), "an ask waits");
    drop(guest);
    let gone = (0..200).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        consent::front().is_none()
    });
    assert!(gone, "the ask is withdrawn with its view");
    *super::super::connection().lock().unwrap() = Default::default();
    handle().block_on(backend::lock_signer());
}
