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

/// `ax-door.json` in the shell's runtime directory (`Dirs::runtime`:
/// `$XDG_RUNTIME_DIR/ducktape` on the desktop, else its cache directory;
/// never the state directory, which is `~/Library/Logs` on macOS, where
/// support bundles collect).
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
fn door_file() -> PathBuf {
    crate::backend::dirs().runtime.join("ax-door.json")
}

/// The door file as this app wrote it. Dropped (the app quitting), the file
/// goes, unless another app has written its own door there since.
pub struct Written {
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

/// Writes the door's port and token. Readable by this user only because
/// `Dirs::runtime` is: the shell makes that directory and makes it private
/// (`contract::Dirs`); it is never created here, so a shell that forgot
/// gets this error and no door, not a token in a directory anyone reads.
fn write_door_file(path: PathBuf, door: &DoorFile) -> std::io::Result<Written> {
    let mut file = std::fs::File::create(&path)?;
    let text = serde_json::to_string(door).unwrap_or_default();
    file.write_all(text.as_bytes())?;
    Ok(Written { path, text })
}

/// Opens the door when `DUCKTAPE_AX_DOOR` asks for it; the calls it takes
/// arrive on the returned channel for [`serve`], and the door file stays
/// while the [`Written`] does.
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
pub fn open() -> Option<(futures::channel::mpsc::UnboundedReceiver<Call>, Written)> {
    open_env(
        std::env::var("DUCKTAPE_AX_DOOR").ok().as_deref(),
        std::env::var("DUCKTAPE_AX_DOOR_PRIVATE").ok().as_deref(),
        Ok(door_file()),
    )
}

/// Opens the door on `port`, whatever the environment says: for a shell
/// whose launch takes no environment (a phone, wasm-mobile SPEC §11.1).
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
pub fn open_at(port: u16) -> Option<(futures::channel::mpsc::UnboundedReceiver<Call>, Written)> {
    open_env(Some(&port.to_string()), None, Ok(door_file()))
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
    // the door file's path is settled, and its directory there, before the
    // port is taken: a door no client can learn of is never opened. The
    // directory is the shell's to make and make private (`contract::Dirs`);
    // the door never makes it
    let file = match file {
        Ok(file) if file.parent().is_some_and(std::path::Path::is_dir) => file,
        Ok(file) => {
            let error = format!("{}: no runtime directory", file.display());
            tracing::warn!(target: "ducktape::app", reason = "ax_door_file_unwritten", %error, "the test door stays shut");
            return None;
        }
        Err(error) => {
            tracing::warn!(target: "ducktape::app", reason = "ax_door_file_unwritten", %error, "the test door stays shut");
            return None;
        }
    };
    let opened = bind(port).and_then(|listener| Ok((listener.local_addr()?.port(), listener)));
    let (port, listener) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            // a door file left by a run before names a door that is not
            // this app's: a client must not take its token to that port
            let _ = std::fs::remove_file(&file);
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
            accept(listener, &token, private, |request| {
                let (reply, answer) = std::sync::mpsc::channel();
                sender.unbounded_send((request, reply)).ok()?;
                answer.recv().ok()
            })
        })
        .ok()?;
    // written once the thread is up, so the file never names a door no one
    // answers; on a failure here the thread stays, and no client can learn
    // its token
    let written = match write_door_file(file, &door) {
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

/// The door's HTTP/1.1 loop: one request per connection, each connection
/// read and answered on a thread of its own, so a caller that stalls or
/// trickles holds up no other; `/reveal` exists only when `private`.
fn accept(
    listener: TcpListener,
    token: &str,
    private: bool,
    answer: impl Fn(Request) -> Option<Reply> + Sync,
) {
    std::thread::scope(|scope| {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                // out of descriptors, say: the caller waits in the backlog
                // rather than the thread spinning
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let answer = &answer;
            scope.spawn(move || serve(stream, token, private, answer));
        }
    });
}

