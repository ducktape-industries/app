//! `store.get` / `store.set` / `store.list`: what a view keeps on this
//! device between runs.
//!
//! One file per program per network per seated key,
//! `<config>/store/<chain>/<key>/<program>.borsh`, `<key>` the session's
//! `signer` (hex) as the view is handed it (`roster::props`); with no key
//! seated, the device's own `<config>/store/<chain>/<program>.borsh` (a
//! recipient's notifier's under its own store, `notifier::Recipient`): a
//! borsh map of key → bytes, rewritten whole through
//! `noded_client::fs::atomic_write`. A key's file, made on its first
//! open, takes the device file's entries whose key has that key as one of
//! its `/` parts (`reads/<key>`, as hosts before per-key files kept them);
//! nothing of another key's is copied into it. A view
//! names keys, never paths — the chain, the seated key and the program
//! come from the host — so no view reaches another view's file, another
//! network's or another key's.
//!
//! [`answer`] reads the view's ask as a [`Request`] and routes it: each
//! one but `store.changes` is [`applied`] to the file's map, a pure step,
//! read and written off the window thread, one request after another in
//! the order the view sent them (`kernel::in_order` under the file's
//! path), so a `get` reads the `set` before it.
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ask {
    File(Request),
    Changes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Get(String),
    Set(String, Option<Vec<u8>>),
    List(String),
}

impl Ask {
    fn read(
        capability: Capability,
        operation: &str,
        payload: &[u8],
    ) -> Option<Result<Self, wire::Error>> {
        let read = match (capability, operation) {
            (Capability::Store, "get") => methods::decode(payload)
                .map_err(malformed)
                .and_then(|asked: String| key(&asked).map(|()| Request::Get(asked))),
            (Capability::Store, "set") => methods::decode(payload).map_err(malformed).and_then(
                |(asked, value): (String, Option<Vec<u8>>)| {
                    key(&asked).map(|()| Request::Set(asked, value))
                },
            ),
            (Capability::Store, "list") => methods::decode(payload)
                .map_err(malformed)
                .map(Request::List),
            (Capability::Store, "changes") => {
                return Some(match payload.is_empty() {
                    true => Ok(Self::Changes),
                    false => Err(malformed("`store.changes` takes no payload")),
                });
            }
            _ => return None,
        };
        Some(read.map(Self::File))
    }
}

impl Request {
    fn stage(&self) -> &'static str {
        match self {
            Self::Get(_) => "io.store.get",
            Self::Set(..) => "io.store.set",
            Self::List(_) => "io.store.list",
        }
    }
}

pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
    props: &Option<Vec<u8>>,
) -> bool {
    let Some(asked) = Ask::read(capability, operation, payload) else {
        return false;
    };
    match (asked, file(view, props)) {
        (Err(refused), _) | (Ok(_), Err(refused)) => view.reply(id, Err(refused)),
        (Ok(Ask::Changes), Ok(file)) => listen(view, id, file),
        (Ok(Ask::File(request)), Ok(file)) => ask(view, id, file, request),
    }
    true
}

fn applied(mut kept: Kept, request: &Request) -> (Vec<u8>, Option<Kept>) {
    match request {
        Request::Get(asked) => (methods::encode(&kept.remove(asked)), None),
        Request::Set(asked, value) => {
            match value {
                Some(value) => kept.insert(asked.clone(), value.clone()),
                None => kept.remove(asked),
            };
            (methods::encode(&()), Some(kept))
        }
        Request::List(prefix) => {
            let keys: Vec<String> = kept
                .into_keys()
                .filter(|key| key.starts_with(prefix.as_str()))
                .collect();
            (methods::encode(&keys), None)
        }
    }
}

fn run(path: &Path, request: &Request) -> Answer {
    let (answer, changed) = applied(load(path)?, request);
    if let Some(kept) = changed {
        write(path, &methods::encode(&kept))?;
    }
    Ok(answer)
}

