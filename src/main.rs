// the host core, shared with the phone: `crate::render`, `crate::runtime`,
// ... name its modules, so the shell reads as it did when they were here
// the AX test door: in a build with the `ax-door` feature only (kit and qa),
// and in the tests, which read the trees it reads
#[cfg(any(test, feature = "ax-door"))]
use ducktape_host::ax;
use ducktape_host::{a11y, fonts, perf, render, runtime};
mod backend;
mod os;
mod shell;
mod tray;
mod ui;
mod update;

fn main() {
    // a CEF child process (feature `web`) is this binary with `--type=...`
    render::web::subprocess();
    #[cfg(debug_assertions)]
    if std::env::args().nth(1).as_deref() == Some("--render-tree") {
        shell::render_tree_fixture();
        return;
    }
    // the view developer's window: parsed before the log, so a usage
    // error says so without an app.log
    #[cfg(debug_assertions)]
    let live = (std::env::args().nth(1).as_deref() == Some("--live")).then(shell::Live::parse);
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            let build = option_env!("DUCKTAPE_APP_BUILD").unwrap_or("unknown");
            println!("ducktape-app {}+{build}", env!("CARGO_PKG_VERSION"));
            return;
        }
        // the test door's client: talks to a running app, opens nothing
        #[cfg(feature = "ax-door")]
        Some("ax") => {
            let _ = install_os();
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(ax::cli(&args));
        }
        // a build without the door says so rather than launching the app
        #[cfg(not(feature = "ax-door"))]
        Some("ax") => {
            eprintln!("ax: this build has no AX door (it is built with the `ax-door` feature)");
            std::process::exit(2);
        }
        // lay down a self-updating install of this binary
        Some("install") => std::process::exit(update::install_cli()),
        // a view's compile, in a child so its memory leaves with it
        Some(ducktape_host::runner::COMPILE_APART) => {
            let args: Vec<std::ffi::OsString> = std::env::args_os().skip(2).collect();
            std::process::exit(ducktape_host::runner::compile_child(&args));
        }
        // "Restart to update"'s waiter: starts the app once this one is gone
        Some(update::RESTART_WAITER) => {
            install_log();
            std::process::exit(update::restart_waiter());
        }
        Some("--help" | "-h") => {
            println!("usage: ducktape-app [--version | --help]");
            println!("       ducktape-app install  (install this binary; it then updates itself)");
            #[cfg(feature = "ax-door")]
            println!("       ducktape-app ax <command...>  (the AX door's client)");
            #[cfg(debug_assertions)]
            println!("       ducktape-app --render-tree  (debug build: the render-tree fixture)");
            #[cfg(debug_assertions)]
            println!(
                "       ducktape-app --live <endpoint> <program-id> <seed>  (debug build: one view on a node)"
            );
            return;
        }
        _ => {}
    }
    let os = install_os();
    // a call's picture codec on Linux is Cisco's binary: where it is
    #[cfg(all(feature = "call", target_os = "linux"))]
    backend::openh264::supply();
    // the sampler reads the OS facts: after them
    perf::start();
    install_log();
    perf::mark("log");
    // the self-update's boot step: flip to a staged release, or back from
    // one that never came up, and exec the release that should run
    update::boot();
    raise_open_file_limit();
    #[cfg(debug_assertions)]
    if let Some(live) = live {
        shell::run_live(live, os.clone());
        render::web::shutdown();
        return;
    }
    shell::run(os);
    render::web::shutdown();
}

/// The desktop's OS services into the host core, before anything reads a
/// directory (the host's `start` takes the same services again, as its
/// `Boot`). No home to put them in is a refusal with the sentence that
/// names the variable to set, not a GUI that starts and finds nothing.
fn install_os() -> std::sync::Arc<dyn ducktape_host::contract::OsServices> {
    match os::DesktopOs::new() {
        Ok(os) => {
            let os: std::sync::Arc<dyn ducktape_host::contract::OsServices> =
                std::sync::Arc::new(os);
            ducktape_host::contract::install(os.clone());
            os
        }
        Err(error) => {
            eprintln!("ducktape-app: {error}");
            std::process::exit(2);
        }
    }
}

/// macOS launches a GUI with a 256-fd soft limit; the app's stores and
/// sockets hit that as a bare EMFILE.
fn raise_open_file_limit() {
    // SAFETY: plain libc calls on a stack-local rlimit.
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        let wanted = limit.rlim_max.min(8192);
        if limit.rlim_cur >= wanted {
            return;
        }
        limit.rlim_cur = wanted;
        let raised = libc::setrlimit(libc::RLIMIT_NOFILE, &limit) == 0;
        tracing::info!(target: "ducktape::app", soft_limit = wanted, raised, "open-file limit");
    }
}

/// The app's sink is `app.log` in its platform state directory, plus a
/// panic hook that lands in it. `RUST_LOG` ADDS to `info`. The GUI has a
/// home by now (`install_os` refused without one); the restart waiter, which
/// has no OS services, logs nowhere when it has none.
fn install_log() {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    use tracing_subscriber::{EnvFilter, Layer as _};

    let env = std::env::var("RUST_LOG").unwrap_or_default();
    let filter = EnvFilter::builder()
        .parse(format!("info,{env}"))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let file = backend::app_log_path().ok().and_then(|path| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let file_layer = file.map(|file| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
    });
    let _ = tracing_subscriber::registry()
        .with(file_layer.with_filter(filter))
        .try_init();
    install_panic_hook();
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()))
            .unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            target: "ducktape::app",
            event = "app_panic",
            thread = std::thread::current().name().unwrap_or("?"),
            payload = info.payload_as_str().unwrap_or("non-string panic payload"),
            location,
            backtrace = %backtrace,
            "panicked at: {info}"
        );
        default(info);
    }));
}
