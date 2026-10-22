//! Web pages in a gpui window through CEF off-screen rendering: a [`Page`]
//! is one browser, drawn as a gpui view, that shows an address, tells its
//! owner where it is ([`PageState`], as a gpui event) and takes back,
//! forward, reload and stop. Nothing here knows of views or the wire.
//!
//! Shape:
//! - subprocesses are the app's own binary: [`child`] is the first thing in
//!   `main`, before anything of the app starts.
//! - CEF starts on the first page, not at app start. On Linux it runs in its
//!   setuid sandbox when `chrome-sandbox` beside the binary is root's and
//!   setuid, and without one otherwise: releases ship no helper.
//!   macOS: CEF's framework sits beside the binary (`build.rs` links it
//!   there) or in the bundle's `Frameworks/` beside its helper apps, the app
//!   loads it before any CEF call (`mac.rs`), and each child enters its
//!   seatbelt sandbox first.
//!   CEF's signal handlers are off: a SIGTERM is the app's to handle.
//! - the message loop is CEF's external pump: `on_schedule_message_pump_work`
//!   (any CEF thread) sends its delay down a channel; one gpui foreground
//!   task waits for it (or 33 ms) and calls `do_message_loop_work` on the
//!   main thread, so every CEF callback runs on gpui's thread. Callbacks
//!   only write into a page's [`page::Shared`]; the pump hands the page its
//!   frame, its state and its browser after the work returns.
//! - every page loads only what [`loadable`] allows, its own navigations
//!   included; popups, downloads and permission prompts are refused.
//! - a `duck://` address is answered by what the app hands [`serve`]
//!   (`scheme.rs`), never by the web.
//! - [`shutdown`] after gpui returns: browsers closed, then CEF.
//!
//! Without feature `cef` the crate is empty (`Cargo.toml` says why).
#![cfg(feature = "cef")]

mod a11y;
mod cursor;
#[cfg(target_os = "linux")]
mod glib;
mod handlers;
mod keys;
#[cfg(target_os = "macos")]
mod mac;
mod menu;
mod page;
mod scheme;

pub use page::{Leave, Loaded, Page, PageState};

pub fn color_scheme(dark: bool) {
    ENGINE.with_borrow_mut(|engine| {
        if engine.dark == Some(dark) {
            return;
        }
        engine.dark = Some(dark);
        for (_, shared) in &engine.pages {
            if let Some(host) = shared.host() {
                handlers::scheme(&host, dark);
            }
        }
    });
}

fn dark() -> Option<bool> {
    ENGINE.with_borrow(|engine| engine.dark)
}
pub use scheme::{Answer, Network, SCHEME, Served, serve};

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{App, AsyncApp, SharedString, WeakEntity};

/// The pump's inbox: `on_schedule_message_pump_work` delays, from any thread.
static PUMP: OnceLock<mpsc::UnboundedSender<i64>> = OnceLock::new();
/// `on_context_initialized` has run: browsers may be created.
static READY: AtomicBool = AtomicBool::new(false);
/// Longest wait between two `do_message_loop_work` calls (cefclient's 30 fps).
const MAX_PUMP_DELAY: Duration = Duration::from_millis(33);

/// `address`'s origin as an address (`https://host:port/`), when it has
/// one (not `data:` or `about:`): what a page's address is cut back to when
/// the whole one cannot be told, as it names the same site.
pub fn origin(address: &str) -> Option<String> {
    let origin = url::Url::parse(address).ok()?.origin();
    origin
        .is_tuple()
        .then(|| format!("{}/", origin.ascii_serialization()))
}

/// Whether a page may load `address`: `https://` or `duck://` with a host, or `http://`
/// on this machine (`localhost`, a loopback address), never with a user
/// name or password, no control characters.
/// Read with the WHATWG parser, as Chromium reads it, so the host checked
/// is the host loaded. The one rule every load and every navigation of a
/// page is held to.
pub fn loadable(address: &str) -> bool {
    if address.contains(char::is_control) {
        return false;
    }
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => true,
        ("duck", Some(url::Host::Domain(name))) => !name.is_empty() && url.port().is_none(),
        ("http", Some(url::Host::Domain(name))) => name == "localhost",
        ("http", Some(url::Host::Ipv4(ip))) => ip.is_loopback(),
        ("http", Some(url::Host::Ipv6(ip))) => ip.is_loopback(),
        _ => false,
    }
}

