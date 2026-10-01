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
            Some("undeclared_capability"),
            "{kind} undeclared"
        );
        assert!(
            !matches!(
                refused_with(&[family], kind).as_deref(),
                Some("undeclared_capability" | "unknown_request")
            ),
            "{kind} declared"
        );
    }
    // an unknown kind is still unknown, declared or not
    assert_eq!(
        refused_with(&[], "chat.props").as_deref(),
        Some("unknown_request")
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
        assert_eq!(refusal.code, "unknown_request");
        assert!(refusal.message.contains(kind));
    }
    assert!(guest.pending.is_empty());
}

/// A node method routes to the node handler, which answers for the missing
/// node before it reads the request; chain and invite kinds route there
/// too. What the request says is judged by `query` itself, below.
#[test]
fn node_methods_answer_for_the_missing_node_first() {
    for (capability, operation) in [
        (Capability::Module, "query"),
        (Capability::Chain, "status"),
        (Capability::Invite, "create"),
    ] {
        let mut guest = guest();
        assert!(answer(&mut guest, capability, operation, 7, b"not borsh"));
        assert!(
            matches!(guest.pending.pop(), Some(wire::Event::Response {
                id: 7, result: Err(refusal), done: true
            }) if refusal.code == "not_connected"),
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
        assert_eq!(open(link), (Some("malformed_request".into()), 0), "{link}");
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
