mod a11y;
mod ui;
pub(crate) use ui::*;

mod ax;
mod backend;
mod editor;
mod fonts;
mod render;
mod runtime;
mod shell;
mod tray;

fn main() {
    #[cfg(debug_assertions)]
    if std::env::args().nth(1).as_deref() == Some("--render-tree") {
        shell::render_tree_fixture();
        return;
    }
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            let build = option_env!("DUCKTAPE_APP_BUILD").unwrap_or("unknown");
            println!("ducktape-app {}+{build}", env!("CARGO_PKG_VERSION"));
            return;
        }
        // the test door's client: talks to a running app, opens nothing
        Some("ax") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            std::process::exit(ax::cli(&args));
        }
        Some("--help" | "-h") => {
            println!("usage: ducktape-app [--version]");
            return;
        }
        _ => {}
    }
    install_log();
    raise_open_file_limit();
    // no view ships with the app: every one comes off the connected node.
    // A view developer's DUCKTAPE_VIEWS_DIR supplies files in their place,
    // and app.log says so for each one it supplies
    runtime::override_views_from(std::env::var_os("DUCKTAPE_VIEWS_DIR").map(Into::into));
    shell::run();
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
/// panic hook that lands in it. `RUST_LOG` ADDS to `info`; no home, no
/// file: the events go nowhere rather than a GUI refusing to start.
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
