//! Where a network's keys live on this device: `remotes/<name>` under the
//! ducktape home, bound to the chain first seen under that name; and the
//! password-locked key file from before keys moved into the OS.

use std::path::PathBuf;

/// Where this device keeps its wallets for a network: the directory
/// [`bind_keyring`] named, under the ducktape home.
pub(crate) fn keystore_root(keyring: &str) -> Result<PathBuf, String> {
    let named = !keyring.is_empty() && !keyring.contains(['/', '\\']) && keyring != "..";
    if !named {
        return Err("the node named no network".into());
    }
    Ok(ducktape_home::root()?.join("remotes").join(keyring))
}

/// The file in a network's key directory that says which chain it belongs
/// to: that chain's founding time, as the node's status reports it.
const FOUNDED: &str = "network-founded";

/// Where a network's keys live on this device, and whether its NAME was
/// already here for another chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Keyring {
    /// The directory under `remotes/` ([`keystore_root`]).
    pub(crate) dir: String,
    /// Another chain holds this name's directory: this one is a different
    /// network that happens to share it.
    pub(crate) other_chain: bool,
}

/// Binds `network` as the node names it, founded at `founded` (the node's
/// `Status::time`), to a key directory. A name is not an identity — two
/// chains can both be called "testkit" — but a name AND its founding time
/// are: the genesis block is built from exactly those two
/// (`Block::genesis(name, time)` in core). So the first chain seen under a
/// name keeps `remotes/<name>` (a directory from before this binding is
/// claimed by whichever chain connects first, and keeps working for it),
/// and any other chain with that name gets `remotes/<name>+<founded>` —
/// its own keys, never the first chain's.
pub(crate) fn bind_keyring(network: &str, founded: u64) -> Result<Keyring, String> {
    bind_in(&ducktape_home::root()?.join("remotes"), network, founded)
}

fn bind_in(remotes: &std::path::Path, network: &str, founded: u64) -> Result<Keyring, String> {
    // `+` is never in a sanitized name, so `<name>+<founded>` can not be
    // another network's own directory.
    let name: String = network
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                true => c,
                false => '_',
            },
        )
        .collect();
    if name.is_empty() || name.chars().all(|c| c == '.') {
        return Err("the node named no network".into());
    }
    let mark = remotes.join(&name).join(FOUNDED);
    // a mark that is there but unreadable names no chain: never claimed, or
    // the first chain's keys would sign for whichever connects next
    let damaged = |error: &dyn std::fmt::Display| {
        format!(
            "This device can't tell which network its {network} keys belong to: {} is damaged ({error}). Remove it only if this node is the network those keys were made on.",
            mark.display()
        )
    };
    let known = match std::fs::read_to_string(&mark) {
        Ok(text) => Some(
            text.trim()
                .parse::<u64>()
                .map_err(|error| damaged(&error))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(damaged(&error)),
    };
    match known {
        Some(known) if known != founded => Ok(Keyring {
            dir: format!("{name}+{founded}"),
            other_chain: true,
        }),
        Some(_) => Ok(Keyring {
            dir: name,
            other_chain: false,
        }),
        None => {
            // An unwritten mark only means the claim is made again next
            // time; the keys themselves are where they always were.
            let written = std::fs::create_dir_all(remotes.join(&name))
                .and_then(|()| super::atomic_write(&mark, founded.to_string().as_bytes(), false));
            if let Err(error) = written {
                tracing::warn!(target: "ducktape::app", %error, network, "chain mark not written");
            }
            Ok(Keyring {
                dir: name,
                other_chain: false,
            })
        }
    }
}

/// The key file a sign-in opens: `DUCKTAPE_USER_KEY`, else the keyring's
/// active wallet.
pub(crate) fn session_key_path(keyring: &str) -> Result<PathBuf, String> {
    keystore::wallet::active_user_key(&keystore_root(keyring)?)
}

/// Whether a password-locked key file from before keys moved into the OS is
/// here for `keyring`. The OS-kept key is [`device_key`](super::device_key)'s;
/// finding one of these instead, the key screen asks its password once and
/// moves it into the OS store.
pub(crate) fn key_exists(keyring: &str) -> bool {
    let Ok(path) = session_key_path(keyring) else {
        return false;
    };
    !matches!(
        keystore::userkey::key_file_state(&path),
        keystore::userkey::KeyFileState::Absent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_binds_to_the_chain_first_seen_under_it() {
        let remotes = tempfile::tempdir().unwrap();
        let first = bind_in(remotes.path(), "testkit", 100).unwrap();
        assert_eq!(first.dir, "testkit");
        assert!(!first.other_chain);
        // the same chain again: the same keys
        assert_eq!(bind_in(remotes.path(), "testkit", 100).unwrap(), first);
        // another chain with the name: its own directory, never the first's
        let other = bind_in(remotes.path(), "testkit", 200).unwrap();
        assert_eq!(other.dir, "testkit+200");
        assert!(other.other_chain);
        assert_eq!(bind_in(remotes.path(), "testkit", 200).unwrap(), other);
        // the first chain still owns the name afterwards
        assert_eq!(bind_in(remotes.path(), "testkit", 100).unwrap(), first);
        assert!(bind_in(remotes.path(), "", 1).is_err());
        assert!(bind_in(remotes.path(), "..", 1).is_err());
    }

    /// A mark that is there but names no chain (a cut write, foreign bytes,
    /// not a file) binds nothing and is left as it is: the keys under it are
    /// never handed to whichever chain connects next.
    #[test]
    fn a_damaged_chain_mark_is_never_claimed() {
        let remotes = tempfile::tempdir().unwrap();
        let mark = remotes.path().join("testkit").join(FOUNDED);
        std::fs::create_dir_all(mark.parent().unwrap()).unwrap();
        for damaged in ["", "10O", "\u{0}\u{0}"] {
            std::fs::write(&mark, damaged).unwrap();
            let refused = bind_in(remotes.path(), "testkit", 100).unwrap_err();
            assert!(refused.contains(FOUNDED), "{refused}");
            let left = std::fs::read_to_string(&mark).unwrap();
            assert_eq!(left, damaged, "the mark was claimed");
        }
        std::fs::remove_file(&mark).unwrap();
        std::fs::create_dir(&mark).unwrap();
        assert!(bind_in(remotes.path(), "testkit", 100).is_err());
        assert!(mark.is_dir());
    }

    #[test]
    fn a_key_directory_from_before_the_binding_goes_to_the_first_chain_that_connects() {
        let remotes = tempfile::tempdir().unwrap();
        let keys = keystore::wallet::key_file(&remotes.path().join("testkit"), "default");
        std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
        std::fs::write(&keys, "sealed").unwrap();
        assert_eq!(
            bind_in(remotes.path(), "testkit", 7).unwrap().dir,
            "testkit"
        );
        assert!(keys.exists(), "the old key is left where it was");
        assert!(bind_in(remotes.path(), "testkit", 8).unwrap().other_chain);
    }
}