pub fn private(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.');
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => !reachable_v4(ip),
        Some(url::Host::Ipv6(ip)) => !reachable_v6(ip),
        None => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GloballyReachable {
    Yes,
    No,
}

const IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY: &[(Ipv4Addr, u32, GloballyReachable)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8, GloballyReachable::No),
    (Ipv4Addr::new(10, 0, 0, 0), 8, GloballyReachable::No),
    (Ipv4Addr::new(100, 64, 0, 0), 10, GloballyReachable::No),
    (Ipv4Addr::new(127, 0, 0, 0), 8, GloballyReachable::No),
    (Ipv4Addr::new(169, 254, 0, 0), 16, GloballyReachable::No),
    (Ipv4Addr::new(172, 16, 0, 0), 12, GloballyReachable::No),
    (Ipv4Addr::new(192, 0, 0, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(192, 0, 0, 9), 32, GloballyReachable::Yes),
    (Ipv4Addr::new(192, 0, 0, 10), 32, GloballyReachable::Yes),
    (Ipv4Addr::new(192, 0, 2, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(192, 88, 99, 2), 32, GloballyReachable::No),
    (Ipv4Addr::new(192, 168, 0, 0), 16, GloballyReachable::No),
    (Ipv4Addr::new(198, 18, 0, 0), 15, GloballyReachable::No),
    (Ipv4Addr::new(198, 51, 100, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(203, 0, 113, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(240, 0, 0, 0), 4, GloballyReachable::No),
    (Ipv4Addr::new(255, 255, 255, 255), 32, GloballyReachable::No),
];

const IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY: &[(Ipv6Addr, u32, GloballyReachable)] = &[
    (
        Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0),
        128,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
        128,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0),
        48,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0),
        64,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x100, 0, 0, 1, 0, 0, 0, 0),
        64,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0),
        23,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, 1),
        128,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, 2),
        128,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, 3),
        128,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 3, 0, 0, 0, 0, 0, 0),
        32,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 4, 0x112, 0, 0, 0, 0, 0),
        48,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 0x20, 0, 0, 0, 0, 0, 0),
        28,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 0x30, 0, 0, 0, 0, 0, 0),
        28,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0),
        32,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0),
        20,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x5f00, 0, 0, 0, 0, 0, 0, 0),
        16,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0),
        7,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0),
        10,
        GloballyReachable::No,
    ),
];

const RFC_6052_WELL_KNOWN_PREFIX: Ipv6Addr = Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0);

fn reachable_v4(ip: Ipv4Addr) -> bool {
    let bits = u32::from(ip);
    IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY
        .iter()
        .filter(|(net, len, _)| bits >> (32 - len) == u32::from(*net) >> (32 - len))
        .max_by_key(|(_, len, _)| *len)
        .is_none_or(|(_, _, reachable)| *reachable == GloballyReachable::Yes)
}

fn reachable_v6(ip: Ipv6Addr) -> bool {
    if let Some(ip) = ip.to_ipv4_mapped() {
        return reachable_v4(ip);
    }
    let bits = u128::from(ip);
    if bits >> 32 == u128::from(RFC_6052_WELL_KNOWN_PREFIX) >> 32 {
        return reachable_v4(Ipv4Addr::from(bits as u32));
    }
    IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY
        .iter()
        .filter(|(net, len, _)| bits >> (128 - len) == u128::from(*net) >> (128 - len))
        .max_by_key(|(_, len, _)| *len)
        .is_none_or(|(_, _, reachable)| *reachable == GloballyReachable::Yes)
}

/// A CEF child process (zygote, renderer, GPU, utility: this binary started
/// with `--type=...`): runs it and returns its exit code. `None` in the
/// app's own process, which goes on to start the app.
pub fn child() -> Option<i32> {
    if !std::env::args().any(|arg| arg.starts_with("--type=")) {
        return None;
    }
    let args = cef::args::Args::new();
    #[cfg(target_os = "macos")]
    if !mac::sandbox(args.as_main_args()) || !mac::load() {
        return Some(1);
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    Some(cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut handlers::app()),
        std::ptr::null_mut(),
    ))
}

