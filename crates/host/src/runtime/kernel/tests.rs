use super::*;
use std::time::{Duration, Instant};

mod node_methods;

/// A view holding every capability, its code doing nothing.
pub(in crate::runtime) fn view() -> View {
    let code = crate::runtime::view::prepared_for_test(
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0)
            (func (export "snapshot") (result i64) i64.const 0)
            (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#,
    );
    let mut view = View::instantiate(
        "request-test",
        crate::contract::runner(),
        code,
        "request test",
    )
    .unwrap();
    view.capabilities = Capability::ALL.to_vec();
    // on screen, of an app always in front: its node subscriptions follow
    // at once, whatever another test does to the app's own
    view.on_screen.send_replace(true);
    view.in_front = tokio::sync::watch::channel(true).1;
    view
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
    /// `400 Bad Request` with the body: the node's refusal.
    Refuse(Vec<u8>),
}

/// A node on a local socket whose routes each answer one [`Mode`], one
/// thread per connection; a request line names its route by prefix.
pub(super) struct FakeNode {
    pub(super) node: node::Node,
    /// Connections taken so far.
    pub(super) accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

pub(super) fn fake_node(routes: Vec<(&'static str, Mode)>) -> FakeNode {
    use std::sync::atomic::Ordering;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let node = node::Node {
        client: RpcClient::new(format!("http://{}", listener.local_addr().unwrap())),
        network: "test-network".into(),
        apart: None,
    };
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = accepted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            let routes = routes.clone();
            std::thread::spawn(move || serve(stream, &routes));
        }
    });
    FakeNode { node, accepted }
}