/// One connection to the door: its request read, routed and answered.
fn serve(
    mut stream: TcpStream,
    token: &str,
    private: bool,
    answer: &impl Fn(Request) -> Option<Reply>,
) {
    let reply = match read_request(&stream, token) {
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
    let _ = write!(
        stream,
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{revision}Connection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
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

/// One whole line into `line`. A line the source's end cut short is no
/// line: never taken for the empty one that ends the head.
fn whole_line(reader: &mut impl std::io::BufRead, line: &mut String) -> std::io::Result<()> {
    line.clear();
    reader.read_line(line)?;
    match line.ends_with('\n') {
        true => Ok(()),
        false => Err(std::io::ErrorKind::InvalidData.into()),
    }
}

/// Method, target, bearer token and body. The body is read only for the
/// caller that holds `token`; any other gets it empty, unread.
fn read_request(
    source: impl std::io::Read,
    token: &str,
) -> std::io::Result<(String, String, Option<String>, Vec<u8>)> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let mut reader = BufReader::new(source);
    let mut line = String::new();
    whole_line(&mut reader, &mut line)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let (method, target) = (method.to_owned(), target.to_owned());
    let (mut auth, mut length) = (None, 0u64);
    loop {
        whole_line(&mut reader, &mut line)?;
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
    if !same(auth.as_deref().unwrap_or_default(), token) {
        return Ok((method, target, auth, Vec::new()));
    }
    let mut body = Vec::new();
    reader.take(length).read_to_end(&mut body)?;
    match body.len() as u64 == length {
        true => Ok((method, target, auth, body)),
        false => Err(invalid()),
    }
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
        ("GET", "audit") => Ok(Request::Audit {
            filter,
            walk: flag("walk"),
            launcher: flag("launcher"),
        }),
        ("GET", "perf") => Ok(Request::Perf {
            by_instance: params.get("by") == Some(&"instance"),
        }),
        ("POST", "perf/reset") => Ok(Request::PerfReset),
        ("POST", "motion") => parse(body).map(Request::Motion),
        ("GET", "inspect/tree") => Ok(Request::Inspect(inspect::Ask::Tree(
            params.get("in").map(|scope| scope.to_string()),
        ))),
        ("POST", "inspect/find") => {
            parse(body).map(|find| Request::Inspect(inspect::Ask::Find(find)))
        }
        ("POST", "inspect/press") => {
            parse(body).map(|press| Request::Inspect(inspect::Ask::Press(press)))
        }
        ("POST", "inspect/type") => {
            parse(body).map(|typed| Request::Inspect(inspect::Ask::Type(typed)))
        }
        ("POST", "inspect/wait") => {
            parse(body).map(|wait| Request::Inspect(inspect::Ask::Wait(wait)))
        }
        ("POST", "inspect/scroll") => {
            parse(body).map(|scroll| Request::Inspect(inspect::Ask::Scroll(scroll)))
        }
        ("GET", "inspect/screenshot") => Ok(Request::Inspect(inspect::Ask::Screenshot(
            params.get("window").map(|window| window.to_string()),
        ))),
        ("GET", "inspect/memory") => Ok(Request::Inspect(inspect::Ask::Memory {
            regions: flag("regions"),
        })),
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
                "POST /motion",
                "GET /inspect/tree",
                "POST /inspect/find",
                "POST /inspect/press",
                "POST /inspect/type",
                "POST /inspect/wait",
                "POST /inspect/scroll",
                "GET /inspect/screenshot",
                "GET /inspect/memory",
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
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
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

#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
const USAGE: &str = "usage: ducktape-app ax tree [--window W] [--view V] [--compact] [--bounds]
       ducktape-app ax actions [--window W] [--view V]
       ducktape-app ax act <id> <press|focus|set_value|type|increment|decrement|expand|collapse|context_menu|scroll_into_view> [value]
       ducktape-app ax key <keys> [--text T] [--window W] [--no-read]   (keys: tab, shift-tab, enter, ctrl-k …; --no-read: press without reading any tree, answers {}, docs/perf.md §4.3)
       ducktape-app ax keys [--window W]
       ducktape-app ax drag <x1,y1> <x2,y2> [--id ID] [--steps N] [--window W]   (px; local to ID's bounds when given)
       ducktape-app ax audit [--window W] [--view V] [--walk] [--launcher]   (docs/ax.md phase 1; --launcher: the shell screen is not the desk)
       ducktape-app ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]
       ducktape-app ax perf [--by instance | --reset]   (docs/perf.md; the app launched with DUCKTAPE_PERF=1; --reset zeroes the counters)
       ducktape-app ax reveal <id>   (only with DUCKTAPE_AX_DOOR_PRIVATE=1)
       ducktape-app ax inspect tree [--in W[:V]]   (the views' own trees by authored path, and the shell's own controls by id; no accessibility tree)
       ducktape-app ax inspect find [--id ID] [--role R] [--label L] [--text T] [--in W[:V]]
       ducktape-app ax inspect press <id> [click|aux_click|submit|dismiss]   (scrolled into view first; a view's node: its own handler event; a native control: a pointer's click, refused when covered)
       ducktape-app ax inspect type <id> <text>   (scrolled into view first)
       ducktape-app ax inspect scroll <id> (--by X,Y | --to start|end --deadline-ms MS)   (a wheel at the node's middle: by px once, or to an end until a wheel moves nothing on screen; refused when covered)
       ducktape-app ax inspect wait [--id ID] [--role R] [--label L] [--text T] [--in W[:V]] [--gone] --deadline-ms MS   (408 carries the tree it last read)
       ducktape-app ax inspect screenshot [--window W]   (a PNG in base64, in a build with test-support; else 501 in its own words)
       ducktape-app ax inspect memory [--regions]   (the app's memory by owner beside the OS's own figures; --regions adds the OS's breakdown by region)";

/// `x,y` as the CLI takes a position.
fn point(word: &str) -> Option<[f32; 2]> {
    let (x, y) = word.split_once(',')?;
    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
}

/// `ducktape-app ax …`: prints the door's JSON. Exit 0 answered, 1 not
/// found, refused or timed out, 2 the door is not open.
#[cfg_attr(not(feature = "ax-door"), allow(dead_code))]
pub fn cli(args: &[String]) -> i32 {
    let Some((method, target, body)) = request(args) else {
        eprintln!("{USAGE}");
        return 1;
    };
    let door = {
        let path = door_file();
        std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))
            .and_then(|text| {
                serde_json::from_str::<DoorFile>(&text).map_err(|error| error.to_string())
            })
    };
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
                name @ ("compact" | "bounds" | "gone" | "walk" | "launcher" | "reset" | "no-read"
                | "regions"),
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
        (Some("inspect"), ["tree"]) => ("GET", format!("/inspect/tree?{}", query(&["in"])), String::new()),
        (Some("inspect"), ["find"]) => (
            "POST",
            "/inspect/find".to_owned(),
            json!({ "id": flags.get("id"), "role": flags.get("role"), "label": flags.get("label"), "text": flags.get("text"), "in": flags.get("in") }).to_string(),
        ),
        (Some("inspect"), ["press", id, press @ ..]) if press.len() <= 1 => (
            "POST",
            "/inspect/press".to_owned(),
            json!({ "id": id, "press": press.first().copied().unwrap_or("click") }).to_string(),
        ),
        (Some("inspect"), ["type", id, text]) => (
            "POST",
            "/inspect/type".to_owned(),
            json!({ "id": id, "text": text }).to_string(),
        ),
        (Some("inspect"), ["wait"]) if flags.get("deadline-ms").is_some_and(|ms| ms.parse::<u64>().is_ok()) => (
            "POST",
            "/inspect/wait".to_owned(),
            json!({
                "id": flags.get("id"),
                "role": flags.get("role"),
                "label": flags.get("label"),
                "text": flags.get("text"),
                "in": flags.get("in"),
                "gone": flags.contains_key("gone"),
                "deadline_ms": flags.get("deadline-ms").and_then(|ms| ms.parse::<u64>().ok()),
            })
            .to_string(),
        ),
        (Some("inspect"), ["scroll", id]) if flags.get("by").is_some_and(|by| point(by).is_some()) => (
            "POST",
            "/inspect/scroll".to_owned(),
            json!({ "id": id, "by": flags.get("by").and_then(|by| point(by)).map(|[x, y]| json!({ "x": x, "y": y })) }).to_string(),
        ),
        (Some("inspect"), ["scroll", id]) if flags.get("deadline-ms").is_some_and(|ms| ms.parse::<u64>().is_ok()) => (
            "POST",
            "/inspect/scroll".to_owned(),
            json!({ "id": id, "to": flags.get("to"), "deadline_ms": flags.get("deadline-ms").and_then(|ms| ms.parse::<u64>().ok()) }).to_string(),
        ),
        (Some("inspect"), ["screenshot"]) => ("GET", format!("/inspect/screenshot?{}", query(&["window"])), String::new()),
        (Some("inspect"), ["memory"]) => ("GET", format!("/inspect/memory?{}", query(&["regions"])), String::new()),
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

    /// `/perf` reads, by program unless `?by=instance`; only a POST resets,
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

    /// `POST /motion` carries whether to ask for less motion; a body
    /// without it is refused, and the path is in the 404 list.
    #[test]
    fn motion_route() {
        assert_eq!(
            route("POST", "/motion", br#"{"reduce":true}"#, false),
            Ok(Request::Motion(crate::ax::Motion { reduce: true }))
        );
        let Err(refused) = route("POST", "/motion", b"{}", false) else {
            panic!("a motion request says which");
        };
        assert_eq!(refused.status, 400);
        let Err(refused) = route("GET", "/motion", b"", false) else {
            panic!("a GET never sets it");
        };
        assert!(refused.body.contains("POST /motion"));
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

    #[test]
    fn memory_route_and_cli_words() {
        assert_eq!(
            route("GET", "/inspect/memory?regions=1", b"", false),
            Ok(Request::Inspect(inspect::Ask::Memory { regions: true }))
        );
        assert_eq!(
            route("GET", "/inspect/memory", b"", false),
            Ok(Request::Inspect(inspect::Ask::Memory { regions: false }))
        );
        assert_eq!(
            request(&words("inspect memory --regions")),
            Some(("GET", "/inspect/memory?regions=1".to_owned(), String::new()))
        );
        assert_eq!(
            request(&words("inspect memory")),
            Some(("GET", "/inspect/memory?".to_owned(), String::new()))
        );
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

    /// A head the source ends before its empty line is not a request, and
    /// a body lands whole behind a long head.
    #[test]
    fn a_head_has_an_end() {
        assert!(read_request(&b"GET /tree HTTP/1.1\r\nX-Pad: b\r\n"[..], TOKEN).is_err());
        let body = "b".repeat(64 << 10);
        let near = format!(
            "POST /act HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nX-Pad: {}\r\nContent-Length: {}\r\n\r\n{body}",
            "a".repeat(1 << 20),
            body.len()
        );
        let (_, _, _, read) = read_request(near.as_bytes(), TOKEN).unwrap();
        assert_eq!(read, body.as_bytes());
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    /// A door on an ephemeral port that answers `{}` to every request it
    /// routes, and what reached its `answer`. Its thread outlives the test:
    /// `accept` never returns.
    fn door(private: bool) -> (u16, std::sync::mpsc::Receiver<Request>) {
        let listener = bind(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (heard, seen) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            accept(listener, TOKEN, private, move |request| {
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
        let (port, seen) = door(false);
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
        let (port, seen) = door(false);
        let (status, body) = send(port, &reveal).unwrap();
        assert_eq!(status, 404);
        assert!(body.contains("POST /act") && !body.contains("reveal"));
        assert!(seen.try_recv().is_err());
        let (port, seen) = door(true);
        assert_eq!(send(port, &reveal).unwrap().0, 200);
        assert!(matches!(seen.try_recv(), Ok(Request::Reveal(_))));
    }

    /// A caller that sends a byte now and then, never a whole request,
    /// holds up no other caller: the next one is answered while it is
    /// still sending.
    #[test]
    fn a_trickling_caller_holds_up_no_other() {
        let (port, seen) = door(false);
        let mut slow = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        slow.write_all(b"G").unwrap();
        let (status, _) = send(port, &request_with(TOKEN, "GET", "/tree", "")).unwrap();
        assert_eq!(status, 200);
        assert!(matches!(seen.try_recv(), Ok(Request::Tree { .. })));
        drop(slow);
    }

    /// An open door has its file, naming a port that answers; the file goes
    /// with the [`Written`] (the app quitting).
    #[test]
    fn an_open_door_has_its_file_until_the_app_quits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ax-door.json");
        let (_calls, written) = open_env(Some("0"), None, Ok(path.clone())).unwrap();
        let door: DoorFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let (status, _) = send(door.port, &request_with("wrong", "GET", "/tree", "")).unwrap();
        assert_eq!(status, 401);
        drop(written);
        assert!(!path.exists(), "the file outlived the door");
    }

    /// The door never makes its own directory: a runtime directory the
    /// shell did not make (and so did not make private) is no door at all,
    /// and the port stays free.
    #[test]
    fn a_door_makes_no_runtime_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("forgotten");
        let path = missing.join("ax-door.json");
        let held = bind(0).unwrap();
        let port = held.local_addr().unwrap().port();
        drop(held);
        assert!(open_env(Some(&port.to_string()), None, Ok(path)).is_none());
        assert!(!missing.exists(), "the door made the directory itself");
        bind(port).expect("the port was taken by a door with no file");
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

    /// A door file path that cannot be made leaves no listener: nothing is
    /// bound, so the port stays free.
    #[test]
    fn a_door_with_no_file_path_binds_nothing() {
        let held = bind(0).unwrap();
        let port = held.local_addr().unwrap().port();
        drop(held);
        assert!(open_env(Some(&port.to_string()), None, Err("no cache dir".into())).is_none());
        bind(port).expect("the port was taken by a door no client can use");
    }

    /// A wrong token with a large declared body is 401 at once: the caller
    /// never sends the body, and the door does not wait for it.
    #[test]
    fn a_wrong_token_is_refused_without_reading_the_body() {
        let (port, seen) = door(false);
        let head = format!(
            "POST /act HTTP/1.1\r\nAuthorization: Bearer wrong\r\nContent-Length: {}\r\n\r\n",
            1u64 << 40
        );
        assert_eq!(send(port, head.as_bytes()).unwrap().0, 401);
        assert!(seen.try_recv().is_err());
    }

    /// `POST /key` and `POST /act` take typed text of any length.
    #[test]
    fn typed_text_is_taken_whole() {
        let status = |path: &str, body: serde_json::Value| {
            route("POST", path, body.to_string().as_bytes(), false)
                .map_or_else(|reply| reply.status, |_| 200)
        };
        let long = "é".repeat(1 << 16);
        assert_eq!(status("/key", json!({ "text": long })), 200);
        assert_eq!(
            status(
                "/act",
                json!({ "id": "x", "action": "type", "value": long })
            ),
            200
        );
    }
}
