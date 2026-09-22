use super::*;

/// `DUCKTAPE_AX_DOOR`: unset or empty is no door; a port (0 picks one) is a
/// door; anything else — an address included — is refused.
fn door_port(value: Option<&str>) -> Result<Option<u16>, String> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(port) => port.parse().map(Some).map_err(|_| {
            format!("DUCKTAPE_AX_DOOR={port}: a port number (0 picks one); the door binds 127.0.0.1 only")
        }),
    }
}

/// The only listener the door opens: 127.0.0.1, on `port`.
fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port))
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct DoorFile {
    port: u16,
    token: String,
}

/// `$XDG_RUNTIME_DIR/ducktape/ax-door.json`, else the app's state directory.
fn door_file() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("ducktape").join("ax-door.json")),
        None => Ok(crate::backend::state_dir()?.join("ax-door.json")),
    }
}

/// Writes the door's port and token, readable by this user only.
fn write_door_file(path: &Path, door: &DoorFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(serde_json::to_string(door).unwrap_or_default().as_bytes())
}

/// Opens the door when `DUCKTAPE_AX_DOOR` asks for it; the calls it takes
/// arrive on the returned channel for [`serve`].
pub(crate) fn open() -> Option<futures::channel::mpsc::UnboundedReceiver<Call>> {
    open_env(
        std::env::var("DUCKTAPE_AX_DOOR").ok().as_deref(),
        std::env::var("DUCKTAPE_AX_DOOR_PRIVATE").ok().as_deref(),
    )
}

/// `private`: `DUCKTAPE_AX_DOOR_PRIVATE`; exactly `1` adds [`reveal`].
fn open_env(
    value: Option<&str>,
    private: Option<&str>,
) -> Option<futures::channel::mpsc::UnboundedReceiver<Call>> {
    let private = private.map(str::trim) == Some("1");
    let port = match door_port(value) {
        Ok(port) => port?,
        Err(error) => {
            tracing::warn!(target: "ducktape::app", reason = "ax_door_refused", %error, "the test door stays shut");
            return None;
        }
    };
    let opened = bind(port).and_then(|listener| Ok((listener.local_addr()?.port(), listener)));
    let (port, listener) = opened
        .inspect_err(|error| tracing::warn!(target: "ducktape::app", reason = "ax_door_unbound", %error, "the test door stays shut"))
        .ok()?;
    let door = DoorFile {
        port,
        token: format!("{:032x}", rand::random::<u128>()),
    };
    let written = door_file()
        .and_then(|path| write_door_file(&path, &door).map_err(|error| error.to_string()));
    if let Err(error) = written {
        tracing::warn!(target: "ducktape::app", reason = "ax_door_file_unwritten", %error, "the test door stays shut");
        return None;
    }
    let (sender, calls) = futures::channel::mpsc::unbounded();
    let token = door.token;
    std::thread::Builder::new()
        .name("ax-door".into())
        .spawn(move || {
            accept(listener, &token, private, |request| {
                let (reply, answer) = std::sync::mpsc::channel();
                sender.unbounded_send((request, reply)).ok()?;
                answer.recv().ok()
            })
        })
        .ok()?;
    tracing::info!(target: "ducktape::app", port, "ax_door_open");
    if private {
        tracing::info!(target: "ducktape::app", "ax_door_private=on");
    }
    Some(calls)
}

/// The door's HTTP/1.1 loop: one request per connection, answered in turn;
/// `/reveal` exists only when `private`.
/// ponytail: one at a time, so a long `wait` holds the next caller; the one
/// consumer is a sequential runner.
fn accept(
    listener: TcpListener,
    token: &str,
    private: bool,
    answer: impl Fn(Request) -> Option<Reply>,
) {
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let reply = match read_request(&stream) {
            Err(_) => Reply::new(400, json!({ "error": "not an HTTP/1.1 request" })),
            Ok((_, _, auth, _)) if !same(auth.as_deref().unwrap_or_default(), token) => {
                Reply::new(401, json!({ "error": "the door's token is required" }))
            }
            Ok((method, target, _, body)) => match route(&method, &target, &body, private) {
                Ok(request) => answer(request)
                    .unwrap_or_else(|| Reply::new(503, json!({ "error": "the app is closing" }))),
                Err(reply) => reply,
            },
        };
        let reason = match reply.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            408 => "Request Timeout",
            _ => "Service Unavailable",
        };
        let revision = reply.revision.map_or(String::new(), |revision| {
            format!("X-Ax-Revision: {revision}\r\n")
        });
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{revision}Connection: close\r\n\r\n{}",
            reply.status,
            reply.body.len(),
            reply.body
        );
    }
}

/// A token comparison that takes the same time wherever the first
/// difference is.
fn same(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Method, target, bearer token and body.
fn read_request(stream: &TcpStream) -> std::io::Result<(String, String, Option<String>, Vec<u8>)> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let (mut auth, mut length) = (None, 0usize);
    loop {
        line.clear();
        reader.read_line(&mut line)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("authorization") {
                auth = value.strip_prefix("Bearer ").map(str::to_owned);
            } else if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().map_err(|_| invalid())?;
            }
        }
    }
    if length > 1 << 20 {
        return Err(invalid());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok((method, target, auth, body))
}

