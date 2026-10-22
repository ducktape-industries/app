//! An artifact, live: CEF loads duck pages that reach for this machine or
//! follow a link, each case's own listener on 127.0.0.1 tells what reached it,
//! and the app's side hears what the page hands it.

use std::cell::{Cell, RefCell};
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use ducktape_web::{Leave, Network, Page, PageState, Served};
use gpui_kit::accesskit::Role;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, ParentElement as _, Point, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, WindowHandle, div, px,
};

const INNER: &str = "DUCKTAPE_WEB_LIVE_INNER";
const ONLY: &str = "DUCKTAPE_WEB_LIVE_ONLY";
const PORTS: &str = "DUCKTAPE_WEB_LIVE_PORTS";
const REPORT: &str = "DUCKTAPE_WEB_LIVE_REPORT";
const CHROMIUM: &str = "websocket-tls";
const PRECONNECT: &str = "preconnect";
const SITE: &str = "duck://duckpond-live/files/live/";
const PUBLIC: &str = "https://public.example/";
const LANDING: &str = "duck://duckpond-live/files/live/landing.html";
const ROOM: &str = "duck://duckpond-live/chat/general";
const OUTLINE: &str = include_str!("../testdata/outline.html");
const READY: &str = "navigation.addEventListener('navigateerror', () => done('dropped')); new PerformanceObserver((list) => { if (list.getEntriesByName('first-contentful-paint').length) { document.title = 'ready'; } }).observe({ type: 'paint', buffered: true });";
const SIZE: &str = "<script>const size = () => { document.title = 'size:' + innerHeight + ':' + Math.ceil(document.documentElement.getBoundingClientRect().height); }; addEventListener('resize', size); new ResizeObserver(size).observe(document.documentElement);</script>";
const SHARED_WORKER: &str = "try { importScripts('http://127.0.0.1:PORT/shared-worker-import'); } catch (error) {} onconnect = (event) => { const port = event.ports[0]; fetch('http://127.0.0.1:PORT/shared-worker', { mode: 'no-cors' }).then(() => port.postMessage('answered'), () => port.postMessage('refused')); };";
const WORKER: &str = "try { importScripts('http://127.0.0.1:PORT/worker-import'); } catch (error) {} fetch('http://127.0.0.1:PORT/worker', { mode: 'no-cors' }).then(() => postMessage('answered'), () => postMessage('refused'));";

