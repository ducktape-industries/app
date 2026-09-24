//! `store.get` / `store.set`: what a view keeps on this device between runs.
//!
//! One file per view per network, `<config>/store/<chain>/<module>.borsh`:
//! a borsh map of key → bytes, rewritten whole through a temp file and a
//! rename. A view names keys, never paths — the chain and the module come
//! from the host — so no view reaches another view's file or another
//! network's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::Guest;
use super::kernel::Answer;
use super::wire::{self, doors};

type Kept = BTreeMap<String, Vec<u8>>;

pub(super) fn answer(
    guest: &mut Guest,
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    let answer = match (capability, operation) {
        ("store", "get") => file(guest).and_then(|path| get(&path, payload)),
        ("store", "set") => file(guest).and_then(|path| set(&path, payload)),
        _ => return false,
    };
    guest.reply(id, answer);
    true
}

/// This view's file on the network in hand; a view from before the last
/// (re)connect is refused, so it never writes into the next network's.
fn file(guest: &Guest) -> Result<PathBuf, wire::Refusal> {
    let connection = super::connection().lock().expect("views rpc");
    if connection.rev != guest.connection_rev {
        return Err(wire::Refusal::new(
            "stale_connection",
            "view belongs to a previous network connection",
        ));
    }
    if connection.chain.is_empty() {
        return Err(wire::Refusal::new(
            "not_connected",
            "not connected to a network",
        ));
    }
    let config = crate::backend::config_dir().map_err(|error| refusal("host_fault", error))?;
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

fn refusal(reason: &'static str, error: impl std::fmt::Display) -> wire::Refusal {
    wire::Refusal::new(reason, error.to_string())
}

fn key(key: &str) -> Result<(), wire::Refusal> {
    match key.is_empty() {
        true => Err(refusal("malformed_request", "a store key is not empty")),
        false => Ok(()),
    }
}

fn load(path: &Path) -> Result<Kept, wire::Refusal> {
    match std::fs::read(path) {
        Ok(bytes) => doors::decode(&bytes).map_err(|error| refusal("host_fault", error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Kept::new()),
        Err(error) => Err(refusal("host_fault", error)),
    }
}

fn get(path: &Path, payload: &[u8]) -> Answer {
    let asked: String =
        doors::decode(payload).map_err(|error| refusal("malformed_request", error))?;
    key(&asked)?;
    Ok(doors::encode(&load(path)?.remove(&asked)))
}

// ponytail: the whole file rewritten on each set; a view keeps a few small keys
fn set(path: &Path, payload: &[u8]) -> Answer {
    let (asked, value): (String, Option<Vec<u8>>) =
        doors::decode(payload).map_err(|error| refusal("malformed_request", error))?;
    key(&asked)?;
    let mut kept = load(path)?;
    match value {
        Some(value) => kept.insert(asked, value),
        None => kept.remove(&asked),
    };
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("borsh.tmp");
        std::fs::write(&temp, doors::encode(&kept))?;
        std::fs::rename(&temp, path)
    };
    write().map_err(|error| refusal("host_fault", error))?;
    Ok(doors::encode(&()))
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
            &doors::encode(&(key.to_owned(), value.map(<[u8]>::to_vec))),
        )
    }

    fn read(path: &Path, key: &str) -> Option<Vec<u8>> {
        doors::decode(&get(path, &doors::encode(&key.to_owned())).unwrap()).unwrap()
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
            put(&file, "", Some(b"x")).unwrap_err().reason,
            "malformed_request"
        );
        assert!(get(&file, &doors::encode(&String::new())).is_err());
        put(&file, "a", Some(b"1")).unwrap();
        put(&file, "b", Some(b"2")).unwrap();
        put(&file, "a", None).unwrap();
        let reopened = load(&file).unwrap();
        assert_eq!(reopened, Kept::from([("b".to_owned(), b"2".to_vec())]));
        assert_eq!(read(&file, "a"), None);
        assert!(!file.with_extension("borsh.tmp").exists());
        let _ = std::fs::remove_dir_all(config);
    }
}