/// Closes every browser and shuts CEF down, if a page ever started it.
/// After gpui's run returns, on the main thread.
pub fn shutdown() {
    let pages = ENGINE.with_borrow(|engine| match engine.status {
        Status::Running => Some(engine.pages.clone()),
        _ => None,
    });
    let Some(pages) = pages else { return };
    for (_, shared) in &pages {
        shared.close();
    }
    while pages.iter().any(|(_, shared)| shared.open()) {
        cef::do_message_loop_work();
        std::thread::sleep(Duration::from_millis(10));
    }
    for (_, shared) in &pages {
        shared.forget();
    }
    ENGINE.with_borrow_mut(|engine| {
        engine.pages.clear();
        engine.contexts.clear();
        engine.status = Status::Down;
    });
    cef::shutdown();
}

#[derive(Default)]
enum Status {
    #[default]
    Idle,
    Running,
    /// Why no page can show: said by every page.
    Refused(SharedString),
    Down,
}

/// The engine, on the main thread: whether CEF runs, the pages it pumps,
/// one request context (cookies, cache) per profile directory.
#[derive(Default)]
struct Engine {
    status: Status,
    pages: Vec<(WeakEntity<Page>, Rc<page::Shared>)>,
    contexts: HashMap<PathBuf, cef::RequestContext>,
    dark: Option<bool>,
}

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::default();
}

/// CEF up, once, for a page whose profiles live under `root`: `Ok` while
/// it runs, else why there is no web view.
fn start(root: &Path, cx: &mut App) -> Result<(), SharedString> {
    let status = ENGINE.with_borrow(|engine| match &engine.status {
        Status::Idle => None,
        Status::Running => Some(Ok(())),
        Status::Refused(reason) => Some(Err(reason.clone())),
        Status::Down => Some(Err("The web view has shut down.".into())),
    });
    if let Some(status) = status {
        return status;
    }
    // CEF may ask for work while it initializes
    let (sender, inbox) = mpsc::unbounded();
    let _ = PUMP.set(sender);
    let outcome = initialize(root);
    ENGINE.with_borrow_mut(|engine| {
        engine.status = match &outcome {
            Ok(()) => Status::Running,
            Err(reason) => Status::Refused(reason.clone()),
        }
    });
    if outcome.is_ok() {
        keys::bind(cx);
        #[cfg(target_os = "linux")]
        gpui_linux::set_clipboard_wait(glib::turn);
        cx.spawn(async move |cx: &mut AsyncApp| pump(inbox, cx).await)
            .detach();
    }
    outcome
}

fn initialize(root: &Path) -> Result<(), SharedString> {
    let sandbox = sandbox_helper_installed();
    #[cfg(target_os = "macos")]
    if !mac::load() {
        return Err("The web view's framework is not beside the app.".into());
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let _ = std::fs::create_dir_all(root);
    if !handlers::initialize(sandbox, root) {
        return Err(format!(
            "The web view did not start (CEF exit code {}).",
            cef::get_exit_code()
        )
        .into());
    }
    Ok(())
}

/// Linux: the setuid helper CEF's sandbox runs through, beside the binary,
/// root's with the setuid bit; without it CEF runs with no sandbox.
#[cfg(target_os = "linux")]
fn sandbox_helper_installed() -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::env::current_exe()
        .map(|exe| exe.with_file_name("chrome-sandbox"))
        .and_then(std::fs::metadata)
        .is_ok_and(|helper| helper.uid() == 0 && helper.mode() & 0o4000 != 0)
}

#[cfg(not(target_os = "linux"))]
fn sandbox_helper_installed() -> bool {
    true
}