enum Wait {
    Settled,
    Navigated,
    Clicked,
    Reloaded(&'static str),
    Heard(&'static str),
}

enum Shown {
    Always,
    Heard(&'static str),
    Left(&'static str),
    Ran,
    Refused(&'static str),
}

struct Case {
    name: &'static str,
    script: &'static str,
    link: Option<&'static str>,
    wait: Wait,
    reached: &'static [&'static str],
    shown: Shown,
}

const CASES: &[Case] = &[
    Case {
        name: "fetch",
        script: "fetch('http://127.0.0.1:PORT/fetch', { mode: 'no-cors' }).then(() => done('answered'), () => done('refused'));",
        link: None,
        wait: Wait::Settled,
        reached: &["/fetch"],
        shown: Shown::Always,
    },
    Case {
        name: "websocket",
        script: "try { const socket = new WebSocket('ws://127.0.0.1:PORT/websocket'); socket.onopen = () => done('open'); socket.onclose = () => done('closed'); } catch (error) { done('thrown'); }",
        link: None,
        wait: Wait::Settled,
        reached: &["/websocket"],
        shown: Shown::Always,
    },
    Case {
        name: CHROMIUM,
        script: "try { const socket = new WebSocket('wss://127.0.0.1:PORT/websocket-tls'); socket.onopen = () => done('open'); socket.onclose = () => done('closed'); } catch (error) { done('thrown'); }",
        link: None,
        wait: Wait::Settled,
        reached: &["tls-hello"],
        shown: Shown::Always,
    },
    Case {
        name: "image",
        script: "const image = new Image(); image.onload = () => done('answered'); image.onerror = () => done('refused'); image.src = 'http://127.0.0.1:PORT/image';",
        link: None,
        wait: Wait::Settled,
        reached: &["/image"],
        shown: Shown::Always,
    },
    Case {
        name: "frame",
        script: "const frame = document.createElement('iframe'); frame.src = 'http://public.test:PORT/frame'; document.body.append(frame);",
        link: None,
        wait: Wait::Navigated,
        reached: &["/frame", "tls-hello"],
        shown: Shown::Refused("frame-src"),
    },
    Case {
        name: "object",
        script: "const object = document.createElement('object'); object.type = 'text/html'; object.data = 'http://public.test:PORT/object'; document.body.append(object);",
        link: None,
        wait: Wait::Navigated,
        reached: &["/object", "tls-hello"],
        shown: Shown::Refused("object-src"),
    },
    Case {
        name: "embed",
        script: "const embed = document.createElement('embed'); embed.type = 'text/html'; embed.src = 'http://public.test:PORT/embed'; document.body.append(embed);",
        link: None,
        wait: Wait::Navigated,
        reached: &["/embed", "tls-hello"],
        shown: Shown::Refused("object-src"),
    },
    Case {
        name: "shared-worker",
        script: "const worker = new SharedWorker('shared-worker.js'); worker.onerror = () => done('no-worker'); worker.port.onmessage = (event) => done(event.data); worker.port.start();",
        link: None,
        wait: Wait::Settled,
        reached: &["/shared-worker-import", "/shared-worker"],
        shown: Shown::Ran,
    },
    Case {
        name: "worker",
        script: "const worker = new Worker('worker.js'); worker.onerror = () => done('no-worker'); worker.onmessage = (event) => done(event.data);",
        link: None,
        wait: Wait::Settled,
        reached: &["/worker-import", "/worker"],
        shown: Shown::Ran,
    },
    Case {
        name: "redirect",
        script: "const image = new Image(); image.onload = () => done('answered'); image.onerror = () => done('refused'); image.src = 'http://public.test:PORT/redirect';",
        link: None,
        wait: Wait::Settled,
        reached: &["/redirect-target"],
        shown: Shown::Heard("/redirect"),
    },
    Case {
        name: "keepalive",
        script: "addEventListener('pagehide', () => { fetch('http://127.0.0.1:PORT/keepalive', { keepalive: true, mode: 'no-cors' }); fetch('keepalive-after.txt', { keepalive: true }); }); addEventListener('load', () => { document.title = 'ready'; });",
        link: None,
        wait: Wait::Reloaded("keepalive-after.txt"),
        reached: &["/keepalive"],
        shown: Shown::Always,
    },
    Case {
        name: "away",
        script: "navigation.addEventListener('navigateerror', () => done('dropped')); location = 'http://127.0.0.1:PORT/away';",
        link: None,
        wait: Wait::Navigated,
        reached: &["/away"],
        shown: Shown::Always,
    },
    Case {
        name: "away-public",
        script: "navigation.addEventListener('navigateerror', () => done('dropped')); location = 'https://public.example/';",
        link: None,
        wait: Wait::Navigated,
        reached: &[],
        shown: Shown::Always,
    },
    Case {
        name: "away-click",
        script: READY,
        link: Some("http://127.0.0.1:PORT/away-click"),
        wait: Wait::Clicked,
        reached: &["/away-click"],
        shown: Shown::Always,
    },
    Case {
        name: "click-public",
        script: READY,
        link: Some(PUBLIC),
        wait: Wait::Clicked,
        reached: &[],
        shown: Shown::Left(PUBLIC),
    },
    Case {
        name: "click-room",
        script: READY,
        link: Some(ROOM),
        wait: Wait::Clicked,
        reached: &[],
        shown: Shown::Left(ROOM),
    },
    Case {
        name: "click-page",
        script: READY,
        link: Some(LANDING),
        wait: Wait::Clicked,
        reached: &[],
        shown: Shown::Left(LANDING),
    },
    Case {
        name: "dns-prefetch",
        script: "const link = document.createElement('link'); link.rel = 'dns-prefetch'; link.href = 'https://public.test:PORT/'; document.head.append(link); done('linked');",
        link: None,
        wait: Wait::Settled,
        reached: &["tls-hello", "connection", "/"],
        shown: Shown::Always,
    },
    Case {
        name: PRECONNECT,
        script: "const link = document.createElement('link'); link.rel = 'preconnect'; link.href = 'https://127.0.0.1:PORT/'; document.head.append(link);",
        link: None,
        wait: Wait::Heard("tls-hello"),
        reached: &["/"],
        shown: Shown::Heard("tls-hello"),
    },
];

fn main() {
    if let Some(code) = ducktape_web::child() {
        std::process::exit(code);
    }
    let code = if std::env::var_os(INNER).is_some() {
        inner()
    } else {
        outer()
    };
    std::process::exit(code);
}

fn outer() -> i32 {
    let log = Log::default();
    let sink = Sink::default();
    let mut ports = Vec::new();
    for case in CASES {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port on this machine");
        let port = listener
            .local_addr()
            .expect("the listener's address")
            .port();
        ports.push(format!("{}={port}", case.name));
        let log = log.clone();
        let sink = sink.clone();
        std::thread::spawn(move || listen(listener, case.name, port, log, sink));
    }
    let ports = ports.join(",");
    let report = TcpListener::bind("127.0.0.1:0").expect("a port for the report");
    let report_port = report.local_addr().expect("the report's address").port();
    std::thread::spawn({
        let sink = sink.clone();
        move || {
            for mut stream in report.incoming().flatten() {
                let mut held = sink.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = stream.write_all(b"ready\n");
                *held = Some(stream);
            }
        }
    });
    let exe = std::env::current_exe().expect("this test's binary");
    let runtime = exe
        .parent()
        .and_then(Path::parent)
        .expect("the directory cef-dll-sys copies CEF's runtime files into");
    let runner = runtime.join("ducktape-web-live");
    let _ = std::fs::remove_file(&runner);
    if std::fs::hard_link(&exe, &runner).is_err() {
        std::fs::copy(&exe, &runner).expect("this test's binary beside CEF's runtime files");
    }
    let named: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let only = named.join(",");
    println!("live: Chromium's local network check off, the app's fence and policy on");
    let checked_off = run(&runner, &ports, report_port, false, &only);
    let shipped = named.is_empty() || named.iter().any(|name| name == CHROMIUM);
    let chromium = match shipped {
        true => {
            println!("live: as shipped, Chromium's local network check on");
            run(&runner, &ports, report_port, true, CHROMIUM)
        }
        false => 0,
    };
    let _ = std::fs::remove_file(&runner);
    let after = after(&log, &only, shipped);
    checked_off.max(chromium).max(after)
}

fn after(log: &Log, only: &str, shipped: bool) -> i32 {
    let heard = log.lock().unwrap_or_else(PoisonError::into_inner).clone();
    println!("live: every case again, by what its own listener heard once every run ended");
    let mut fenced = true;
    for case in CASES {
        let ran = picked(only, case.name) || (shipped && case.name == CHROMIUM);
        if !ran {
            continue;
        }
        let reached: Vec<&str> = heard
            .iter()
            .filter(|(name, what)| *name == case.name && case.reached.contains(&what.as_str()))
            .map(|(_, what)| what.as_str())
            .collect();
        match reached.is_empty() {
            true => println!("live {}: as it should, by its listener", case.name),
            false => {
                fenced = false;
                println!(
                    "live {}: REACHED, by its listener: {}",
                    case.name,
                    reached.join("; ")
                );
            }
        }
    }
    i32::from(!fenced)
}

fn run(runner: &Path, ports: &str, report: u16, local_network_check: bool, only: &str) -> i32 {
    let origins: Vec<String> = ports
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(_, port)| format!("http://public.test:{port}"))
        .collect();
    let mut args = vec![
        "--ozone-platform=headless".to_owned(),
        "--host-resolver-rules=MAP public.test 127.0.0.1".to_owned(),
        format!(
            "--unsafely-treat-insecure-origin-as-secure={}",
            origins.join(",")
        ),
    ];
    if !local_network_check {
        args.push("--disable-features=LocalNetworkAccessChecks,BlockInsecurePrivateNetworkRequests,PrivateNetworkAccessSendPreflights".to_owned());
    }
    Command::new(runner)
        .args(args)
        .env(INNER, "1")
        .env(ONLY, only)
        .env(PORTS, ports)
        .env(REPORT, report.to_string())
        .status()
        .expect("the run beside CEF's runtime files")
        .code()
        .unwrap_or(1)
}

type Heard = Arc<Mutex<Vec<String>>>;

type Log = Arc<Mutex<Vec<(&'static str, String)>>>;

type Sink = Arc<Mutex<Option<TcpStream>>>;

fn listen(listener: TcpListener, case: &'static str, port: u16, log: Log, sink: Sink) {
    for stream in listener.incoming().flatten() {
        let log = log.clone();
        let sink = sink.clone();
        std::thread::spawn(move || answer(stream, case, port, &log, &sink));
    }
}

fn record(case: &'static str, what: &str, log: &Log, sink: &Sink) {
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((case, what.to_owned()));
    if let Some(stream) = sink.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = writeln!(stream, "{case} {what}");
    }
}

fn answer(mut stream: TcpStream, case: &'static str, port: u16, log: &Log, sink: &Sink) {
    let mut first = [0u8; 1];
    let sent = stream.peek(&mut first).unwrap_or(0);
    let kind = match (sent, first[0]) {
        (0, _) => "connection",
        (_, 0x16) => "tls-hello",
        _ => "request",
    };
    println!("live: {case}'s listener took a connection: {kind}");
    if kind != "request" {
        record(case, kind, log, sink);
        return;
    }
    let Ok(read) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(read).lines();
    let Some(Ok(request)) = lines.next() else {
        return;
    };
    for line in lines {
        let Ok(line) = line else { break };
        if line.is_empty() {
            break;
        }
    }
    let mut parts = request.split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default().to_owned();
    println!("live: {case}'s listener heard {method} {target}");
    record(case, &target, log, sink);
    let reply = match (method, target.as_str()) {
        ("OPTIONS", _) => "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Private-Network: true\r\nAccess-Control-Allow-Headers: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
        (_, "/redirect") => format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/redirect-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        (_, "/frame" | "/object" | "/embed") => {
            let page = format!("<!doctype html><title>nested</title><script>new WebSocket('wss://127.0.0.1:{port}/nested');</script>");
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len())
        }
        _ => "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Private-Network: true\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned(),
    };
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.shutdown(Shutdown::Write);
}

fn serves(address: &str) -> bool {
    address.starts_with(SITE)
}

fn served(address: &str, ports: &[(String, String)]) -> Served {
    let name = address.strip_prefix(SITE).unwrap_or_default();
    let case = match name {
        "worker.js" => "worker",
        "shared-worker.js" => "shared-worker",
        other => other.strip_suffix(".html").unwrap_or(other),
    };
    let port = ports
        .iter()
        .find(|(named, _)| named == case)
        .map_or("0", |(_, port)| port.as_str());
    let script = |text: &str| Served {
        status: 200,
        mime: "text/javascript".to_owned(),
        body: text.replace("PORT", port).into_bytes(),
    };
    match name {
        "shared-worker.js" => script(SHARED_WORKER),
        "worker.js" => script(WORKER),
        "landing.html" => html("done('landed');", None, port),
        "keepalive-after.txt" => Served {
            status: 200,
            mime: "text/plain".to_owned(),
            body: b"ok".to_vec(),
        },
        "outline.html" => Served {
            status: 200,
            mime: "text/html; charset=utf-8".to_owned(),
            body: OUTLINE
                .replace("</html>", &format!("{SIZE}</html>"))
                .into_bytes(),
        },
        _ => match CASES
            .iter()
            .find(|case| name.strip_suffix(".html") == Some(case.name))
        {
            Some(case) => html(case.script, case.link, port),
            None => Served {
                status: 404,
                mime: "text/plain".to_owned(),
                body: Vec::new(),
            },
        },
    }
}

fn html(script: &str, link: Option<&str>, port: &str) -> Served {
    let link = link
        .map(|href| format!("<a href=\"{href}\" style=\"position: fixed; inset: 0\">away</a>"))
        .unwrap_or_default();
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>loading</title><body>{link}<script>const done = (what) => {{ document.title = 'done:' + what; }};\ndocument.addEventListener('securitypolicyviolation', (event) => done('policy:' + event.violatedDirective));\n{script}</script></body>"
    );
    Served {
        status: 200,
        mime: "text/html; charset=utf-8".to_owned(),
        body: body.replace("PORT", port).into_bytes(),
    }
}

fn left(url: &str) -> bool {
    !url.is_empty() && !url.starts_with("duck:")
}

fn foreign(title: &str) -> bool {
    let ours = title.is_empty()
        || title == "loading"
        || title == "ready"
        || title.starts_with("done:")
        || title.starts_with("size:");
    !ours
}

fn chosen(name: &str) -> bool {
    picked(&std::env::var(ONLY).unwrap_or_default(), name)
}

fn picked(only: &str, name: &str) -> bool {
    let named = only.split(',').any(|chosen| chosen == name);
    named || (only.is_empty() && ![CHROMIUM, PRECONNECT].contains(&name))
}

struct Embed {
    page: Entity<Page>,
    height: Rc<Cell<Option<u32>>>,
}

impl Render for Embed {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let height = self
            .height
            .get()
            .map_or(window.viewport_size().height, |height| px(height as f32));
        div().w_full().h(height).child(self.page.clone())
    }
}

