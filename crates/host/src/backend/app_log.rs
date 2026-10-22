//! The app's log, one format on every shell: tracing's lines at `info` and what `RUST_LOG` adds,
//! without colour, and a panic's place and backtrace among them as `app_panic`.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer};

#[derive(Clone, Debug)]
pub struct Launches {
    this: PathBuf,
    previous: PathBuf,
}

impl Launches {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            this: dir.join("app.log"),
            previous: dir.join("app.previous.log"),
        }
    }

    pub fn begin(&self) -> io::Result<File> {
        if let Some(dir) = self.this.parent() {
            std::fs::create_dir_all(dir)?;
        }
        match std::fs::rename(&self.this, &self.previous) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        File::create(&self.this)
    }

    pub fn copy_to(&self, to: &Path) -> io::Result<()> {
        let mut copy = File::create_new(to)?;
        for path in [&self.previous, &self.this] {
            match File::open(path) {
                Ok(mut log) => {
                    io::copy(&mut log, &mut copy)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        copy.sync_all()
    }
}

pub fn install(file: Option<File>, stderr: bool) {
    let to_stderr = stderr.then(|| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(io::stderr)
            .with_filter(filter())
    });
    let _ = tracing_subscriber::registry()
        .with(file.map(file_layer))
        .with(to_stderr)
        .try_init();
    install_panic_hook();
}

fn filter() -> EnvFilter {
    let env = std::env::var("RUST_LOG").unwrap_or_default();
    EnvFilter::builder()
        .parse(format!("info,{env}"))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

fn file_layer<S>(file: File) -> impl Layer<S>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .with_filter(filter())
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

#[cfg(test)]
mod tests;