/// The external pump: `do_message_loop_work` when CEF asks (after its
/// delay, capped) or every [`MAX_PUMP_DELAY`]; then each page takes what
/// its callbacks left (`Page::pumped`). Ends at [`shutdown`].
async fn pump(mut inbox: mpsc::UnboundedReceiver<i64>, cx: &mut AsyncApp) {
    let mut due = Instant::now();
    loop {
        let wait = due.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            let timer = cx.background_executor().timer(wait).fuse();
            futures::pin_mut!(timer);
            futures::select_biased! {
                delay = inbox.next() => {
                    let Some(delay) = delay else { return };
                    let delay = Duration::from_millis(u64::try_from(delay).unwrap_or(0))
                        .min(MAX_PUMP_DELAY);
                    due = due.min(Instant::now() + delay);
                    continue;
                }
                _ = timer => {}
            }
        }
        let pages = ENGINE.with_borrow_mut(|engine| {
            // a closed browser leaves; a page not yet made stays while it lives
            engine.pages.retain(|(page, shared)| {
                shared.open() || (!shared.started() && page.upgrade().is_some())
            });
            matches!(engine.status, Status::Running).then(|| engine.pages.clone())
        });
        let Some(pages) = pages else { return };
        #[cfg(target_os = "linux")]
        glib::turn();
        cef::do_message_loop_work();
        due = Instant::now() + MAX_PUMP_DELAY;
        let ready = READY.load(Ordering::SeqCst);
        for (page, shared) in pages {
            if ready {
                shared.create(context);
            }
            let _ = page.update(cx, |page, cx| page.pumped(cx));
        }
    }
}

/// Asks the pump for work in `delay_ms` (any thread).
fn schedule(delay_ms: i64) {
    if let Some(pump) = PUMP.get() {
        let _ = pump.unbounded_send(delay_ms);
    }
}

/// The request context of `profile`: its own cookies and cache, made the
/// first time a page of it opens.
fn context(profile: &Path, fenced: bool) -> Option<cef::RequestContext> {
    let profile = match fenced {
        true => {
            let mut dir = profile.as_os_str().to_owned();
            dir.push(".artifact");
            PathBuf::from(dir)
        }
        false => profile.to_owned(),
    };
    ENGINE.with_borrow_mut(|engine| {
        if let Some(context) = engine.contexts.get(&profile) {
            return Some(context.clone());
        }
        let context = handlers::context(&profile, fenced)?;
        engine.contexts.insert(profile, context.clone());
        Some(context)
    })
}

/// A page joins the pump.
fn register(page: WeakEntity<Page>, shared: Rc<page::Shared>) {
    ENGINE.with_borrow_mut(|engine| engine.pages.push((page, shared)));
}

#[cfg(test)]
mod tests {
    use super::{color_scheme, dark, loadable, origin, private};

    #[test]
    fn an_artifact_reaches_no_address_iana_marks_not_globally_reachable() {
        for address in [
            "http://127.0.0.1:8765/",
            "https://127.0.0.2/",
            "http://2130706433/",
            "http://localhost/",
            "http://LOCALHOST./",
            "http://app.localhost/",
            "ws://localhost:9222/devtools",
            "wss://127.0.0.1/",
            "http://0.0.0.0/",
            "http://0/",
            "http://10.1.2.3/",
            "http://100.64.0.1/",
            "http://100.100.100.100/",
            "http://100.127.255.254/",
            "http://169.254.169.254/latest/meta-data/",
            "http://172.16.0.1/",
            "http://172.31.255.255/",
            "http://192.0.0.8/",
            "http://192.0.0.170/",
            "http://192.0.2.1/",
            "http://192.88.99.2/",
            "http://192.168.1.1/",
            "http://198.18.0.1/",
            "http://198.19.255.255/",
            "http://198.51.100.7/",
            "http://203.0.113.7/",
            "http://240.0.0.1/",
            "http://255.255.255.255/",
            "http://[::1]:8000/",
            "http://[::]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:192.168.0.1]/",
            "http://[64:ff9b::7f00:1]/",
            "http://[64:ff9b:1::1]/",
            "http://[100::1]/",
            "http://[100:0:0:1::1]/",
            "http://[2001:2::1]/",
            "https://[2001:db8::1]/",
            "https://[3fff::1]/",
            "https://[5f00::1]/",
            "https://[fc00::1]/",
            "https://[fd12:3456::1]/",
            "https://[fe80::1]/",
        ] {
            assert!(private(address), "{address}");
        }
        for address in [
            "duck://duckpond-725bcad1/files/shared/plan.html",
            "duck://duckpond-725bcad1/blobs/00ff",
            "https://example.com/",
            "https://localhost.example.com/",
            "http://8.8.8.8/",
            "http://100.63.255.255/",
            "http://100.128.0.0/",
            "http://172.15.255.255/",
            "http://172.32.0.1/",
            "http://192.0.0.9/",
            "http://192.0.0.10/",
            "http://192.31.196.1/",
            "http://192.169.0.1/",
            "http://198.20.0.1/",
            "https://[2001:1::1]/",
            "https://[2001:3::1]/",
            "https://[2001:4860:4860::8888]/",
            "https://[64:ff9b::808:808]/",
            "https://[fec0::1]/",
            "data:text/html,hi",
            "about:blank",
            "",
        ] {
            assert!(!private(address), "{address}");
        }
    }