struct Verdict {
    case: &'static str,
    title: String,
    reached: Vec<String>,
    missing: Option<String>,
}

impl Verdict {
    fn fenced(&self) -> bool {
        self.reached.is_empty() && self.missing.is_none()
    }

    fn say(&self) {
        let name = self.case;
        if !self.reached.is_empty() {
            println!("live {name}: REACHED: {}", self.reached.join("; "));
        }
        if let Some(why) = &self.missing {
            println!("live {name}: NOT SHOWN: {why} (title {:?})", self.title);
        }
        if self.fenced() {
            println!("live {name}: as it should (title {:?})", self.title);
        }
    }
}

struct Run {
    cx: TestAppContext,
    profile: PathBuf,
    heard: Heard,
    reads: Heard,
    leaves: Rc<RefCell<Vec<String>>>,
    followed: usize,
    current: &'static str,
    told: String,
    visual: Option<VisualTestContext>,
    refusal: Option<String>,
}

impl Run {
    fn watch(&mut self, page: &Entity<Page>) {
        let leaves = self.leaves.clone();
        self.cx.update(|cx| {
            cx.subscribe(page, move |_, leave: &Leave, _| {
                println!("live: the app was handed {}", leave.0);
                leaves.borrow_mut().push(leave.0.clone())
            })
            .detach()
        });
        assert!(
            !page.read_with(&self.cx, |page, _| page.unavailable()),
            "the web view did not start"
        );
    }

