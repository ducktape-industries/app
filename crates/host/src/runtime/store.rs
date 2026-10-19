//! `store.get` / `store.set` / `store.list`: what a view keeps on this
//! device between runs.
//!
//! One file per program per network per seated key,
//! `<config>/store/<chain>/<key>/<program>.borsh`, `<key>` the session's
//! `signer` (hex) as the view is handed it (`roster::props`); with no key
//! seated, the device's own `<config>/store/<chain>/<program>.borsh` (a
//! recipient's notifier's under its own store, `notifier::Recipient`): a
//! borsh map of key → bytes, rewritten whole through
//! `noded_client::fs::atomic_write`. A key's file that is not there yet
//! starts as a copy of the device's, on the first request that reads it,
//! so what was kept before files were per key stays. A view names keys,
//! never paths — the chain, the seated key and the program come from the
//! host — so no view reaches another view's file, another network's or
//! another key's.
//!
//! The file is read and written off the window thread, one request after
//! another in the order the view sent them (`kernel::in_order` under the
//! file's path), so a `get` reads the `set` before it.
//!
//! The file is the program's, shared by every instance of it on the device
//! running with the same key (its panes, its notifier): `store.changes`
//! tells each the keys another one set, once the value is in the file, so
//! a `get` it asks then reads it. An instance never hears its own, and it
//! hears the file of the key it runs with now ([`follow`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::View;
use super::kernel::{Answer, in_order, spawn_host_subscription, spawn_reply};
use super::wire::{self, methods, methods::Capability};

type Kept = BTreeMap<String, Vec<u8>>;

pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
    props: &Option<Vec<u8>>,
) -> bool {
    let method: Method = match (capability, operation) {
        (Capability::Store, "get") => ("io.store.get", get),
        (Capability::Store, "set") => (SET, set),
        (Capability::Store, "list") => ("io.store.list", list),
        (Capability::Store, "changes") => {
            match (payload.is_empty(), file(view, props)) {
                (false, _) => view.reply(
                    id,
                    Err(refusal(
                        methods::refusal::MALFORMED_REQUEST,
                        "`store.changes` takes no payload",
                    )),
                ),
                (true, Ok(file)) => listen(view, id, file),
                (true, Err(refused)) => view.reply(id, Err(refused)),
            }
            return true;
        }
        _ => return false,
    };
    match file(view, props) {
        Ok(file) => ask(view, id, file, method, payload.to_vec()),
        Err(refused) => view.reply(id, Err(refused)),
    }
    true
}

/// The file a request reads and writes, and what it is filed under.
#[derive(Clone, Debug, PartialEq, Eq)]
struct File {
    /// the file read and written
    path: PathBuf,
    /// the program's device file on this network: where a key's file
    /// starts from, and what `store.changes` files its listeners under
    device: PathBuf,
    /// the seated key (hex) the file is kept for; empty for none
    signer: String,
}

/// An instance of a program, as `store.changes` tells them apart: the
/// view's own identity (`View::alive`).
fn instance(view: &View) -> usize {
    std::sync::Arc::as_ptr(&view.alive) as usize
}

/// One `store.changes` subscriber: the instance it is, the key it runs
/// with now, and where its keys go.
struct Listener {
    instance: usize,
    signer: String,
    send: tokio::sync::mpsc::UnboundedSender<String>,
}

type Listeners = HashMap<PathBuf, Vec<Listener>>;

/// Each device file's `store.changes` subscribers, every key's alike.
fn listeners() -> std::sync::MutexGuard<'static, Listeners> {
    static LISTENERS: OnceLock<Mutex<Listeners>> = OnceLock::new();
    LISTENERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `store.changes` on `file`: each key another instance sets in the file
/// of the key this one runs with.
fn listen(view: &mut View, id: u64, file: File) {
    let (send, mut keys) = tokio::sync::mpsc::unbounded_channel();
    listeners().entry(file.device).or_default().push(Listener {
        instance: instance(view),
        signer: file.signer,
        send,
    });
    spawn_host_subscription(view, id, move |mut items| async move {
        while let Some(key) = keys.recv().await {
            if !items.send(Ok(methods::encode(&key))).await {
                return;
            }
        }
    });
}

/// The props `view` is handed now: its `store.changes` hear the file of
/// the key they name from now on, as its requests read it. Props that do
/// not read as a session move nothing.
pub(super) fn follow(view: &View, props: &Option<Vec<u8>>) {
    let Some(signer) = props.as_deref().and_then(signer_of) else {
        return;
    };
    let instance = instance(view);
    for listener in listeners().values_mut().flatten() {
        if listener.instance == instance && listener.signer != signer {
            listener.signer.clone_from(&signer);
        }
    }
}

/// `key` was set in `file` by `writer`: every other instance listening
/// with the same key hears it. One that stopped listening is let go.
fn told(file: &File, key: String, writer: usize) {
    let mut listeners = listeners();
    let Some(listening) = listeners.get_mut(&file.device) else {
        return;
    };
    listening.retain(|listener| {
        listener.instance == writer
            || listener.signer != file.signer
            || listener.send.send(key.clone()).is_ok()
    });
    if listening.is_empty() {
        listeners.remove(&file.device);
    }
}