fn adopt(file: &File) -> Result<(), wire::Error> {
    if file.signer.is_empty() || file.path.exists() || !file.device.exists() {
        return Ok(());
    }
    let own: Kept = load(&file.device)?
        .into_iter()
        .filter(|(key, _)| key.split('/').any(|part| part == file.signer))
        .collect();
    match own.is_empty() {
        true => Ok(()),
        false => write(&file.path, &methods::encode(&own)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct File {
    path: PathBuf,
    device: PathBuf,
    signer: String,
    was: Vec<File>,
}

fn moved(file: &File) -> Result<(), wire::Error> {
    for was in &file.was {
        for (from, to) in [(&was.device, &file.device), (&was.path, &file.path)] {
            let due = from.exists() && !to.exists();
            if due {
                rename(from, to)?;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "web")]
pub(crate) fn carried(root: &Path, program: &str) -> PathBuf {
    let renamed = super::in_hand().roster.renamed_from(program);
    carried_from(root, &renamed, program)
}

#[cfg(any(test, feature = "web"))]
fn carried_from(root: &Path, renamed: &[String], program: &str) -> PathBuf {
    let to = root.join(escaped(program));
    for from in renamed {
        let from = root.join(escaped(from));
        let due = from.exists() && !to.exists();
        if !due {
            continue;
        }
        if let Err(refused) = rename(&from, &to) {
            tracing::warn!(
                target: "ducktape::app",
                reason = "carry_refused",
                from = %from.display(),
                to = %to.display(),
                error = %refused.message,
                "a renamed program's folder stayed under its old id"
            );
        }
    }
    to
}

fn rename(from: &Path, to: &Path) -> Result<(), wire::Error> {
    match crate::contract::os().disk().rename(from, to) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(refusal(methods::refusal::HOST_FAULT, error))
        }
        _ => Ok(()),
    }
}

fn instance(view: &View) -> usize {
    std::sync::Arc::as_ptr(&view.alive) as usize
}

struct Listener {
    instance: usize,
    signer: String,
    send: tokio::sync::mpsc::UnboundedSender<String>,
}

type Listeners = HashMap<PathBuf, Vec<Listener>>;

fn listeners() -> std::sync::MutexGuard<'static, Listeners> {
    static LISTENERS: OnceLock<Mutex<Listeners>> = OnceLock::new();
    LISTENERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

fn ask(view: &mut View, id: u64, file: File, request: Request) {
    let (tell, answer) = tokio::sync::oneshot::channel();
    spawn_reply(view, id, async move {
        answer
            .await
            .unwrap_or_else(|_| Err(refusal(methods::refusal::HOST_FAULT, "the store job died")))
    });
    let writer = instance(view);
    let name = file.path.to_string_lossy().into_owned();
    in_order(&name, move || {
        let _timed = crate::perf::time(crate::perf::Key::Shell, request.stage());
        let answered = moved(&file)
            .and_then(|()| adopt(&file))
            .and_then(|()| run(&file.path, &request));
        if let (Ok(_), Request::Set(key, _)) = (&answered, request) {
            told(&file, key, writer);
        }
        let _ = tell.send(answered);
    });
}

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
    let was = connection
        .on
        .roster()
        .renamed_from(view.program)
        .iter()
        .map(|from| located(&store, &connection.chain, &signer, from))
        .collect();
    Ok(File {
        was,
        ..located(&store, &connection.chain, &signer, view.program)
    })
}

pub(super) fn signer_of(props: &[u8]) -> Option<String> {
    methods::decode::<methods::Session>(props)
        .ok()
        .map(|session| session.signer)
}

pub(super) fn store_of(recipient: Option<&super::notifier::Recipient>) -> PathBuf {
    match recipient {
        Some(recipient) => recipient.store.clone(),
        None => crate::backend::dirs().config.join("store"),
    }
}

pub(super) fn path(store: &Path, chain: &str, program: &str) -> PathBuf {
    store
        .join(escaped(chain))
        .join(format!("{}.borsh", escaped(program)))
}

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
        was: Vec::new(),
    }
}

pub(super) fn escaped(name: &str) -> String {
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

fn malformed(error: impl std::fmt::Display) -> wire::Error {
    refusal(methods::refusal::MALFORMED_REQUEST, error)
}

fn key(key: &str) -> Result<(), wire::Error> {
    match key.is_empty() {
        true => Err(malformed("a store key is not empty")),
        false => Ok(()),
    }
}

fn load(path: &Path) -> Result<Kept, wire::Error> {
    crate::backend::read_or_set_aside(path, methods::decode::<Kept>)
        .map(Option::unwrap_or_default)
        .map_err(|error| refusal(methods::refusal::HOST_FAULT, error))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), wire::Error> {
    crate::contract::os()
        .disk()
        .write(path, bytes, false)
        .map_err(|error| refusal(methods::refusal::HOST_FAULT, error))
}

#[cfg(test)]
mod tests;