    fn open(&mut self, url: &str) -> (WindowHandle<Page>, Entity<Page>) {
        let profile = self.profile.clone();
        let address = url.to_owned();
        let window = self
            .cx
            .add_window(move |window, cx| Page::new(&address, profile, window, cx));
        let page = window.root(&mut self.cx).expect("the page in its window");
        self.watch(&page);
        (window, page)
    }

    fn state(&self, page: &Entity<Page>) -> PageState {
        page.read_with(&self.cx, |page, _| page.state().clone())
    }

    fn heard(&self, path: &str) -> bool {
        let seen = format!("{} {path}", self.current);
        self.heard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&seen)
    }

    fn read(&self, name: &str) -> bool {
        let address = format!("{SITE}{name}");
        self.reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&address)
    }

    fn look(&mut self) {
        if self.refusal.is_some() {
            return;
        }
        let Some(visual) = self.visual.as_mut() else {
            return;
        };
        self.refusal = visual.update(|window, cx| {
            window.activate_a11y();
            window.draw(cx).clear(cx);
            window.a11y_tree().and_then(|tree| {
                tree.nodes
                    .iter()
                    .find(|(_, node)| node.role() == Role::Status)
                    .and_then(|(_, node)| node.label().map(str::to_owned))
            })
        });
        if let Some(notice) = &self.refusal {
            println!("live: the page says {notice:?}");
        }
    }

    fn until(&mut self, page: &Entity<Page>, mut done: impl FnMut(&Self) -> bool) {
        loop {
            self.look();
            let state = self.state(page);
            let told = format!(
                "url {:?} title {:?} loading {} height {:?}",
                state.url, state.title, state.loading, state.height
            );
            if told != self.told {
                println!("live: the page is at {told}");
                self.told = told;
            }
            if done(self) {
                return;
            }
            self.cx.dispatcher.run_until_parked();
            self.cx.dispatcher.advance_clock_to_next_timer();
            self.follow();
        }
    }

    fn follow(&mut self) {
        let fresh = self.leaves.borrow()[self.followed..].to_vec();
        self.followed += fresh.len();
        for url in fresh {
            let web = url.starts_with("http://") || url.starts_with("https://");
            if web {
                self.open(&url);
            }
        }
    }

    fn close(&mut self, window: gpui_kit::AnyWindowHandle) {
        self.visual = None;
        let _ = self
            .cx
            .update_window(window, |_, window, _| window.remove_window());
        self.cx.dispatcher.run_until_parked();
    }

    fn moved(&self, page: &Entity<Page>, case: &Case, leaves: usize) -> bool {
        let state = self.state(page);
        state.title.starts_with("done:")
            || foreign(&state.title)
            || self.refusal.is_some()
            || left(&state.url)
            || case.reached.iter().any(|path| self.heard(path))
            || self.leaves.borrow().len() > leaves
    }

    fn case(&mut self, case: &Case) -> Verdict {
        println!("live {}: opens", case.name);
        self.current = case.name;
        let leaves = self.leaves.borrow().len();
        let address = format!("{SITE}{}.html", case.name);
        let (window, page) = self.open(&address);
        self.visual = Some(VisualTestContext::from_window(window.into(), &self.cx));
        self.refusal = None;
        match case.wait {
            Wait::Settled => self.until(&page, |run| run.state(&page).title.starts_with("done:")),
            Wait::Navigated => self.until(&page, |run| run.moved(&page, case, leaves)),
            Wait::Heard(path) => self.until(&page, |run| run.heard(path)),
            Wait::Clicked => {
                self.until(&page, |run| run.state(&page).title == "ready");
                if let Some(visual) = self.visual.as_mut() {
                    visual.update(|window, cx| {
                        window.draw(cx).clear(cx);
                    });
                    visual.simulate_click(Point::default(), Modifiers::none());
                }
                self.until(&page, |run| run.moved(&page, case, leaves));
            }
            Wait::Reloaded(after) => {
                self.until(&page, |run| run.state(&page).title == "ready");
                page.update(&mut self.cx, |page, _| page.reload());
                self.until(&page, |run| run.read(after));
            }
        }
        let state = self.state(&page);
        let mut reached: Vec<String> = case
            .reached
            .iter()
            .filter(|path| self.heard(path))
            .map(|path| format!("the listener heard {path}"))
            .collect();
        if foreign(&state.title) {
            reached.push(format!(
                "the piece showed another document, {:?}",
                state.title
            ));
        }
        if left(&state.url) {
            reached.push(format!("the page went to {}", state.url));
        } else {
            let moved = !state.url.is_empty() && state.url != address;
            if moved {
                reached.push(format!("the piece moved to {}", state.url));
            }
        }
        let handed = self.leaves.borrow()[leaves..].to_vec();
        let control = match case.shown {
            Shown::Left(url) => Some(url),
            _ => None,
        };
        reached.extend(
            handed
                .iter()
                .filter(|url| Some(url.as_str()) != control)
                .map(|url| format!("the app was handed {url}")),
        );
        let missing = match case.shown {
            Shown::Always => None,
            Shown::Heard(path) => {
                (!self.heard(path)).then(|| format!("the listener never heard {path}"))
            }
            Shown::Left(url) => (!handed.iter().any(|handed| handed == url))
                .then(|| format!("the app was never handed {url}")),
            Shown::Ran => {
                (state.title == "done:no-worker").then(|| "the worker never ran".to_owned())
            }
            Shown::Refused(directive) => (state.title != format!("done:policy:{directive}"))
                .then(|| format!("the page was never refused by {directive}")),
        };
        let verdict = Verdict {
            case: case.name,
            title: state.title,
            reached,
            missing,
        };
        verdict.say();
        self.close(window.into());
        verdict
    }

    fn outline(&mut self) -> Verdict {
        println!("live outline: opens");
        let profile = self.profile.clone();
        let kept = Rc::new(Cell::new(None));
        let window = self.cx.add_window(move |window, cx| {
            let told = kept.clone();
            let page = cx.new(|cx| Page::new(&format!("{SITE}outline.html"), profile, window, cx));
            cx.subscribe(&page, move |_, _, state: &PageState, cx| {
                if told.replace(state.height) != state.height {
                    cx.notify();
                }
            })
            .detach();
            Embed { page, height: kept }
        });
        let embed = window.root(&mut self.cx).expect("the embed in its window");
        let page = embed.read_with(&self.cx, |embed, _| embed.page.clone());
        self.watch(&page);
        self.visual = Some(VisualTestContext::from_window(window.into(), &self.cx));
        self.refusal = None;
        self.until(&page, |run| {
            let state = run.state(&page);
            state
                .height
                .is_some_and(|height| state.title == format!("size:{height}:{height}"))
        });
        let state = self.state(&page);
        Verdict {
            case: "outline",
            title: state.title,
            reached: Vec::new(),
            missing: None,
        }
    }
}

