//! The door's transport and its client. The server side: `DUCKTAPE_AX_DOOR`
//! parsed, 127.0.0.1 bound, the `{port, token}` door file written 0600, a
//! hand-written HTTP/1.1 loop on the `ax-door` thread that checks the bearer
//! token and routes each request to [`Request`] for [`serve`]. The client
//! side: `ducktape-app ax …` ([`cli`]) reads the same door file and prints
//! the door's JSON.
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

/// `$XDG_RUNTIME_DIR/ducktape/ax-door.json`, else the app's cache directory:
/// never the state directory, which is `~/Library/Logs` on macOS, where
/// support bundles collect.
fn door_file() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("ducktape").join("ax-door.json")),
        None => Ok(crate::backend::cache_dir()?.join("ax-door.json")),
    }
}

/// The door file as this app wrote it. Dropped (the app quitting), the file
/// goes, unless another app has written its own door there since.
pub(crate) struct Written {
    path: PathBuf,
    text: String,
}

impl Drop for Written {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|text| text == self.text) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Writes the door's port and token, readable by this user only.
fn write_door_file(path: PathBuf, door: &DoorFile) -> std::io::Result<Written> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let text = serde_json::to_string(door).unwrap_or_default();
    file.write_all(text.as_bytes())?;
    Ok(Written { path, text })
}

/// Opens the door when `DUCKTAPE_AX_DOOR` asks for it; the calls it takes
/// arrive on the returned channel for [`serve`], and the door file stays
/// while the [`Written`] does.
pub(crate) fn open() -> Option<(futures::channel::mpsc::UnboundedReceiver<Call>, Written)> {
    open_env(
        std::env::var("DUCKTAPE_AX_DOOR").ok().as_deref(),
        std::env::var("DUCKTAPE_AX_DOOR_PRIVATE").ok().as_deref(),
        door_file(),
    )
}

/// `private`: `DUCKTAPE_AX_DOOR_PRIVATE`; exactly `1` adds [`reveal`].
/// `file`: where the door file goes ([`door_file`]).
fn open_env(
    value: Option<&str>,
    private: Option<&str>,
    file: Result<PathBuf, String>,
) -> Option<(futures::channel::mpsc::UnboundedReceiver<Call>, Written)> {
    let private = private.map(str::trim) == Some("1");
    let port = match door_port(value) {
        Ok(port) => port?,
        Err(error) => {
            tracing::warn!(target: "ducktape::app", reason = "ax_door_refused", %error, "the test door stays shut");
            return None;
        }
    };
    let opened = bind(port).and_then(|listener| Ok((listener.local_addr()?.port(), listener)));
    let (port, listener) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            // a door file left by a run before names a door that is not
            // this app's: a client must not take its token to that port
            if let Ok(path) = &file {
                let _ = std::fs::remove_file(path);
            }
            tracing::warn!(target: "ducktape::app", reason = "ax_door_unbound", %error, "the test door stays shut");
            return None;
        }
    };
    let door = DoorFile {
        port,
        token: format!("{:032x}", rand::random::<u128>()),
    };
    let (sender, calls) = futures::channel::mpsc::unbounded();
    let token = door.token.clone();
    std::thread::Builder::new()
        .name("ax-door".into())
        .spawn(move || {
            accept(listener, &token, private, REQUEST_DEADLINE, |request| {
                let (reply, answer) = std::sync::mpsc::channel();
                sender.unbounded_send((request, reply)).ok()?;
                answer.recv().ok()
            })
        })
        .ok()?;
    // written once the thread is up, so the file never names a door no one
    // answers; on a failure here the thread stays, and no client can learn
    // its token
    let written =
        file.and_then(|path| write_door_file(path, &door).map_err(|error| error.to_string()));
    let written = match written {
        Ok(written) => written,
        Err(error) => {
            tracing::warn!(target: "ducktape::app", reason = "ax_door_file_unwritten", %error, "the test door stays shut");
            return None;
        }
    };
    tracing::info!(target: "ducktape::app", port, "ax_door_open");
    if private {
        tracing::info!(target: "ducktape::app", "ax_door_private=on");
    }
    Some((calls, written))
}

/// Most bytes of a request line and its headers: all a caller without the
/// token gets the door to read.
const HEAD_MAX: u64 = 16 << 10;

/// Most header lines in one request.
const HEADERS_MAX: usize = 64;

/// Most bytes of a request body.
const BODY_MAX: usize = 1 << 20;

