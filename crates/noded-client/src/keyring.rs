//! Where a network's keys live on a device: a directory under the device's
//! `remotes` directory, bound to the chain first seen under that name.

use std::path::Path;

/// The file in a network's key directory that says which chain it belongs
/// to: that chain's founding time, as the node's status reports it.
const FOUNDED: &str = "network-founded";

/// Where a network's keys live on this device, and whether its NAME was
/// already here for another chain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keyring {
    /// The directory under `remotes`.
    pub dir: String,
    /// Another chain holds this name's directory: this one is a different
    /// network that happens to share it.
    pub other_chain: bool,
}

/// Binds `network` as the node names it, founded at `founded` (the node's
/// `Status::time`), to a key directory under `remotes`. A name is not an
/// identity — two chains can both be called "testkit" — but a name AND its
/// founding time are: the genesis block is built from exactly those two
/// (`Block::genesis(name, time)` in core). So the first chain seen under a
/// name keeps `<remotes>/<name>` (a directory from before this binding is
/// claimed by whichever chain connects first, and keeps working for it),
/// and any other chain with that name gets `<remotes>/<name>+<founded>` —
/// its own keys, never the first chain's.
pub fn bind(remotes: &Path, network: &str, founded: u64) -> Result<Keyring, String> {
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
            let written = std::fs::create_dir_all(remotes.join(&name)).and_then(|()| {
                crate::fs::atomic_write(&mark, founded.to_string().as_bytes(), false)
            });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_binds_to_the_chain_first_seen_under_it() {
        let remotes = tempfile::tempdir().unwrap();
        let first = bind(remotes.path(), "testkit", 100).unwrap();
        assert_eq!(first.dir, "testkit");
        assert!(!first.other_chain);
        // the same chain again: the same keys
        assert_eq!(bind(remotes.path(), "testkit", 100).unwrap(), first);
        // another chain with the name: its own directory, never the first's
        let other = bind(remotes.path(), "testkit", 200).unwrap();
        assert_eq!(other.dir, "testkit+200");
        assert!(other.other_chain);
        assert_eq!(bind(remotes.path(), "testkit", 200).unwrap(), other);
        // the first chain still owns the name afterwards
        assert_eq!(bind(remotes.path(), "testkit", 100).unwrap(), first);
        assert!(bind(remotes.path(), "", 1).is_err());
        assert!(bind(remotes.path(), "..", 1).is_err());
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
            let refused = bind(remotes.path(), "testkit", 100).unwrap_err();
            assert!(refused.contains(FOUNDED), "{refused}");
            let left = std::fs::read_to_string(&mark).unwrap();
            assert_eq!(left, damaged, "the mark was claimed");
        }
        std::fs::remove_file(&mark).unwrap();
        std::fs::create_dir(&mark).unwrap();
        assert!(bind(remotes.path(), "testkit", 100).is_err());
        assert!(mark.is_dir());
    }

    /// A directory of keys from before the binding (a desktop's wallet file,
    /// say) belongs to the first chain that connects under its name.
    #[test]
    fn a_key_directory_from_before_the_binding_goes_to_the_first_chain_that_connects() {
        let remotes = tempfile::tempdir().unwrap();
        let keys = remotes
            .path()
            .join("testkit")
            .join("keys")
            .join("default.key");
        std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
        std::fs::write(&keys, "sealed").unwrap();
        assert_eq!(bind(remotes.path(), "testkit", 7).unwrap().dir, "testkit");
        assert!(keys.exists(), "the old key is left where it was");
        assert!(bind(remotes.path(), "testkit", 8).unwrap().other_chain);
    }
}