fn inner() -> i32 {
    let ports: Vec<(String, String)> = std::env::var(PORTS)
        .unwrap_or_default()
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, port)| (name.to_owned(), port.to_owned()))
        .collect();
    let report = std::env::var(REPORT).unwrap_or_default();
    let stream = TcpStream::connect(format!("127.0.0.1:{report}")).expect("the listeners' report");
    let mut lines = BufReader::new(stream).lines();
    let ready = lines.next().and_then(Result::ok);
    assert_eq!(ready.as_deref(), Some("ready"), "the listeners' first word");
    let heard = Heard::default();
    std::thread::spawn({
        let heard = heard.clone();
        move || {
            for line in lines.map_while(Result::ok) {
                heard
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(line);
            }
        }
    });
    let reads = Heard::default();
    let log = reads.clone();
    ducktape_web::serve(Network {
        read: Box::new(move |address, answer| {
            let served = served(&address, &ports);
            log.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(address);
            answer(served)
        }),
        serves,
    });
    let cx = TestAppContext::single();
    cx.executor().allow_parking();
    cx.update(gpui_kit::init);
    let root = std::env::temp_dir().join(format!("ducktape-web-live-{}", std::process::id()));
    let mut run = Run {
        cx,
        profile: root.join("profile"),
        heard,
        reads,
        leaves: Rc::default(),
        followed: 0,
        current: "",
        told: String::new(),
        visual: None,
        refusal: None,
    };
    let mut verdicts: Vec<Verdict> = CASES
        .iter()
        .filter(|case| chosen(case.name))
        .map(|case| run.case(case))
        .collect();
    if chosen("outline") {
        verdicts.push(run.outline());
    }
    println!("live: every case");
    for verdict in &verdicts {
        verdict.say();
    }
    let all = verdicts.iter().all(Verdict::fenced);
    ducktape_web::shutdown();
    let _ = std::fs::remove_dir_all(&root);
    i32::from(!all)
}