/// Longest a connection may take to send its whole request, however slowly
/// it trickles in; and longest one write of the reply waits on its reader.
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// A connection read against one deadline for the whole request, where the
/// socket's own timeout starts again on every read.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl std::io::Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

/// The door's HTTP/1.1 loop: one request per connection, answered in turn;
/// `/reveal` exists only when `private`. A connection has `patience` to send
/// its request ([`REQUEST_DEADLINE`]).
/// ponytail: one at a time, so a long `wait` holds the next caller, and so
/// does each stalled connection for its `patience`; the one consumer is a
/// sequential runner.
fn accept(
    listener: TcpListener,
    token: &str,
    private: bool,
    patience: Duration,
    answer: impl Fn(Request) -> Option<Reply>,
) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            // out of descriptors, say: the caller waits in the backlog
            // rather than the thread spinning
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };
        let reading = Deadline {
            stream: &stream,
            until: Instant::now() + patience,
        };
        let reply = match read_request(reading) {
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
            409 => "Conflict",
            _ => "Service Unavailable",
        };
        let revision = reply.revision.map_or(String::new(), |revision| {
            format!("X-Ax-Revision: {revision}\r\n")
        });
        let mut stream = stream;
        let _ = stream.set_write_timeout(Some(patience));
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

/// One whole line into `line`. A line the head's cap or the source's end
/// cut short is no line: never taken for the empty one that ends the head.
fn whole_line(reader: &mut impl std::io::BufRead, line: &mut String) -> std::io::Result<()> {
    line.clear();
    reader.read_line(line)?;
    match line.ends_with('\n') {
        true => Ok(()),
        false => Err(std::io::ErrorKind::InvalidData.into()),
    }
}

