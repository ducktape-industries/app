//! `store.get` / `store.set`: what a view keeps on this device between runs.
//!
//! One file per view per network, `<config>/store/<chain>/<module>.borsh`:
//! a borsh map of key → bytes, rewritten whole through
//! `backend::atomic_write`, at most [`MAX_KEYS`] keys and [`MAX_BYTES`]. A
//! view names keys, never paths — the chain and the module come from the
//! host — so no view reaches another view's file or another network's.
//!
//! The file is read and written off the window thread, one request after
//! another in the order the view sent them (`kernel::in_order` under the
//! file's path), so a `get` reads the `set` before it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::Guest;
use super::kernel::{Answer, in_order, spawn_reply};
use super::wire::{self, methods, methods::Capability};

type Kept = BTreeMap<String, Vec<u8>>;

/// What one view keeps on one network: a `set` that would grow its file
/// past either is refused `too_large`; one that does not grow it lands.
const MAX_BYTES: usize = 4 << 20;
const MAX_KEYS: usize = 1024;

pub(super) fn answer(
    guest: &mut Guest,
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
    match file(guest) {
        Ok(path) => ask(guest, id, path, method, payload.to_vec()),
        Err(refused) => guest.reply(id, Err(refused)),
    }
    true
}

/// A store method: its perf timer, and what it does to the file.
type Method = (&'static str, fn(&Path, &[u8]) -> Answer);

/// `method` on the file at `path`, queued behind the view's earlier
/// requests on it and answered as a reply; nothing touches the file for a
/// request refused `in_flight_limit`.
fn ask(guest: &mut Guest, id: u64, path: PathBuf, (stage, run): Method, payload: Vec<u8>) {
    let (tell, told) = tokio::sync::oneshot::channel();
    let admitted = spawn_reply(guest, id, async move {
        told.await
            .unwrap_or_else(|_| Err(refusal(methods::refusal::HOST_FAULT, "the store job died")))
    });
    if admitted {
        // timed where it runs, on the blocking pool (docs/perf.md, question 7)
        let name = path.to_string_lossy().into_owned();
        in_order(&name, move || {
            let _timed = crate::perf::time(crate::perf::Key::Shell, stage);
            let _ = tell.send(run(&path, &payload));
        });
    }
}

/// This view's file on the network in hand; a view from before the last
/// (re)connect is refused, so it never writes into the next network's.
fn file(guest: &Guest) -> Result<PathBuf, wire::Error> {
    let connection = super::connection().lock().expect("views rpc");
    if connection.rev != guest.connection_rev {
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
    let config = crate::backend::config_dir()
        .map_err(|error| refusal(methods::refusal::HOST_FAULT, error))?;
    Ok(path(&config, &connection.chain, guest.module))
}

fn path(config: &Path, chain: &str, module: &str) -> PathBuf {
    config
        .join("store")
        .join(escaped(chain))
        .join(format!("{}.borsh", escaped(module)))
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
    // a drop, or a value no longer than the one it replaces, always lands
    let grows = value
        .as_ref()
        .is_some_and(|value| kept.get(&asked).is_none_or(|old| value.len() > old.len()));
    match value {
        Some(value) => kept.insert(asked, value),
        None => kept.remove(&asked),
    };
    let bytes = methods::encode(&kept);
    if grows && (bytes.len() > MAX_BYTES || kept.len() > MAX_KEYS) {
        return Err(refusal(
            methods::refusal::TOO_LARGE,
            format!(
                "a view keeps at most {MAX_KEYS} keys and {} MiB on this device",
                MAX_BYTES >> 20
            ),
        ));
    }
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::backend::atomic_write(path, &bytes, false)
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

    /// A view setting 1 MiB under fresh keys is refused `too_large` once
    /// its file would pass 4 MiB, and the file stays under it; a drop, and
    /// a value no longer than the one it replaces, still land at the cap.
    #[test]
    fn a_view_past_its_cap_is_refused_too_large_and_its_file_stays_under_it() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        let mib = vec![7u8; 1 << 20];
        let refused = (0..8)
            .map(|nth| put(&file, &format!("k{nth}"), Some(&mib)))
            .position(|answer| answer.is_err())
            .expect("a fresh MiB was refused before the eighth");
        assert_eq!(refused, 3, "three MiB and their keys fit under 4 MiB");
        let answer = put(&file, "k3", Some(&mib)).unwrap_err();
        assert_eq!(answer.code, methods::refusal::TOO_LARGE);
        let size = std::fs::metadata(&file).unwrap().len() as usize;
        assert!(size <= MAX_BYTES, "{size} bytes on disk");
        assert_eq!(read(&file, "k3"), None);
        put(&file, "k0", Some(b"small")).unwrap();
        put(&file, "k1", None).unwrap();
        put(&file, "k3", Some(&mib)).unwrap();
        let _ = std::fs::remove_dir_all(config);
    }

    /// Past [`MAX_KEYS`] a fresh key is refused `too_large`; a kept key is
    /// still set and dropped.
    #[test]
    fn a_view_past_its_keys_is_refused_a_fresh_one() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        let full: Kept = (0..MAX_KEYS)
            .map(|nth| (nth.to_string(), vec![1]))
            .collect();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, methods::encode(&full)).unwrap();
        assert_eq!(
            put(&file, "fresh", Some(b"1")).unwrap_err().code,
            methods::refusal::TOO_LARGE
        );
        put(&file, "0", Some(b"2")).unwrap();
        put(&file, "1", None).unwrap();
        put(&file, "fresh", Some(b"1")).unwrap();
        assert_eq!(load(&file).unwrap().len(), MAX_KEYS);
        let _ = std::fs::remove_dir_all(config);
    }

    /// A `set` and a `get` behind it, asked back to back: neither is
    /// answered on the thread that asked (the window thread, in a redraw),
    /// and the `get` reads what the `set` kept.
    #[test]
    fn a_get_behind_a_set_reads_it_off_the_window_thread() {
        let config = scratch();
        let file = path(&config, "dev#01", "chat");
        let mut guest = crate::runtime::kernel::tests::guest();
        for nth in 0..20u64 {
            let value = nth.to_string().into_bytes();
            let asked = methods::encode(&("reads".to_owned(), Some(value)));
            ask(
                &mut guest,
                2 * nth,
                file.clone(),
                ("io.store.set", set),
                asked,
            );
            let asked = methods::encode(&"reads".to_owned());
            ask(
                &mut guest,
                2 * nth + 1,
                file.clone(),
                ("io.store.get", get),
                asked,
            );
        }
        assert!(
            guest.pending.is_empty(),
            "answered on the window thread: {:?}",
            guest.pending
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut answered = Vec::new();
        while answered.len() < 40 && std::time::Instant::now() < deadline {
            guest.replies.drain_into(&mut answered).unwrap();
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