/// A store method: its perf timer, and what it does to the file.
type Method = (&'static str, fn(&Path, &[u8]) -> Answer);

/// `store.set`'s perf timer, which is also how [`ask`] knows a set.
const SET: &str = "io.store.set";

/// `method` on `file`, queued behind the view's earlier requests on it and
/// answered as a reply. A `set` that kept its value is told to the
/// program's other instances ([`told`]).
fn ask(view: &mut View, id: u64, file: File, (stage, run): Method, payload: Vec<u8>) {
    let (tell, answer) = tokio::sync::oneshot::channel();
    spawn_reply(view, id, async move {
        answer
            .await
            .unwrap_or_else(|_| Err(refusal(methods::refusal::HOST_FAULT, "the store job died")))
    });
    let writer = instance(view);
    // timed where it runs, on the blocking pool (docs/perf.md, question 7)
    let name = file.path.to_string_lossy().into_owned();
    in_order(&name, move || {
        let _timed = crate::perf::time(crate::perf::Key::Shell, stage);
        let answered = adopt(&file).and_then(|()| run(&file.path, &payload));
        if stage == SET
            && answered.is_ok()
            && let Ok((key, _)) = methods::decode::<(String, Option<Vec<u8>>)>(&payload)
        {
            told(&file, key, writer);
        }
        let _ = tell.send(answered);
    });
}

/// A key's file not there yet starts as a copy of the device's: what was
/// kept before files were per key. Nothing to copy, nothing is written.
fn adopt(file: &File) -> Result<(), wire::Error> {
    if file.path == file.device || file.path.exists() || !file.device.exists() {
        return Ok(());
    }
    let bytes = std::fs::read(&file.device)
        .map_err(|error| refusal(methods::refusal::HOST_FAULT, error))?;
    write(&file.path, &bytes)
}

/// This view's file on its own connection's network, for the key its
/// props name; a view from before the app's last (re)connect, or of a
/// followed set that left, is refused, so it never writes into the next
/// network's or after the run in hand.
fn file(view: &View, props: &Option<Vec<u8>>) -> Result<File, wire::Error> {
    let connection = &view.connection;
    if connection.stale() {
        return Err(wire::Error::new(
            methods::refusal::STALE_CONNECTION,
            "view belongs to a previous network connection",
        ));
    }
    if connection.chain.is_empty() {
        return Err(wire::Error::new(
            methods::refusal::NOT_CONNECTED,
            "not connected to a network",
        ));
    }
    let store = store_of(view.recipient.as_deref());
    let signer = props.as_deref().and_then(signer_of).unwrap_or_default();
    Ok(located(&store, &connection.chain, &signer, view.program))
}

/// The seated key (hex) a session names; empty for none.
fn signer_of(props: &[u8]) -> Option<String> {
    methods::decode::<methods::Session>(props)
        .ok()
        .map(|session| session.signer)
}

/// Where `recipient`'s notifiers keep their files: its store, or the app's
/// `<config>/store` for the person at this device, every network's run of
/// a program alike (in hand or followed), so a switch finds its cursor.
pub(super) fn store_of(recipient: Option<&super::notifier::Recipient>) -> PathBuf {
    match recipient {
        Some(recipient) => recipient.store.clone(),
        None => crate::backend::dirs().config.join("store"),
    }
}

fn path(store: &Path, chain: &str, program: &str) -> PathBuf {
    store
        .join(escaped(chain))
        .join(format!("{}.borsh", escaped(program)))
}

/// `program`'s file on `chain` for `signer`, or the device's for none.
fn located(store: &Path, chain: &str, signer: &str, program: &str) -> File {
    let device = path(store, chain, program);
    let keyed = match signer.is_empty() {
        true => device.clone(),
        false => path(&store.join(escaped(chain)), signer, program),
    };
    File {
        path: keyed,
        device,
        signer: signer.to_owned(),
    }
}

/// A name as one path segment, one-to-one: ASCII letters, digits and `-`
/// stay, every other byte is `_` and two hex digits. No `/`, no `..`.
fn escaped(name: &str) -> String {
    name.bytes()
        .map(|byte| match byte.is_ascii_alphanumeric() || byte == b'-' {
            true => char::from(byte).to_string(),
            false => format!("_{byte:02x}"),
        })
        .collect()
}

fn refusal(reason: &'static str, error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(reason, error.to_string())
}

fn key(key: &str) -> Result<(), wire::Error> {
    match key.is_empty() {
        true => Err(refusal(
            methods::refusal::MALFORMED_REQUEST,
            "a store key is not empty",
        )),
        false => Ok(()),
    }
}

fn load(path: &Path) -> Result<Kept, wire::Error> {
    crate::backend::read_or_set_aside(path, methods::decode::<Kept>)
        .map(Option::unwrap_or_default)
        .map_err(|error| refusal(methods::refusal::HOST_FAULT, error))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), wire::Error> {
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        noded_client::fs::atomic_write(path, bytes, false)
    };
    write().map_err(|error| refusal(methods::refusal::HOST_FAULT, error))
}

fn get(path: &Path, payload: &[u8]) -> Answer {
    let asked: String = methods::decode(payload)
        .map_err(|error| refusal(methods::refusal::MALFORMED_REQUEST, error))?;
    key(&asked)?;
    Ok(methods::encode(&load(path)?.remove(&asked)))
}

fn set(path: &Path, payload: &[u8]) -> Answer {
    let (asked, value): (String, Option<Vec<u8>>) = methods::decode(payload)
        .map_err(|error| refusal(methods::refusal::MALFORMED_REQUEST, error))?;
    key(&asked)?;
    let mut kept = load(path)?;
    match value {
        Some(value) => kept.insert(asked, value),
        None => kept.remove(&asked),
    };
    write(path, &methods::encode(&kept))?;
    Ok(methods::encode(&()))
}

/// The keys kept under a prefix (empty: every key), in byte order.
fn list(path: &Path, payload: &[u8]) -> Answer {
    let prefix: String = methods::decode(payload)
        .map_err(|error| refusal(methods::refusal::MALFORMED_REQUEST, error))?;
    let keys: Vec<String> = load(path)?
        .into_keys()
        .filter(|key| key.starts_with(&prefix))
        .collect();
    Ok(methods::encode(&keys))
}

#[cfg(test)]
mod tests;