/// Method, target, bearer token and body: the request line and headers in
/// [`HEAD_MAX`] bytes and [`HEADERS_MAX`] lines, the body in [`BODY_MAX`].
fn read_request(
    source: impl std::io::Read,
) -> std::io::Result<(String, String, Option<String>, Vec<u8>)> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let mut reader = BufReader::new(source.take(HEAD_MAX));
    let mut line = String::new();
    whole_line(&mut reader, &mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let (mut auth, mut length) = (None, 0usize);
    for headers in 0.. {
        whole_line(&mut reader, &mut line)?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if headers == HEADERS_MAX {
            return Err(invalid());
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
    if length > BODY_MAX {
        return Err(invalid());
    }
    // the body is past the head's cap: what the reader holds, and `length`
    // bytes more at most
    reader.get_mut().set_limit(length as u64);
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
        ("POST", "act") => parse::<Act>(body).and_then(|act| {
            typed(act.value.as_deref().unwrap_or_default()).map(|()| Request::Act(act))
        }),
        ("POST", "wait") => parse(body).map(Request::Wait),
        ("POST", "reveal") if private => parse(body).map(Request::Reveal),
        ("POST", "key") => parse::<Key>(body).and_then(|key| {
            typed(&key.keys)
                .and(typed(&key.text))
                .map(|()| Request::Key(key))
        }),
        ("GET", "keys") => Ok(Request::Keys(filter)),
        ("POST", "drag") => parse(body).map(Request::Drag),
        ("GET", "audit") => Ok(Request::Audit {
            filter,
            walk: flag("walk"),
            launcher: flag("launcher"),
        }),
        ("GET", "perf") => Ok(Request::Perf {
            by_instance: params.get("by") == Some(&"instance"),
        }),
        ("POST", "perf/reset") => Ok(Request::PerfReset),
        _ => {
            let mut endpoints = vec![
                "GET /tree",
                "GET /actions",
                "POST /act",
                "POST /wait",
                "POST /key",
                "GET /keys",
                "POST /drag",
                "GET /audit",
                "GET /perf",
                "POST /perf/reset",
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

/// 400 for text of more than [`MAX_TEXT`] characters.
fn typed(text: &str) -> Result<(), Reply> {
    match text.chars().count() > MAX_TEXT {
        true => Err(Reply::new(
            400,
            json!({ "error": format!("{MAX_TEXT} characters at most") }),
        )),
        false => Ok(()),
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
       ducktape-app ax act <id> <press|focus|set_value|type|increment|decrement|expand|collapse|context_menu|scroll_into_view> [value]
       ducktape-app ax key <keys> [--text T] [--window W] [--no-read]   (keys: tab, shift-tab, enter, ctrl-k …; --no-read: press without reading any tree, answers {}, docs/perf.md §4.3)
       ducktape-app ax keys [--window W]
       ducktape-app ax drag <x1,y1> <x2,y2> [--id ID] [--steps N] [--window W]   (px; local to ID's bounds when given)
       ducktape-app ax audit [--window W] [--view V] [--walk] [--launcher]   (docs/ax.md phase 1; --launcher: the shell screen is not the desk)
       ducktape-app ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]
       ducktape-app ax perf [--by instance | --reset]   (docs/perf.md; the app launched with DUCKTAPE_PERF=1; --reset zeroes the counters)
       ducktape-app ax reveal <id>   (only with DUCKTAPE_AX_DOOR_PRIVATE=1)";

/// `x,y` as the CLI takes a position.
fn point(word: &str) -> Option<[f32; 2]> {
    let (x, y) = word.split_once(',')?;
    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
}

/// `ducktape-app ax …`: prints the door's JSON. Exit 0 answered, 1 not
/// found, refused or timed out, 2 the door is not open.
pub(crate) fn cli(args: &[String]) -> i32 {
    let Some((method, target, body)) = request(args) else {
        eprintln!("{USAGE}");
        return 1;
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

/// The door call the CLI's words ask for: method, target and body; `None`
/// for words the usage does not list.
fn request(args: &[String]) -> Option<(&'static str, String, String)> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut words = Vec::new();
    let mut rest = args.iter().skip(1);
    while let Some(arg) = rest.next() {
        match arg.strip_prefix("--") {
            Some(
                name @ ("compact" | "bounds" | "gone" | "walk" | "launcher" | "reset" | "no-read"),
            ) => {
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
            json!({ "keys": keys.first().copied().unwrap_or_default(), "text": flags.get("text").copied().unwrap_or_default(), "window": flags.get("window"), "delta": !flags.contains_key("no-read") }).to_string(),
        ),
        (Some("keys"), []) => ("GET", format!("/keys?{}", query(&["window"])), String::new()),
        (Some("audit"), []) => (
            "GET",
            format!("/audit?{}", query(&["window", "view", "walk", "launcher"])),
            String::new(),
        ),
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
        (Some("perf"), []) if !flags.contains_key("reset") => {
            ("GET", format!("/perf?{}", query(&["by"])), String::new())
        }
        (Some("perf"), []) if !flags.contains_key("by") => ("POST", "/perf/reset".to_owned(), String::new()),
        _ => return None,
    };
    Some((method, target, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    /// `/perf` reads, by module unless `?by=instance`; only a POST resets,
    /// and both are in the 404 list a wrong path gets.
    #[test]
    fn perf_routes() {
        assert_eq!(
            route("GET", "/perf", b"", false),
            Ok(Request::Perf { by_instance: false })
        );
        assert_eq!(
            route("GET", "/perf?by=instance", b"", false),
            Ok(Request::Perf { by_instance: true })
        );
        assert_eq!(
            route("POST", "/perf/reset", b"", false),
            Ok(Request::PerfReset)
        );
        let Err(refused) = route("GET", "/perf/reset", b"", false) else {
            panic!("a GET never resets");
        };
        assert_eq!(refused.status, 404);
        assert!(refused.body.contains("POST /perf/reset"));
    }

    /// `ax perf` is a GET of `/perf`, with `--by instance` carried as the
    /// query; `ax perf --reset` is the POST that zeroes it, and takes no `--by`.
    #[test]
    fn perf_cli_words() {
        assert_eq!(
            request(&words("perf")),
            Some(("GET", "/perf?".to_owned(), String::new()))
        );
        assert_eq!(
            request(&words("perf --by instance")),
            Some(("GET", "/perf?by=instance".to_owned(), String::new()))
        );
        assert_eq!(request(&words("perf extra")), None);
        assert_eq!(
            request(&words("perf --reset")),
            Some(("POST", "/perf/reset".to_owned(), String::new()))
        );
        assert_eq!(request(&words("perf --reset --by instance")), None);
    }

    /// `POST /key` carries `"delta": false` when asked, and only then; a body
    /// without it means the delta, as before.
    #[test]
    fn key_route_parses_the_no_delta_flag() {
        let key = |body: &str| match route("POST", "/key", body.as_bytes(), false) {
            Ok(Request::Key(key)) => key,
            other => panic!("not a key request: {other:?}"),
        };
        assert!(key(r#"{"keys":"tab"}"#).delta);
        let bare = key(r#"{"keys":"tab","window":"console","delta":false}"#);
        assert!(!bare.delta);
        assert_eq!(bare.window.as_deref(), Some("console"));
    }

    /// `ax key … --no-read` sends `"delta": false`; without it the body says
    /// `true`, and the flag takes no value word.
    #[test]
    fn key_cli_words() {
        let body = |line: &str| {
            let (method, target, body) = request(&words(line)).expect("a key request");
            assert_eq!((method, target.as_str()), ("POST", "/key"));
            serde_json::from_str::<serde_json::Value>(&body).unwrap()
        };
        assert_eq!(body("key tab --window console")["delta"], true);
        let quiet = body("key tab --no-read --window console");
        assert_eq!(quiet["delta"], false);
        assert_eq!(
            (quiet["keys"].as_str(), quiet["window"].as_str()),
            (Some("tab"), Some("console"))
        );
        assert_eq!(body("key --no-read")["keys"], "");
    }

    /// A caller without the token can make the door read no more than
    /// [`HEAD_MAX`] bytes: a head with no line end is refused there, however
    /// much more the caller has to send.
    #[test]
    fn a_head_is_read_to_its_cap_and_no_further() {
        let sent = 1 << 20;
        let mut endless = std::io::repeat(b'A').take(sent);
        assert!(read_request(&mut endless).is_err());
        assert_eq!(sent - endless.limit(), HEAD_MAX);
    }

    /// [`HEADERS_MAX`] headers are a request, one more is not; a head the
    /// source ends before its empty line is not one either; and a body
    /// lands whole past a head near its cap.
    #[test]
    fn a_head_has_a_header_cap_and_an_end() {
        let head = |headers: usize| {
            format!(
                "GET /tree HTTP/1.1\r\n{}\r\n",
                "X-Pad: b\r\n".repeat(headers)
            )
        };
        assert!(read_request(head(HEADERS_MAX).as_bytes()).is_ok());
        assert!(read_request(head(HEADERS_MAX + 1).as_bytes()).is_err());
        assert!(read_request(&b"GET /tree HTTP/1.1\r\nX-Pad: b\r\n"[..]).is_err());
        let body = "b".repeat(64 << 10);
        let near = format!(
            "POST /act HTTP/1.1\r\nX-Pad: {}\r\nContent-Length: {}\r\n\r\n{body}",
            "a".repeat(15 << 10),
            body.len()
        );
        let (_, _, _, read) = read_request(near.as_bytes()).unwrap();
        assert_eq!(read, body.as_bytes());
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// A door on an ephemeral port that answers `{}` to every request it
    /// routes, and what reached its `answer`. Its thread outlives the test:
    /// `accept` never returns.
    fn door(private: bool, patience: Duration) -> (u16, std::sync::mpsc::Receiver<Request>) {
        let listener = bind(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (heard, seen) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            accept(listener, TOKEN, private, patience, move |request| {
                heard.send(request).ok()?;
                Some(Reply::ok(json!({})))
            })
        });
        (port, seen)
    }

    /// `bytes` sent as they are; the answer's status and body, or an error
    /// when none came within 3 s.
    fn send(port: u16, bytes: &[u8]) -> std::io::Result<(u16, String)> {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(bytes)?;
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

    /// A request as the CLI writes it, under `token`.
    fn request_with(token: &str, method: &str, target: &str, body: &str) -> Vec<u8> {
        format!(
            "{method} {target} HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    /// A wrong token, or none, is 401 and the app never hears of the
    /// request; the right one reaches it.
    #[test]
    fn a_wrong_token_is_refused_before_the_app_hears_of_it() {
        let (port, seen) = door(false, REQUEST_DEADLINE);
        let wrong = TOKEN.replace('0', "1");
        let (status, _) = send(port, &request_with(&wrong, "GET", "/tree", "")).unwrap();
        assert_eq!(status, 401);
        let (status, _) = send(port, b"GET /tree HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(status, 401);
        let (status, _) = send(port, &request_with(TOKEN, "GET", "/tree", "")).unwrap();
        assert_eq!(status, 200);
        assert!(matches!(seen.try_recv(), Ok(Request::Tree { .. })));
        assert!(seen.try_recv().is_err(), "only the request with the token");
    }

    /// `POST /reveal` is routed by a private door only; elsewhere it is a
    /// 404 whose endpoint list leaves it out.
    #[test]
    fn reveal_is_routed_on_a_private_door_only() {
        let reveal = request_with(TOKEN, "POST", "/reveal", r#"{"id":"console:x"}"#);
        let (port, seen) = door(false, REQUEST_DEADLINE);
        let (status, body) = send(port, &reveal).unwrap();
        assert_eq!(status, 404);
        assert!(body.contains("POST /act") && !body.contains("reveal"));
        assert!(seen.try_recv().is_err());
        let (port, seen) = door(true, REQUEST_DEADLINE);
        assert_eq!(send(port, &reveal).unwrap().0, 200);
        assert!(matches!(seen.try_recv(), Ok(Request::Reveal(_))));
    }

    /// A head of [`HEAD_MAX`] bytes with no line end is 400 at once, not at
    /// the deadline, and a body over [`BODY_MAX`] is 400 before its token
    /// is weighed (a wrong one included); the app hears of neither.
    #[test]
    fn an_oversized_head_or_body_is_refused_before_the_token() {
        let (port, seen) = door(false, Duration::from_secs(30));
        let (status, _) = send(port, &[b'A'; HEAD_MAX as usize]).unwrap();
        assert_eq!(status, 400);
        let over = |token: &str| {
            format!(
                "POST /act HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n",
                BODY_MAX + 1
            )
        };
        assert_eq!(send(port, over(TOKEN).as_bytes()).unwrap().0, 400);
        assert_eq!(send(port, over("wrong").as_bytes()).unwrap().0, 400);
        assert!(seen.try_recv().is_err());
    }

    /// A caller that sends a byte now and then, never a whole request, has
    /// the door for its deadline, not for as long as it keeps sending: the
    /// next caller is answered.
    #[test]
    fn a_trickling_caller_holds_the_door_no_longer_than_its_deadline() {
        let (port, seen) = door(false, Duration::from_millis(300));
        let mut slow = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(4);
            while Instant::now() < until && slow.write_all(b"G").is_ok() {
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        std::thread::sleep(Duration::from_millis(100));
        let (status, _) = send(port, &request_with(TOKEN, "GET", "/tree", ""))
            .expect("the next caller is answered within 3 s");
        assert_eq!(status, 200);
        assert!(matches!(seen.try_recv(), Ok(Request::Tree { .. })));
    }

    /// An open door has its file, naming a port that answers; the file goes
    /// with the [`Written`] (the app quitting).
    #[test]
    fn an_open_door_has_its_file_until_the_app_quits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ducktape").join("ax-door.json");
        let (_calls, written) = open_env(Some("0"), None, Ok(path.clone())).unwrap();
        let door: DoorFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let (status, _) = send(door.port, &request_with("wrong", "GET", "/tree", "")).unwrap();
        assert_eq!(status, 401);
        drop(written);
        assert!(!path.exists(), "the file outlived the door");
    }

    /// Quitting leaves a door file another app has since written in its
    /// place.
    #[test]
    fn quitting_leaves_another_apps_door_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ax-door.json");
        let ours = DoorFile {
            port: 1,
            token: "ours".into(),
        };
        let written = write_door_file(path.clone(), &ours).unwrap();
        std::fs::write(&path, r#"{"port":2,"token":"theirs"}"#).unwrap();
        drop(written);
        assert!(path.exists(), "another app's door file went");
    }

    /// A door that cannot bind its port takes away the file a run before
    /// left, so no client carries that run's token to whoever holds the
    /// port now.
    #[test]
    fn a_door_that_cannot_bind_takes_a_stale_file_away() {
        let held = bind(0).unwrap();
        let port = held.local_addr().unwrap().port().to_string();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ax-door.json");
        std::fs::write(&path, r#"{"port":1,"token":"stale"}"#).unwrap();
        assert!(open_env(Some(&port), None, Ok(path.clone())).is_none());
        assert!(!path.exists(), "the stale door file stayed");
    }

    /// `POST /key` and `POST /act` carry at most [`MAX_TEXT`] characters in
    /// each typed field; one more is 400 before the app sees it.
    #[test]
    fn typed_text_is_capped() {
        let status = |path: &str, body: serde_json::Value| {
            route("POST", path, body.to_string().as_bytes(), false)
                .map_or_else(|reply| reply.status, |_| 200)
        };
        let (most, over) = ("é".repeat(MAX_TEXT), "é".repeat(MAX_TEXT + 1));
        assert_eq!(status("/key", json!({ "text": most })), 200);
        assert_eq!(status("/key", json!({ "text": over })), 400);
        assert_eq!(
            status("/key", json!({ "keys": "a ".repeat(MAX_TEXT) })),
            400
        );
        assert_eq!(
            status(
                "/act",
                json!({ "id": "x", "action": "type", "value": most })
            ),
            200
        );
        assert_eq!(
            status(
                "/act",
                json!({ "id": "x", "action": "type", "value": over })
            ),
            400
        );
    }
}