    #[test]
    fn a_page_made_after_a_theme_switch_is_told_the_theme_now() {
        assert_eq!(dark(), None, "no theme told before the app tells one");
        color_scheme(true);
        assert_eq!(dark(), Some(true));
        color_scheme(false);
        assert_eq!(dark(), Some(false), "the switch, not the first theme");
        color_scheme(false);
        assert_eq!(dark(), Some(false));
    }

    #[test]
    fn an_origin_names_the_site_and_nothing_else() {
        let long = format!("https://example.com/{}", "a".repeat(4096));
        for (address, expected) in [
            (long.as_str(), Some("https://example.com/")),
            (
                "https://example.com:8443/a?b#c",
                Some("https://example.com:8443/"),
            ),
            ("https://example.com:443/a", Some("https://example.com/")),
            (
                "https://user:secret@example.com/a",
                Some("https://example.com/"),
            ),
            (
                "https://한글.example/",
                Some("https://xn--bj0bj06e.example/"),
            ),
            ("http://[::1]:8000/x", Some("http://[::1]:8000/")),
            ("data:text/plain,hello", None),
            ("about:blank", None),
            ("", None),
        ] {
            assert_eq!(origin(address).as_deref(), expected, "{address}");
        }
    }

    #[test]
    fn only_https_and_this_machines_http_load() {
        for address in [
            "https://example.com",
            "https://ko.wikipedia.org/wiki/한글",
            "https://example.com:8443/a?b#c",
            "http://127.0.0.1:8765/page.html",
            "http://127.0.0.2/",
            "http://localhost/",
            "http://localhost",
            "http://[::1]:8000/",
            // `\` is `/` in an http address: this is localhost's path
            "http://localhost\\@evil.com/",
            "duck://duckpond-725bcad1/files/shared/plan.html",
            "duck://duckpond-725bcad1/blobs/00ff",
        ] {
            assert!(loadable(address), "{address}");
        }
        for address in [
            "",
            "https://",
            "example.com",
            "http://example.com",
            // user info: the host is evil.com
            "http://localhost:80@evil.com/",
            "http://127.0.0.1@evil.com/",
            "https://user:secret@example.com/",
            "https://user@example.com/",
            // `\` ends the host: evil.com
            "http://evil.com\\@localhost/",
            "http://evil.com\\.localhost/",
            "http://localhost.evil.com/",
            "http://127.0.0.1.nip.io/",
            "http://[::2]/",
            // not http(s), whatever follows
            "view-source:https://example.com/",
            "view-source:http://localhost/",
            // an IPv4-mapped loopback is no IPv6 loopback
            "http://[::ffff:127.0.0.1]/",
            // every address of this machine, and of every other
            "http://0.0.0.0/",
            "http://0/",
            // Cyrillic `о`: the parser makes it punycode, not localhost
            "http://l\u{43e}calhost/",
            "http://xn--lcalhost-nbh/",
            // a name under localhost is the resolver's to place
            "http://foo.localhost/",
            "http://localhost./",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:blank",
            "chrome://settings",
            "ftp://localhost/",
            "https://example.com/\n",
            "duck:///files/plan.html",
            "duck://user@duckpond-725bcad1/files/plan.html",
            "duck://duckpond-725bcad1:80/files/plan.html",
        ] {
            assert!(!loadable(address), "{address}");
        }
        let long = |n: usize| format!("https://example.com/{}", "a".repeat(n));
        assert!(
            loadable(&long(1 << 20)),
            "a long address is the engine's to refuse"
        );
    }
}
