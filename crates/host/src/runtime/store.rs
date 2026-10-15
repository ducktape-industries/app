//! `store.get` / `store.set`: what a view keeps on this device between runs.
//!
//! One file per view per network, `<config>/store/<chain>/<program>.borsh`:
//! a borsh map of key → bytes, rewritten whole through
//! `noded_client::fs::atomic_write`. A view names keys, never paths — the chain and the program come from the
//! host — so no view reaches another view's file or another network's.
//!
//! The file is read and written off the window thread, one request after
//! another in the order the view sent them (`kernel::in_order` under the
//! file's path), so a `get` reads the `set` before it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::View;
use super::kernel::{Answer, in_order, spawn_reply};
use super::wire::{self, methods, methods::Capability};

type Kept = BTreeMap<String, Vec<u8>>;

pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    let method: Method = match (capability, operation) {
        (Capability::Store, "get") => ("io.store.get", get),
        (Capability::Store, "set") => ("io.store.set", set),
        _ => return false,
    };
    match file(view) {
        Ok(path) => ask(view, id, path, method, payload.to_vec()),
        Err(refused) => view.reply(id, Err(refused)),
    }
    true
}

/// A store method: its perf timer, and what it does to the file.
type Method = (&'static str, fn(&Path, &[u8]) -> Answer);

/// `method` on the file at `path`, queued behind the view's earlier
/// requests on it and answered as a reply.
fn ask(view: &mut View, id: u64, path: PathBuf, (stage, run): Method, payload: Vec<u8>) {
    let (tell, told) = tokio::sync::oneshot::channel();
    spawn_reply(view, id, async move {
        told.await
            .unwrap_or_else(|_| Err(refusal(methods::refusal::HOST_FAULT, "the store job died")))
    });
    // timed where it runs, on the blocking pool (docs/perf.md, question 7)
    let name = path.to_string_lossy().into_owned();
    in_order(&name, move || {
        let _timed = crate::perf::time(crate::perf::Key::Shell, stage);
        let _ = tell.send(run(&path, &payload));
    });
}

/// This view's file on its own connection's network; a view from before
/// the app's last (re)connect is refused, so it never writes into the next
/// network's.
fn file(view: &View) -> Result<PathBuf, wire::Error> {
    let rev = super::connection().lock().expect("views rpc").rev;
    let connection = &view.connection;
    if connection.rev != rev {
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
    Ok(path(
        &crate::backend::dirs().config,
        &connection.chain,
        view.program,
    ))
}

fn path(config: &Path, chain: &str, program: &str) -> PathBuf {
    config
        .join("store")
        .join(escaped(chain))
        .join(format!("{}.borsh", escaped(program)))
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
    let bytes = methods::encode(&kept);
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        noded_client::fs::atomic_write(path, &bytes, false)
    };
    write().map_err(|error| refusal(methods::refusal::HOST_FAULT, error))?;
    Ok(methods::encode(&()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("ducktape-store-{}", rand::random::<u64>()))
    }

    fn put(path: &Path, key: &str, value: Option<&[u8]>) -> Answer {
        set(
            path,
            &methods::encode(&(key.to_owned(), value.map(<[u8]>::to_vec))),
        )
    }

    fn read(path: &Path, key: &str) -> Option<Vec<u8>> {
        methods::decode(&get(path, &methods::encode(&key.to_owned())).unwrap()).unwrap()
    }

    /// Each view on each network has its own file, and a key kept by one is
    /// absent to the others; a name never climbs out of the store.
    #[test]
    fn views_and_chains_keep_apart() {
        let config = scratch();
        let chat = path(&config, "dev#01", "chat");
        let forge = path(&config, "dev#01", "forge");
        let other = path(&config, "dev#02", "chat");
        put(&chat, "reads", Some(b"mine")).unwrap();
        assert_eq!(read(&chat, "reads").as_deref(), Some(&b"mine"[..]));
        assert_eq!(read(&forge, "reads"), None);
        assert_eq!(read(&other, "reads"), None);
        assert_ne!(path(&config, "a#b", "chat"), path(&config, "a_23b", "chat"));
        let sly = path(&config, "../..", "../x");
        assert!(sly.starts_with(config.join("store")));
        assert_eq!(
            sly.components().count(),
            config.join("store").components().count() + 2
        );
        let _ = std::fs::remove_dir_all(config);
    }

    /// An empty key is refused; `None` drops a key; what was set is read
    /// back from disk by a fresh read, as after a relaunch.
    #[test]
    fn keys_are_named_dropped_and_kept_across_a_reopen() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        assert_eq!(
            put(&file, "", Some(b"x")).unwrap_err().code,
            methods::refusal::MALFORMED_REQUEST
        );
        assert!(get(&file, &methods::encode(&String::new())).is_err());
        put(&file, "a", Some(b"1")).unwrap();
        put(&file, "b", Some(b"2")).unwrap();
        put(&file, "a", None).unwrap();
        let reopened = load(&file).unwrap();
        assert_eq!(reopened, Kept::from([("b".to_owned(), b"2".to_vec())]));
        assert_eq!(read(&file, "a"), None);
        assert_eq!(
            std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
            1
        );
        let _ = std::fs::remove_dir_all(config);
    }

    /// A `set` and a `get` behind it, asked back to back: neither is
    /// answered on the thread that asked (the window thread, in a redraw),
    /// and the `get` reads what the `set` kept.
    #[test]
    fn a_get_behind_a_set_reads_it_off_the_window_thread() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        let mut view = crate::runtime::kernel::tests::view();
        for nth in 0..20u64 {
            let value = nth.to_string().into_bytes();
            let asked = methods::encode(&("reads".to_owned(), Some(value)));
            ask(
                &mut view,
                2 * nth,
                file.clone(),
                ("io.store.set", set),
                asked,
            );
            let asked = methods::encode(&"reads".to_owned());
            ask(
                &mut view,
                2 * nth + 1,
                file.clone(),
                ("io.store.get", get),
                asked,
            );
        }
        assert!(
            view.pending.is_empty(),
            "answered on the window thread: {:?}",
            view.pending
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut answered = Vec::new();
        while answered.len() < 40 && std::time::Instant::now() < deadline {
            view.replies.drain_into(&mut answered);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut gets: Vec<(u64, Option<Vec<u8>>)> = answered
            .into_iter()
            .filter_map(|event| match event {
                wire::Event::Response { id, result, .. } if id % 2 == 1 => {
                    Some((id, methods::decode(&result.unwrap()).unwrap()))
                }
                _ => None,
            })
            .collect();
        gets.sort();
        let want: Vec<_> = (0..20u64)
            .map(|nth| (2 * nth + 1, Some(nth.to_string().into_bytes())))
            .collect();
        assert_eq!(gets, want, "each get reads the set just before it");
        let _ = std::fs::remove_dir_all(config);
    }

    /// A `set` a view sent right before the app quits is on disk once the
    /// quit drain is done, though it was still queued when the drain began.
    #[test]
    fn the_quit_drain_waits_for_a_queued_set() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        let mut view = crate::runtime::kernel::tests::view();
        let release = crate::runtime::kernel::tests::held(&file.to_string_lossy());
        let asked = methods::encode(&("last".to_owned(), Some(b"word".to_vec())));
        ask(&mut view, 0, file.clone(), ("io.store.set", set), asked);
        assert!(!file.exists(), "the set waits behind the held job");

        let mut drained = std::pin::pin!(crate::runtime::kernel::drain());
        assert!(
            crate::runtime::kernel::tests::waiting(drained.as_mut()),
            "the drain is done while the set is still queued"
        );
        release.send(()).unwrap();
        futures::executor::block_on(drained);
        assert_eq!(
            load(&file).unwrap(),
            Kept::from([("last".to_owned(), b"word".to_vec())])
        );
        let _ = std::fs::remove_dir_all(config);
    }

    /// A store file cut to nothing answers `None` to a view, not a host
    /// fault: it is set aside as `.bad`, and the next `set` lands.
    #[test]
    fn a_cut_store_file_reads_as_empty_and_is_set_aside() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        put(&file, "a", Some(b"1")).unwrap();
        std::fs::write(&file, b"").unwrap();
        assert_eq!(read(&file, "a"), None);
        assert!(file.with_extension("borsh.bad").exists());
        assert!(!file.exists());
        put(&file, "a", Some(b"2")).unwrap();
        assert_eq!(read(&file, "a").as_deref(), Some(&b"2"[..]));
        let _ = std::fs::remove_dir_all(config);
    }
}
