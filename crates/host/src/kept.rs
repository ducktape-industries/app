//! A record per network, by chain id, kept in one JSON file under the config folder: read once as
//! the shell starts, and written whole off the window thread on every change, the newest waiting
//! while one is written.
use gpui_kit::App;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[cfg(test)]
mod tests;

/// Each network's record of `R`, by chain id, as one JSON file keeps them.
pub struct Keeper<R> {
    path: Option<PathBuf>,
    networks: BTreeMap<String, R>,
    last: Option<(u64, Vec<u8>)>,
    queued: Arc<Mutex<Queued>>,
    written: Arc<Mutex<u64>>,
}

#[derive(Default)]
struct Queued {
    seq: u64,
    bytes: Option<Vec<u8>>,
    writing: bool,
}

impl<R: Serialize + DeserializeOwned + PartialEq> Keeper<R> {
    pub fn load(file: &str) -> Self {
        Self::at(crate::backend::dirs().config.join(file))
    }

    pub fn at(path: PathBuf) -> Self {
        let read = crate::backend::read_or_set_aside(&path, |bytes| {
            serde_json::from_slice::<BTreeMap<String, R>>(bytes)
        });
        let (path, networks) = match read {
            Ok(networks) => (Some(path), networks.unwrap_or_default()),
            Err(error) => {
                tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "a kept record is unreadable and nothing is kept in it");
                (None, BTreeMap::new())
            }
        };
        Self {
            path,
            networks,
            last: None,
            queued: Arc::default(),
            written: Arc::default(),
        }
    }

    pub fn of(&self, chain: &str) -> Option<&R> {
        self.networks.get(chain)
    }

    pub fn keep(&mut self, chain: &str, record: R, cx: &App) {
        if self.networks.get(chain) == Some(&record) {
            return;
        }
        self.networks.insert(chain.to_owned(), record);
        let Some(path) = self.path.clone() else {
            return;
        };
        let bytes = match serde_json::to_vec_pretty(&self.networks) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "a record was not kept");
                return;
            }
        };
        let mut queued = lock(&self.queued);
        queued.seq += 1;
        queued.bytes = Some(bytes.clone());
        self.last = Some((queued.seq, bytes));
        if std::mem::replace(&mut queued.writing, true) {
            return;
        }
        drop(queued);
        let (queued, written) = (self.queued.clone(), self.written.clone());
        cx.background_executor()
            .spawn(async move {
                while let Some((seq, bytes)) = next(&queued) {
                    write(&path, &written, seq, &bytes);
                }
            })
            .detach();
    }

    pub fn flush(&self) {
        if let (Some(path), Some((seq, bytes))) = (&self.path, &self.last) {
            write(path, &self.written, *seq, bytes);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn next(queued: &Mutex<Queued>) -> Option<(u64, Vec<u8>)> {
    let mut queued = lock(queued);
    let seq = queued.seq;
    let taken = queued.bytes.take().map(|bytes| (seq, bytes));
    queued.writing = taken.is_some();
    taken
}

fn write(path: &Path, written: &Mutex<u64>, seq: u64, bytes: &[u8]) {
    let mut at = lock(written);
    if seq <= *at {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match noded_client::fs::atomic_write(path, bytes, false) {
        Ok(()) => *at = seq,
        Err(error) => {
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "a record was not kept")
        }
    }
}