fn route(method: &str, target: &str, body: &[u8], private: bool) -> Result<Request, Reply> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params: HashMap<&str, &str> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .collect();
    let filter = Filter {
        window: params.get("window").map(|value| value.to_string()),
        view: params.get("view").map(|value| value.to_string()),
    };
    let flag = |name| params.get(name) == Some(&"1");
    match (method, path.trim_start_matches('/')) {
        ("GET", "tree") => Ok(Request::Tree {
            filter,
            compact: flag("compact"),
            bounds: flag("bounds"),
        }),
        ("GET", "actions") => Ok(Request::Actions(filter)),
        ("POST", "act") => parse(body).map(Request::Act),
        ("POST", "wait") => parse(body).map(Request::Wait),
        ("POST", "reveal") if private => parse(body).map(Request::Reveal),
        ("POST", "key") => parse(body).map(Request::Key),
        ("GET", "keys") => Ok(Request::Keys(filter)),
        ("POST", "drag") => parse(body).map(Request::Drag),
        _ => {
            let mut endpoints = vec![
                "GET /tree",
                "GET /actions",
                "POST /act",
                "POST /wait",
                "POST /key",
                "GET /keys",
                "POST /drag",
            ];
            if private {
                endpoints.push("POST /reveal");
            }
            Err(Reply::new(
                404,
                json!({ "error": "no such endpoint", "endpoints": endpoints }),
            ))
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(body)
        .map_err(|error| Reply::new(400, json!({ "error": error.to_string() })))
}

/// One call to the door: its status and body.
fn call(door: &DoorFile, method: &str, target: &str, body: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, door.port))?;
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        door.token,
        body.len()
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    let body = response.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    Ok((status, body.to_owned()))
}

const USAGE: &str = "usage: ducktape-app ax tree [--window W] [--view V] [--compact] [--bounds]
       ducktape-app ax actions [--window W] [--view V]
       ducktape-app ax act <id> <press|focus|set_value|type|scroll_into_view> [value]
       ducktape-app ax key <keys> [--text T] [--window W]   (keys: tab, shift-tab, enter, ctrl-k …)
       ducktape-app ax keys [--window W]
       ducktape-app ax drag <x1,y1> <x2,y2> [--id ID] [--steps N] [--window W]   (px; local to ID's bounds when given)
       ducktape-app ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]
       ducktape-app ax reveal <id>   (only with DUCKTAPE_AX_DOOR_PRIVATE=1)";

/// `x,y` as the CLI takes a position.
fn point(word: &str) -> Option<[f32; 2]> {
    let (x, y) = word.split_once(',')?;
    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
}

/// `ducktape-app ax …`: prints the door's JSON. Exit 0 answered, 1 not
/// found, refused or timed out, 2 the door is not open.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut words = Vec::new();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.strip_prefix("--") {
            Some(name @ ("compact" | "bounds" | "gone")) => {
                flags.insert(name, "1");
            }
            Some(name) => {
                flags.insert(name, rest.next().map_or("", String::as_str));
            }
            None => words.push(arg.as_str()),
        }
    }
    let query = |names: &[&str]| {
        names
            .iter()
            .filter_map(|name| flags.get(name).map(|value| format!("{name}={value}")))
            .collect::<Vec<_>>()
            .join("&")
    };
    let (method, target, body) = match (args.first().map(String::as_str), &words[..]) {
        (Some("tree"), []) => ("GET", format!("/tree?{}", query(&["window", "view", "compact", "bounds"])), String::new()),
        (Some("actions"), []) => ("GET", format!("/actions?{}", query(&["window", "view"])), String::new()),
        (Some("act"), [id, action, value @ ..]) if value.len() <= 1 => (
            "POST",
            "/act".to_owned(),
            json!({ "id": id, "action": action, "value": value.first() }).to_string(),
        ),
        (Some("reveal"), [id]) => ("POST", "/reveal".to_owned(), json!({ "id": id }).to_string()),
        (Some("key"), keys) if keys.len() <= 1 => (
            "POST",
            "/key".to_owned(),
            json!({ "keys": keys.first().copied().unwrap_or_default(), "text": flags.get("text").copied().unwrap_or_default(), "window": flags.get("window") }).to_string(),
        ),
        (Some("keys"), []) => ("GET", format!("/keys?{}", query(&["window"])), String::new()),
        (Some("drag"), [from, to]) if point(from).is_some() && point(to).is_some() => (
            "POST",
            "/drag".to_owned(),
            json!({
                "id": flags.get("id"),
                "from": point(from),
                "to": point(to),
                "steps": flags.get("steps").and_then(|steps| steps.parse::<u32>().ok()),
                "window": flags.get("window"),
            })
            .to_string(),
        ),
        (Some("wait"), []) => (
            "POST",
            "/wait".to_owned(),
            json!({
                "role": flags.get("role"),
                "name": flags.get("name"),
                "state": flags.get("state"),
                "in": flags.get("in"),
                "gone": flags.contains_key("gone"),
                "deadline_ms": flags.get("deadline-ms").and_then(|ms| ms.parse::<u64>().ok()).unwrap_or(5000),
            })
            .to_string(),
        ),
        _ => {
            eprintln!("{USAGE}");
            return 1;
        }
    };
    let door = door_file().and_then(|path| {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_str::<DoorFile>(&text).map_err(|error| error.to_string())
    });
    let shut = "the door is not open: launch the app with DUCKTAPE_AX_DOOR=<port|0>";
    let door = match door {
        Ok(door) => door,
        Err(error) => {
            eprintln!("ax: {shut} ({error})");
            return 2;
        }
    };
    match call(&door, method, &target, &body) {
        Ok((status, body)) => {
            println!("{body}");
            i32::from(status != 200)
        }
        Err(error) => {
            eprintln!("ax: {shut} (127.0.0.1:{}: {error})", door.port);
            2
        }
    }
}