/// One connection to a [`FakeNode`]: its route's mode, played out.
fn serve(mut stream: std::net::TcpStream, routes: &[(&'static str, Mode)]) {
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
        Mode::Refuse(body) => respond(&mut stream, "400 Bad Request", body),
        Mode::Redirect(to) => write!(
            stream,
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {to}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap(),
    }
}

/// The reason `kind` is refused for, asked by a view declaring `declared`,
/// the view of the program bound to the programs role (`invite.create` is
/// a role's own view's) while [`acl_roles`] are bound; `None` if it is not
/// refused at once.
fn refused_with(declared: &[Capability], kind: &str) -> Option<String> {
    let mut view = view();
    view.program = "acl-programs";
    view.capabilities = declared.to_vec();
    let payload = methods::encode(&methods::Call {
        target: "programs".into(),
        body: Vec::new(),
    });
    for id in [1, 2] {
        let request = wire::Request {
            id,
            kind: kind.into(),
            payload: payload.clone(),
        };
        view.answer(request, &None);
    }
    let want =
        Capability::of_kind(kind).map_or(0, |(family, _)| usize::from(!declared.contains(&family)));
    assert_eq!(
        view.undeclared_logged.len(),
        want,
        "{kind} logged {want} time(s)"
    );
    refusal_code(&mut view)
}

/// The code the view's last pending event refuses with; `None` if that
/// event is not a refusal.
fn refusal_code(view: &mut View) -> Option<String> {
    match view.pending.pop() {
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
    let _serial = roles_serial();
    let _roles = acl_roles();
    for kind in methods::ALL {
        // answered to every view, whatever it declares
        // (`every_view_asks_what_this_host_answers`)
        if *kind == "host.methods" {
            continue;
        }
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
        // a build without av captures nothing: the camera's and the
        // microphone's methods are unknown requests there
        let media = matches!(family, Capability::Camera | Capability::Microphone);
        if media && !cfg!(feature = "av") {
            assert_eq!(
                refused_with(&[family], kind).as_deref(),
                Some(refusal::UNKNOWN_REQUEST),
                "{kind} without av"
            );
            continue;
        }
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

// `host.methods` says what this host answers, to a view that declares
// nothing: every method but those this build or platform has none of
// (J-085).
#[test]
fn every_view_asks_what_this_host_answers() {
    for floats in [true, false] {
        let mut view = view();
        view.capabilities = Vec::new();
        view.floats = floats;
        let asked = wire::Request {
            id: 1,
            kind: "host.methods".into(),
            payload: methods::encode(&()),
        };
        view.answer(asked, &None);
        let Some(wire::Event::Response {
            result: Ok(bytes),
            done: true,
            ..
        }) = view.pending.pop()
        else {
            panic!("host.methods answers once");
        };
        let hosted: methods::Hosted = methods::decode(&bytes).unwrap();
        assert_eq!(hosted.level, wire::WIRE_LEVEL);
        assert!(hosted.has("link.window") && hosted.has("host.methods"));
        assert_eq!(hosted.has("host.float"), floats);
        assert_eq!(hosted.has("camera.devices"), cfg!(feature = "av"));
        assert!(
            hosted
                .kinds
                .iter()
                .all(|kind| methods::ALL.contains(&kind.as_str()))
        );
    }
}

// A view built before J-085 fails on an event it does not know, so it is
// sent only those its wire level reads; an event this app could not read
// itself is sent to none.
#[test]
fn a_view_is_sent_only_the_events_its_wire_level_reads() {
    let mut view = view();
    view.wire = "cd5911c62028048f".into();
    view.pending = vec![
        wire::Event::Unknown {
            what: "Event: later".into(),
        },
        wire::Event::Theme { dark: true },
    ];
    view.tick();
    assert_eq!(
        view.ticked_with.last().unwrap(),
        &[wire::Event::Theme { dark: true }]
    );
}

#[test]
fn unknown_kinds_finish_with_a_typed_refusal() {
    let mut view = view();
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
        view.answer(
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
        }) = view.pending.pop()
        else {
            panic!("{kind} must answer a refusal");
        };
        assert_eq!(answered, id as u64);
        assert!(done);
        assert_eq!(refusal.code, refusal::UNKNOWN_REQUEST);
        assert!(refusal.message.contains(kind));
    }
    assert!(view.pending.is_empty());
}

/// The roster's role bindings are the process's: a test that binds them
/// (`acl_roles`), or whose requests read them (`invite.create`,
/// `op.submit`), holds this for its whole run. A test that holds the seat
/// too (`backend::seat_serial`) takes the seat first: the other order
/// deadlocks against one that does.
pub(in crate::runtime) fn roles_serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The app's roster bound to roles named `acl-*`, which no other test's
/// view or target takes; unbound again when dropped, a panic too.
struct AclRoles;

impl Drop for AclRoles {
    fn drop(&mut self) {
        super::super::roster().bind(None);
    }
}

fn acl_roles() -> AclRoles {
    super::super::roster().bind(Some(abi::Roles::from_iter([
        (abi::role::PROGRAMS, "acl-programs"),
        (abi::role::VALIDATORS, "acl-valset"),
        (abi::role::IDENTITY, "acl-identity"),
        (abi::role::BLOBS, "acl-blobs"),
        (abi::role::AWAITS, "acl-awaits"),
    ])));
    AclRoles
}

/// `invite.create`, which the node's key signs, is a role's own view's: a
/// view whose program is bound to no role is refused before the node is
/// reached, and so is every view while no bindings are known; a role's
/// view goes on to the node (here, none connected).
#[test]
fn invite_create_is_a_role_views_own() {
    let _serial = roles_serial();
    let mint = methods::encode(&methods::CreateInvite { ttl_days: 7 });
    let refused = |program: &'static str| {
        let mut view = view();
        view.program = program;
        assert!(answer(
            &mut view,
            Capability::Invite,
            "create",
            7,
            &mint,
            &None
        ));
        refusal_code(&mut view)
    };
    let unbound = refused("acl-programs");
    assert_eq!(unbound.as_deref(), Some(refusal::UNDECLARED_CAPABILITY));
    let _roles = acl_roles();
    // a manifest name is the view's own word: the roster's program decides
    let mut named = view();
    named.name = "acl-programs".into();
    assert!(answer(
        &mut named,
        Capability::Invite,
        "create",
        7,
        &mint,
        &None
    ));
    assert_eq!(
        refusal_code(&mut named).as_deref(),
        Some(refusal::UNDECLARED_CAPABILITY)
    );
    assert_eq!(
        refused("acl-programs").as_deref(),
        Some(refusal::NOT_CONNECTED)
    );
}

/// A node method routes to the node handler, which answers for the missing
/// node before it reads the request; chain kinds route there too, and
/// invite's past its role gate (`invite_create_is_a_role_views_own`). What
/// the request says is judged by `query` itself, below. The one exception
/// is the envelope, read first (`enveloped`): one that names no program
/// is refused before the node, as an undeclared capability is.
#[test]
fn node_methods_answer_for_the_missing_node_first() {
    let to_programs = methods::encode(&methods::Call {
        target: "programs".into(),
        body: b"not borsh".to_vec(),
    });
    for (capability, operation, payload) in [
        (Capability::Program, "query", to_programs.as_slice()),
        (Capability::Chain, "status", b"not borsh"),
    ] {
        let mut view = view();
        assert!(answer(&mut view, capability, operation, 7, payload, &None));
        assert!(
            matches!(view.pending.pop(), Some(wire::Event::Response {
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
        let mut view = view();
        view.activation = Some(Instant::now());
        view.answer(
            wire::Request {
                id: 5,
                kind: "link.open".into(),
                payload: methods::encode(&link.to_owned()),
            },
            &None,
        );
        (refusal_code(&mut view), view.intents.len())
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
        // a system settings page is `host.show`'s, never a link
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera",
    ] {
        assert_eq!(
            open(link),
            (Some(refusal::MALFORMED_REQUEST.into()), 0),
            "{link}"
        );
    }
}

/// `host.title` names the view's window: the host keeps no control
/// characters, no bidi embedding, override or isolate, no line or
/// paragraph separator, no space at either end and at most 80 chars, cut
/// on a char boundary, and replies `()`; empty clears the name; a payload
/// that is no string is refused.
#[test]
fn host_title_is_cleaned_and_anything_but_a_string_refused() {
    let title = |payload: Vec<u8>| {
        let mut view = view();
        view.answer(
            wire::Request {
                id: 3,
                kind: "host.title".into(),
                payload,
            },
            &None,
        );
        (view.pending.pop(), std::mem::take(&mut view.intents))
    };
    let named = |raw: &str| match title(methods::encode(&raw.to_owned())) {
        (
            Some(wire::Event::Response {
                id: 3,
                result: Ok(reply),
                done: true,
            }),
            intents,
        ) => {
            assert_eq!(reply, methods::encode(&()), "{raw:?}: replied ()");
            match intents.as_slice() {
                [Intent::Title(title)] => title.clone(),
                other => panic!("{raw:?}: {other:?}"),
            }
        }
        other => panic!("{raw:?}: {other:?}"),
    };
    assert_eq!(named("#general"), "#general");
    assert_eq!(named(" \t#gen\0er\nal\u{7f}\u{9b}\r "), "#general");
    let reordered = "#\u{202a}g\u{202e}e\u{2066}n\u{2069}e\u{2028}r\u{2029}al";
    assert_eq!(named(reordered), "#general");
    let rtl = "#\u{200f}שלום 👩\u{200d}💻";
    assert_eq!(named(rtl), rtl, "a mark and a joiner stay");
    let long = "한".repeat(10_000);
    assert_eq!(named(&long), long, "a long title is kept whole");
    assert_eq!(named(""), "");
    // a length past its bytes; one byte that is no UTF-8
    for payload in [vec![0xff; 4], methods::encode(&vec![0xffu8])] {
        match title(payload) {
            (
                Some(wire::Event::Response {
                    result: Err(refusal),
                    ..
                }),
                intents,
            ) => {
                assert_eq!(refusal.code, refusal::MALFORMED_REQUEST);
                assert!(intents.is_empty(), "a refused title named the window");
            }
            other => panic!("not refused: {other:?}"),
        }
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

fn clipboard_read(view: &mut View, id: u64) -> Option<String> {
    view.answer(
        wire::Request {
            id,
            kind: "clipboard.read".into(),
            payload: Vec::new(),
        },
        &None,
    );
    refusal_code(view)
}

/// `link.open` needs the view's activation and takes it: a view on a
/// clock, or one handed a route by an OS `duck://` link, opens nothing;
/// one press opens one link.
#[test]
fn link_open_needs_an_activation_and_takes_it() {
    let mut view = view();
    view.answer(link_request(1, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    // a route the OS handed the view (`Windows::open_seat`) activates nothing
    view.route_subscriptions.push(2);
    crate::runtime::route_to("request-test", view.instance, "tx/1".into());
    view.sync_route();
    view.answer(link_request(3, "duck://request-test/tx/1"), &None);
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    assert!(view.intents.is_empty(), "a link opened with no activation");
    view.activation = Some(Instant::now());
    view.answer(link_request(4, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut view), None);
    view.answer(link_request(5, "https://example.com/b"), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("needs_gesture"),
        "a second link on the same press"
    );
    assert_eq!(view.intents.len(), 1);
}

/// `host.show` brings up the reader's account on the view's activation,
/// as `link.open` opens a link: none refuses it `needs_gesture`, and one
/// press brings up one place. A reader with no account (the props name
/// none) is refused `unknown_request`; a place this host does not know
/// does not decode.
#[test]
fn host_show_brings_up_the_readers_account_on_a_press() {
    let show = |id, payload: Vec<u8>| wire::Request {
        id,
        kind: "host.show".into(),
        payload,
    };
    let account = methods::encode(&methods::Place::Account);
    let reader = |account| {
        Some(methods::encode(&methods::Session {
            account,
            ..Default::default()
        }))
    };
    let mut view = view();
    view.answer(show(1, account.clone()), &reader(Some(5)));
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    view.activation = Some(Instant::now());
    view.answer(show(2, account.clone()), &reader(None));
    assert_eq!(refusal_code(&mut view).as_deref(), Some("unknown_request"));
    // past the places this wire knows (the account, the two settings pages)
    view.answer(show(3, vec![u8::MAX]), &reader(Some(5)));
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("malformed_request")
    );
    assert!(view.intents.is_empty(), "nothing shown yet");
    view.answer(show(4, account.clone()), &reader(Some(5)));
    assert_eq!(refusal_code(&mut view), None);
    assert_eq!(view.intents, [Intent::Show(methods::Place::Account)]);
    view.answer(show(5, account), &reader(Some(5)));
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("needs_gesture"),
        "a second place on the same press"
    );
}

/// `host.show` brings up a refused camera's or microphone's system
/// settings only where this host has that page (macOS's, with av), and
/// only at a press or key, taken by the ask; where it has none the place
/// is an unknown request.
#[test]
fn a_settings_place_is_shown_at_a_press_where_the_host_has_it() {
    let show = |place: methods::Place, pressed: bool| {
        let mut view = view();
        if pressed {
            view.activation = Some(Instant::now());
        }
        view.answer(
            wire::Request {
                id: 1,
                kind: "host.show".into(),
                payload: methods::encode(&place),
            },
            &None,
        );
        (refusal_code(&mut view), std::mem::take(&mut view.intents))
    };
    for place in [
        methods::Place::CameraSettings,
        methods::Place::MicrophoneSettings,
    ] {
        if crate::render::av::settings_page(place).is_none() {
            let unknown = (Some(refusal::UNKNOWN_REQUEST.into()), Vec::new());
            assert_eq!(show(place, true), unknown, "{place:?}");
            continue;
        }
        let (refused, intents) = show(place, false);
        assert_eq!(
            refused.as_deref(),
            Some(refusal::NEEDS_GESTURE),
            "{place:?}"
        );
        assert!(intents.is_empty());
        let shown = (None, vec![Intent::Show(place)]);
        assert_eq!(show(place, true), shown, "{place:?}");
    }
}

/// `microphone.chime` makes a sound: only at the person's press or key, so
/// a view on a clock never chimes. (A press is taken as `link.open` takes
/// it, above; here it would sound the test machine's speaker.)
#[cfg(feature = "av")]
#[test]
fn the_chime_needs_a_press() {
    let mut view = view();
    view.answer(
        wire::Request {
            id: 1,
            kind: "microphone.chime".into(),
            payload: methods::encode(&None::<String>),
        },
        &None,
    );
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some(refusal::NEEDS_GESTURE)
    );
}

/// The device lists are taken on a blocking thread (each camera's node
/// opened to ask what it is), never on the window's: the request is a
/// task of the view's, answered when the listing is done.
#[cfg(feature = "av")]
#[test]
fn the_device_lists_are_answered_off_the_window_thread() {
    for (id, kind) in [(1, "camera.devices"), (2, "microphone.devices")] {
        let mut view = view();
        view.answer(
            wire::Request {
                id,
                kind: kind.into(),
                payload: Vec::new(),
            },
            &None,
        );
        assert!(view.pending.is_empty(), "{kind} answered on this thread");
        assert!(view.tasks.iter().any(|(task, _)| *task == id), "{kind}");
    }
}

fn float_request(id: u64, float: methods::Float) -> wire::Request {
    wire::Request {
        id,
        kind: "host.float".into(),
        payload: methods::encode(&float),
    }
}

/// The huddle's float: 320 by 200, never under 240 by 150, kept on top.
const FLOAT: methods::Float = methods::Float {
    width: 320,
    height: 200,
    min_width: 240,
    min_height: 150,
    on_top: true,
};

/// The answer `view` was last handed, as a `T`; it panics on a refusal.
fn answered<T: borsh::BorshDeserialize>(view: &mut View) -> T {
    match view.pending.pop() {
        Some(wire::Event::Response {
            result: Ok(bytes), ..
        }) => methods::decode(&bytes).unwrap(),
        other => panic!("no answer: {other:?}"),
    }
}

/// `host.float` raises a window above every other app's: only at the
/// person's press or key, taken by the ask, so a view on a clock never
/// floats and one press floats once. Its reply says what this platform
/// can do: kept on top where it keeps windows above, not on Wayland.
#[test]
fn a_float_needs_a_press_and_says_what_the_platform_can() {
    let mut view = view();
    view.keeps_above = true;
    view.answer(float_request(1, FLOAT), &None);
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    assert!(view.intents.is_empty(), "floated with no press");
    view.activation = Some(Instant::now());
    view.answer(float_request(2, FLOAT), &None);
    let placement: methods::Placement = answered(&mut view);
    assert_eq!(
        placement,
        methods::Placement {
            floating: true,
            on_top: true
        }
    );
    assert_eq!(view.intents, [Intent::Float(FLOAT)]);
    view.answer(float_request(3, FLOAT), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("needs_gesture"),
        "a second float on the same press"
    );
    // the pin off is a float too, and asks nothing of the platform
    view.activation = Some(Instant::now());
    let off = methods::Float {
        on_top: false,
        ..FLOAT
    };
    view.answer(float_request(4, off), &None);
    assert!(!answered::<methods::Placement>(&mut view).on_top);
    // Wayland: the window floats, nothing keeps it on top
    view.keeps_above = false;
    view.activation = Some(Instant::now());
    view.answer(float_request(5, FLOAT), &None);
    assert_eq!(
        answered::<methods::Placement>(&mut view),
        methods::Placement {
            floating: true,
            on_top: false
        }
    );
}

/// A float names a size a window can have: each side at least its
/// minimum, the minimum at least 1. Anything else is refused before it
/// spends the press (a size past the display is the shell's to fit).
#[test]
fn a_float_of_no_size_is_refused() {
    let mut view = view();
    for float in [
        methods::Float {
            min_width: 0,
            ..FLOAT
        },
        methods::Float {
            min_height: 201,
            ..FLOAT
        },
        methods::Float {
            width: 239,
            ..FLOAT
        },
    ] {
        view.activation = Some(Instant::now());
        view.answer(float_request(1, float), &None);
        assert_eq!(
            refusal_code(&mut view).as_deref(),
            Some("malformed_request"),
            "{float:?}"
        );
        assert!(view.activation.is_some(), "the press is kept: {float:?}");
    }
    assert!(view.intents.is_empty());
}

/// `host.unfloat` only takes a window away: no press needed.
#[test]
fn an_unfloat_needs_no_press() {
    let mut view = view();
    view.answer(
        wire::Request {
            id: 1,
            kind: "host.unfloat".into(),
            payload: Vec::new(),
        },
        &None,
    );
    assert_eq!(refusal_code(&mut view), None);
    assert_eq!(view.intents, [Intent::Unfloat]);
}

/// A host that floats no pane (a phone's) knows none of the float
/// methods, a press or not: each is an unknown request, and nothing moves.
#[test]
fn a_host_that_floats_nothing_knows_no_float_method() {
    let mut view = view();
    view.floats = false;
    for (id, kind, payload) in [
        (1, "host.float", methods::encode(&FLOAT)),
        (2, "host.unfloat", Vec::new()),
        (3, "host.placement", Vec::new()),
    ] {
        view.activation = Some(Instant::now());
        view.answer(
            wire::Request {
                id,
                kind: kind.into(),
                payload,
            },
            &None,
        );
        assert_eq!(
            refusal_code(&mut view).as_deref(),
            Some(refusal::UNKNOWN_REQUEST),
            "{kind}"
        );
    }
    assert!(view.intents.is_empty() && view.placement_subscriptions.is_empty());
}

/// `host.placement` tells where the pane is at subscribe, then once per
/// change the seat places, on the redraw after it; the same placement
/// again tells nothing, and a cancelled subscription hears no more.
#[test]
fn the_placement_is_told_at_subscribe_and_on_each_change() {
    let mut view = view();
    let floating = methods::Placement {
        floating: true,
        on_top: true,
    };
    view.answer(
        wire::Request {
            id: 7,
            kind: "host.placement".into(),
            payload: Vec::new(),
        },
        &None,
    );
    assert_eq!(
        answered::<methods::Placement>(&mut view),
        methods::Placement::default()
    );
    let told = |view: &mut View| {
        view.sync_placement();
        let items: Vec<methods::Placement> = std::mem::take(&mut view.pending)
            .into_iter()
            .map(|event| match event {
                wire::Event::Response {
                    id: 7,
                    result: Ok(bytes),
                    done: false,
                } => methods::decode(&bytes).unwrap(),
                other => panic!("not a placement item: {other:?}"),
            })
            .collect();
        items
    };
    view.set_placement(floating);
    assert_eq!(told(&mut view), [floating]);
    view.set_placement(floating);
    assert_eq!(told(&mut view), [], "the same placement again");
    view.placement_subscriptions.clear();
    view.set_placement(methods::Placement::default());
    assert_eq!(told(&mut view), [], "no subscription");
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
        interactivity: Some(Box::new(wire::Interactivity {
            role: Some(gpui_kit::Role::Link),
            aria: wire::Aria {
                label: Some("the docs".into()),
                ..Default::default()
            },
            on_click: Some(7),
            ..Default::default()
        })),
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
    // the seat hands the tree's stamp to the view at its next turn
    // (`a_trees_activation_reaches_its_view_on_the_next_turn`), and the
    // view asks to open its link on each click it hears
    let mut view = view();
    view.activation = tree.read_with(&native, |tree, _| tree.take_activation());
    let mut refusals = Vec::new();
    for (id, event) in events.borrow().iter().enumerate() {
        if let wire::Event::Click { handler: 7, .. } = event {
            view.answer(
                link_request(id as u64 + 1, "https://example.com/docs"),
                &None,
            );
            refusals.push(refusal_code(&mut view));
        }
    }
    assert_eq!(
        refusals,
        [None],
        "one click, its link admitted: {:?}",
        events.borrow()
    );
    assert_eq!(view.intents.len(), 1);
}

#[test]
fn link_window_asks_for_a_window_on_a_press_and_is_refused_without_one() {
    let window = |id, link: &str| wire::Request {
        id,
        kind: "link.window".into(),
        payload: methods::encode(&link.to_owned()),
    };
    let room = "duck://chat/room/7";
    let mut view = view();
    view.answer(window(1, room), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some(refusal::NEEDS_GESTURE)
    );
    assert!(view.intents.is_empty(), "a window opened with no press");
    view.activation = Some(Instant::now());
    view.answer(window(2, room), &None);
    assert_eq!(refusal_code(&mut view), None);
    assert_eq!(view.intents, [Intent::OpenWindow(room.into())]);
    view.answer(window(3, room), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some(refusal::NEEDS_GESTURE),
        "a second window on the same press"
    );
    for link in ["http://example.com", "javascript:alert(1)", "duck://", ""] {
        view.activation = Some(Instant::now());
        view.answer(window(4, link), &None);
        assert_eq!(
            refusal_code(&mut view).as_deref(),
            Some(refusal::MALFORMED_REQUEST),
            "{link}"
        );
    }
    view.activation = Some(Instant::now());
    view.answer(window(5, "https://example.com/a"), &None);
    assert_eq!(refusal_code(&mut view), None);
    assert_eq!(
        view.intents,
        [
            Intent::OpenWindow(room.into()),
            Intent::OpenWindow("https://example.com/a".into())
        ]
    );
}

/// Every link opened on its own activation goes through.
#[test]
fn link_open_admits_every_link_on_its_own_activation() {
    let mut view = view();
    for id in 1..=8 {
        view.activation = Some(Instant::now());
        view.answer(link_request(id, "https://example.com/"), &None);
        assert_eq!(refusal_code(&mut view), None, "link {id}");
    }
    assert_eq!(view.intents.len(), 8);
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
    let mut view = view();
    assert_eq!(
        clipboard_read(&mut view, 1).as_deref(),
        Some("needs_gesture")
    );
    view.answer(write(2), &None);
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    view.activation = Some(Instant::now());
    assert_eq!(clipboard_read(&mut view, 3), None, "the read is admitted");
    view.answer(write(4), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("needs_gesture"),
        "the read took the activation"
    );
    view.activation = Some(Instant::now());
    view.answer(write(5), &None);
    assert_eq!(refusal_code(&mut view), None, "the write is admitted");
    view.answer(link_request(6, "https://example.com/"), &None);
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    view.activation =
        Some(Instant::now() - crate::runtime::view::ACTIVATION_EXPIRY - Duration::from_secs(1));
    assert_eq!(
        clipboard_read(&mut view, 7).as_deref(),
        Some("needs_gesture"),
        "a stale activation admitted a read"
    );
    assert!(view.activation.is_none(), "a stale activation is gone");
}

/// `file.pick` opens the system's picker only on a press or key, and
/// takes the activation: a view on a clock picks nothing.
#[test]
fn file_pick_needs_an_activation_and_takes_it() {
    let pick = |id| wire::Request {
        id,
        kind: "file.pick".into(),
        payload: methods::encode(&methods::Pick { multiple: true }),
    };
    let mut view = view();
    view.answer(pick(1), &None);
    assert_eq!(refusal_code(&mut view).as_deref(), Some("needs_gesture"));
    view.activation = Some(Instant::now());
    view.answer(pick(2), &None);
    assert_eq!(
        refusal_code(&mut view),
        None,
        "the pick waits for the picker"
    );
    view.answer(pick(3), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("needs_gesture"),
        "the first pick took the activation"
    );
    view.answer(
        wire::Request {
            id: 4,
            kind: "file.pick".into(),
            payload: vec![9],
        },
        &None,
    );
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("malformed_request")
    );
    // a file is put only by a handle of the latest pick
    view.answer(
        wire::Request {
            id: 5,
            kind: "op.put".into(),
            payload: methods::encode(&methods::PutBlobs {
                blobs: vec![methods::NewBlob {
                    kind: "chunk".into(),
                    body: methods::BlobBody::Picked {
                        file: 1,
                        offset: 0,
                        len: 1,
                    },
                }],
            }),
        },
        &None,
    );
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some("malformed_request")
    );
}

#[test]
fn file_read_answers_a_range_of_a_held_file_off_the_window_thread() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("face.jpg");
    std::fs::write(&path, b"portrait").unwrap();
    let mut view = view();
    let file = view.picker.pick(vec![path])[0].file;
    let read = |id, file, offset, len| wire::Request {
        id,
        kind: "file.read".into(),
        payload: methods::encode(&methods::ReadFile { file, offset, len }),
    };
    let refused = [(1, file + 1, 0, 1), (2, file, 4, 5), (3, file, u64::MAX, 2)];
    for (id, file, offset, len) in refused {
        view.answer(read(id, file, offset, len), &None);
        let code = refusal_code(&mut view);
        assert_eq!(code.as_deref(), Some("malformed_request"), "{id}");
    }
    let mut changed = view.replies.changes();
    view.answer(read(4, file, 2, 4), &None);
    assert!(view.pending.is_empty(), "answered on the window thread");
    let mut answered = Vec::new();
    while answered.is_empty() {
        handle().block_on(changed.changed()).unwrap();
        view.replies.drain_into(&mut answered);
    }
    let [
        wire::Event::Response {
            id: 4,
            result: Ok(bytes),
            done: true,
        },
    ] = &answered[..]
    else {
        panic!("{answered:?}");
    };
    assert_eq!(methods::decode::<Vec<u8>>(bytes).unwrap(), b"rtra");
}

#[test]
fn host_session_is_program_independent_and_tracks_updates() {
    let mut view = view();
    let props = Some(super::super::props(
        true,
        "test-network",
        "abcd",
        None,
        "http://127.0.0.1:19001",
    ));
    view.answer(
        wire::Request {
            id: 22,
            kind: "host.session".into(),
            payload: Vec::new(),
        },
        &props,
    );
    assert_eq!(view.props_subscription, Some(22));
    let Some(wire::Event::Response {
        result: Ok(bytes),
        done: false,
        ..
    }) = view.pending.pop()
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
    view.sync_props(&changed);
    let Some(wire::Event::Response {
        id: 22,
        result: Ok(bytes),
        done: false,
    }) = view.pending.pop()
    else {
        panic!("a changed session is pushed")
    };
    let decoded: methods::Session = methods::decode(&bytes).unwrap();
    assert_eq!(decoded.account, Some(7));
}

/// The host's facts reach the view as events, each once until it moves:
/// the reader's UTC offset (a DST change moves it), the body the view is
/// laid out in, and whether the device's pointer is a finger. The latest
/// of each wins the tick.
#[test]
fn the_hosts_facts_cross_once_and_again_when_they_move() {
    let mut view = view();
    view.pending.clear();
    let minutes = super::offset_minutes();
    assert!((-14 * 60..=14 * 60).contains(&minutes));
    view.sync_offset(minutes);
    view.sync_viewport(766., 501.);
    view.sync_pointer(false);
    let told = vec![
        wire::Event::Offset { minutes },
        wire::Event::Viewport {
            width: 766.,
            height: 501.,
        },
        wire::Event::Pointer { touch: false },
    ];
    assert_eq!(view.pending, told);
    view.sync_offset(minutes);
    view.sync_viewport(766., 501.);
    view.sync_pointer(false);
    assert_eq!(view.pending, told, "the same facts are not sent twice");
    view.sync_offset(minutes + 60);
    view.sync_viewport(800., 501.);
    view.sync_pointer(true);
    assert_eq!(
        view.pending,
        [
            wire::Event::Offset {
                minutes: minutes + 60
            },
            wire::Event::Viewport {
                width: 800.,
                height: 501.,
            },
            wire::Event::Pointer { touch: true },
        ],
        "a moved fact replaces the one waiting"
    );
}

/// `host.id` replies a borsh `String` like every other method
/// (`HostId::decode_reply` is `methods::decode::<String>`): raw UTF-8 with
/// no length prefix builds fine here and fails only in the view's decode.
#[test]
fn host_id_answers_a_borsh_string_a_view_can_decode() {
    let mut view = view();
    view.answer(
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
    }) = view.pending.pop()
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

/// A view's node methods reach any program, whatever its manifest says:
/// `op.submit`, `program.query` and `program.changes` naming a program no
/// manifest lists pass on to the node (here none, so `not_connected`),
/// and one that names no program is malformed.
#[test]
fn a_node_method_reaches_any_program() {
    let _serial = roles_serial();
    let call = |target: &str| {
        methods::encode(&methods::Call {
            target: target.into(),
            body: Vec::new(),
        })
    };
    let named = |program: &str| methods::encode(&program.to_owned());
    for (kind, to_any, to_none) in [
        ("op.submit", call("anything"), call("")),
        ("program.query", call("anything"), call("")),
        ("program.changes", named("anything"), named("")),
    ] {
        let mut view = view();
        let request = |id, payload| wire::Request {
            id,
            kind: kind.into(),
            payload,
        };
        view.answer(request(1, to_any), &None);
        assert_eq!(
            refusal_code(&mut view).as_deref(),
            Some(refusal::NOT_CONNECTED),
            "{kind} to a program no manifest names reaches the node"
        );
        view.answer(request(2, to_none), &None);
        assert_eq!(
            refusal_code(&mut view).as_deref(),
            Some(refusal::MALFORMED_REQUEST),
            "{kind} naming no program"
        );
    }
}

/// The reply a task delivered, waited for; `None` after two seconds.
fn awaited(view: &mut View) -> Option<wire::Event> {
    for _ in 0..200 {
        let mut events = Vec::new();
        view.replies.drain_into(&mut events);
        if let Some(event) = events.pop() {
            return Some(event);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

/// The front ask, waited for (the request's task queues it); `None` after
/// two seconds.
fn asked() -> Option<(u64, super::super::consent::Words)> {
    (0..200).find_map(|_| {
        let front = super::super::consent::front();
        if front.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        front
    })
}

/// An `op.submit` of an identity op that removes a key waits for the
/// person: nothing is signed or sent until they approve on the console,
/// Cancel refuses it `consent_refused`, a second ask while one waits is
/// refused, and with no key seated it is refused `session_locked`
/// before anyone is asked.
#[test]
fn an_identity_key_op_waits_for_the_person_and_cancel_refuses_it() {
    use super::super::consent;
    use commonware_codec::DecodeExt as _;
    let _seat = backend::seat_serial();
    let _serial = roles_serial();
    let remove = methods::encode(&methods::Call {
        target: identity::PROGRAM.into(),
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
    let mut view = view();
    // the card names the program, never the manifest's own word for itself
    view.name = "Chat".into();
    // no press behind it: refused before anything is asked, even locked
    handle().block_on(backend::lock_signer());
    view.answer(request(1), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some(refusal::NEEDS_GESTURE)
    );
    // locked: refused before anything is asked
    view.activation = Some(Instant::now());
    view.answer(request(1), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some(refusal::SESSION_LOCKED)
    );
    assert!(consent::front().is_none());
    // seated and connected: the ask waits, nothing reaches the node
    let key = commonware_cryptography::ed25519::PrivateKey::decode([7u8; 32].as_slice()).unwrap();
    handle().block_on(backend::seat_key(std::sync::Arc::new(key)));
    // garbage for the frame's anchor read, on this view's own connection
    let node = fake_node(vec![(
        noded_client::wire::route::STATUS,
        Mode::Answer(b"not a status".to_vec()),
    )]);
    view.connection = super::super::Connection {
        client: Some(node.node.client.clone()),
        network: node.node.network.clone(),
        chain: "test-network#1".into(),
        rev: view.connection.rev,
        live: None,
        apart: None,
    };
    // the props say the seated key holds account 7, the one the op names
    let props = Some(super::super::props(true, "test-network#1", "", Some(7), ""));
    view.activation = Some(Instant::now());
    view.answer(request(2), &props);
    assert_eq!(refusal_code(&mut view), None, "the request waits");
    let (first, words) = asked().expect("an ask waits");
    assert!(
        words
            .said
            .starts_with("Program request-test asks to remove a key from your account"),
        "{words:?}"
    );
    // a second ask while the first waits
    view.activation = Some(Instant::now());
    view.answer(request(3), &None);
    match awaited(&mut view) {
        Some(wire::Event::Response {
            id: 3,
            result: Err(refusal),
            ..
        }) => assert_eq!(refusal.code, refusal::CONSENT_REFUSED),
        other => panic!("{other:?}"),
    }
    // Cancel: refused, and the node was never asked
    assert!(!consent::answer(first, false), "nothing waits behind it");
    match awaited(&mut view) {
        Some(wire::Event::Response {
            id: 2,
            result: Err(refusal),
            ..
        }) => assert_eq!(refusal.code, refusal::CONSENT_REFUSED),
        other => panic!("{other:?}"),
    }
    // resubmitted after Cancel with no new press: no second card
    view.answer(request(5), &None);
    assert_eq!(
        refusal_code(&mut view).as_deref(),
        Some(refusal::NEEDS_GESTURE)
    );
    assert!(consent::front().is_none(), "nothing asked");
    // with no account resolved the card does not call it the person's own
    view.activation = Some(Instant::now());
    view.answer(request(4), &None);
    let (second, words) = asked().expect("an ask waits");
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
    match awaited(&mut view) {
        Some(wire::Event::Response {
            id: 4,
            result: Err(refusal),
            ..
        // the node's garbage status, read: asked after Approve
        }) => assert_eq!(refusal.code, "unexpected_reply", "{refusal:?}"),
        other => panic!("{other:?}"),
    }
    // the view torn down with an ask waiting: the ask goes with it (its
    // task, aborted, lets go of the ask on the runtime's next turn)
    view.activation = Some(Instant::now());
    view.answer(request(7), &None);
    assert!(asked().is_some(), "an ask waits");
    drop(view);
    let gone = (0..200).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        consent::front().is_none()
    });
    assert!(gone, "the ask is withdrawn with its view");
    handle().block_on(backend::lock_signer());
}

/// A view whose program is bound to no role asks the person before its op
/// to a role's program, whatever its manifest calls it, and the card goes
/// up only once the target has described the op: in the validators'
/// program's own words, or as the op's size where the programs role's code
/// carries no describe module. Cancel refuses it and the node is never
/// asked; Approve, and only then, sends it on (the frame's anchor read,
/// which this node, the view's own connection, answers with garbage).
#[test]
fn a_views_op_to_a_role_waits_for_the_targets_own_words() {
    use super::super::consent;
    use super::describe::tests::{HI, list_program};
    use commonware_codec::DecodeExt as _;
    let _seat = backend::seat_serial();
    let _serial = roles_serial();
    let _roles = acl_roles();
    list_program("acl-valset", 0xa1, false, Some(HI));
    // listed, its code carrying no describe module
    list_program("acl-programs", 0xa2, false, None);
    let key = commonware_cryptography::ed25519::PrivateKey::decode([7u8; 32].as_slice()).unwrap();
    handle().block_on(backend::seat_key(std::sync::Arc::new(key)));
    let mut view = view();
    // a manifest name that reads as a role's: the card asks all the same,
    // naming the roster's program
    view.name = "acl-valset".into();
    let node = fake_node(vec![(
        noded_client::wire::route::STATUS,
        Mode::Answer(b"not a status".to_vec()),
    )]);
    view.connection = super::super::Connection {
        client: Some(node.node.client.clone()),
        network: node.node.network.clone(),
        chain: "test-network#1".into(),
        rev: view.connection.rev,
        live: None,
        apart: None,
    };
    let submit = |view: &mut View, id: u64, target: &str| {
        view.activation = Some(Instant::now());
        view.answer(
            wire::Request {
                id,
                kind: "op.submit".into(),
                payload: methods::encode(&methods::Call {
                    target: target.into(),
                    body: vec![1, 2, 3],
                }),
            },
            &None,
        );
        assert_eq!(refusal_code(view), None, "the request waits");
    };
    for (id, target, title, what, shown) in [
        (
            1,
            "acl-valset",
            consent::NETWORK,
            "vote in the validators' program",
            "hi",
        ),
        (
            2,
            "acl-programs",
            consent::NETWORK,
            "sign an op on the programs role",
            "3 bytes this app cannot read",
        ),
    ] {
        submit(&mut view, id, target);
        let (ask, words) = asked().expect("an ask waits");
        assert_eq!(words.title, title);
        assert_eq!(
            words.said,
            format!("Program request-test asks your key to {what}. Approve only if you meant to.")
        );
        assert_eq!(words.shown.as_deref(), Some(shown));
        assert!(!consent::answer(ask, false));
        match awaited(&mut view) {
            Some(wire::Event::Response {
                id: answered,
                result: Err(refusal),
                ..
            }) => assert_eq!(
                (answered, refusal.code.as_str()),
                (id, refusal::CONSENT_REFUSED)
            ),
            other => panic!("{other:?}"),
        }
    }
    // Approve: the op goes on to be signed, its frame's anchor read first
    submit(&mut view, 3, "acl-valset");
    let (ask, _) = asked().expect("an ask waits");
    assert!(!consent::answer(ask, true));
    match awaited(&mut view) {
        Some(wire::Event::Response {
            id: 3,
            result: Err(refusal),
            ..
        // the node's garbage status, read: the node was asked
        }) => assert_eq!(refusal.code, "unexpected_reply", "{refusal:?}"),
        other => panic!("{other:?}"),
    }
    handle().block_on(backend::lock_signer());
}

#[test]
fn what_a_view_shows_goes_to_its_pane() {
    let mut view = view();
    view.answer(
        wire::Request {
            id: 1,
            kind: "host.showing".into(),
            payload: methods::encode(&"general".to_owned()),
        },
        &None,
    );
    assert_eq!(refusal_code(&mut view), None);
    assert_eq!(view.intents, [Intent::Showing("general".into())]);
}
