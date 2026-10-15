//! Everything that crosses the process boundary. The node daemon's client
//! is `noded_client` (`crates/noded-client`), shared with the phone; here is
//! what is this app's own: the one signing key held in memory and the
//! frames it signs ([`session`]); this device's per-network key in the OS
//! store ([`device_key`]) and the key directory it falls back to
//! ([`key_dir`]); the identity flows only the desktop runs ([`identity`])
//! and the two ways onto an account, a passkey ([`passkey`], through
//! [`auth_page`] and [`loopback`]) or another key ([`join`]); a view's wasm
//! out of its program's blob ([`views`]); prefs.json ([`prefs`]) and the
//! recent-nodes list ([`endpoints`]); the app's own directories
//! ([`app_dirs`]).

mod app_dirs;
pub(crate) mod auth_page;
pub(crate) mod device_key;
mod endpoints;
pub(crate) mod identity;
pub(crate) mod invite;
pub(crate) mod join;
mod key_dir;
mod loopback;
pub(crate) mod passkey;
mod prefs;
mod session;
pub(crate) mod views;

pub use app_dirs::app_log_path;
pub(crate) use app_dirs::{cache_dir, config_dir, data_dir};
pub(crate) use endpoints::{RecentEndpoint, forget_endpoint, note_endpoint, recent_endpoints};
pub(crate) use key_dir::{bind_keyring, key_exists, keystore_root, session_key_path};
pub(crate) use noded_client::Client as RpcClient;
pub(crate) use noded_client::wire::{Layer, Status as NodeStatus};
#[cfg(test)]
pub(crate) use prefs::prefs_reads;
pub(crate) use prefs::{
    Appearance, Layout, edit_prefs, load_appearance, load_layout, load_motion, memory_bounds,
    read_prefs, save_appearance, save_layout, save_motion,
};
#[cfg(test)]
pub(crate) use session::seat_serial;
pub(crate) use session::{
    anchor, lock_signer, query_frame, query_signer, seat_key, seated, seated_frame, seated_key,
    seated_sign, seated_signer,
};

use std::path::Path;

use view_wire::methods::refusal;

/// A client for the node at `base`, timed as `io.rpc_client`.
pub(crate) fn rpc_client(base: impl Into<String>) -> RpcClient {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.rpc_client");
    RpcClient::new(base)
}

/// The roles a test node's status names.
#[cfg(test)]
pub(crate) fn test_roles() -> abi::Roles {
    abi::Roles {
        programs: "programs".into(),
        validators: "valset".into(),
        identity: "identity".into(),
        blobs: "blobs".into(),
        awaits: "awaits".into(),
    }
}

/// A refusal the NODE or the PROGRAM authored, carried through with its
/// own token; a transport failure gets the app's: `rpc_client` when nothing
/// reached the node, `node_failed` when the request may have and the answer
/// is what went missing — a write's caller must not treat the two alike.
pub(crate) fn refused(error: noded_client::Error) -> view_wire::Error {
    use noded_client::Error;
    match error {
        Error::Refused(refusal) => view_wire::Error::new(refusal.reason, refusal.sentence),
        Error::Decode(refusal) => {
            view_wire::Error::new(view_wire::code::UNEXPECTED_REPLY, refusal.sentence)
        }
        Error::Failed { status, sentence } => {
            view_wire::Error::new(refusal::NODE_FAILED, format!("{status}: {sentence}"))
        }
        Error::Unreachable(sentence) => view_wire::Error::new(refusal::RPC_CLIENT, sentence),
        Error::Transport(sentence) => view_wire::Error::new(
            refusal::NODE_FAILED,
            format!("no answer came back: {sentence}"),
        ),
    }
}

/// A sentence for the person, off an error the code produced.
pub(crate) fn user_error(message: String) -> String {
    let password_refused = message.contains("corrupt or wrong password");
    if password_refused {
        return "Wrong password.".into();
    }
    let node_slow = message.contains("timed out");
    if node_slow {
        return "The node did not answer in time. Retry in a moment.".into();
    }
    // A keystore refusal that names a CLI command (`run `ducktape wallet
    // new <name>``) is a hint for a terminal, not a sentence for a screen —
    // catch every shape of it here, once, rather than in each caller.
    if message.contains("`ducktape ") {
        return "This device's key needs attention: try again, or read without a key.".into();
    }
    message
}

/// A sentence for a connection attempt that never reached `origin`: the
/// common shapes are `noded_client::words::unreachable`'s; any other text
/// falls through [`user_error`] as it is.
pub(crate) fn connect_error(origin: &str, message: String) -> String {
    noded_client::words::unreachable(origin, &message).unwrap_or_else(|| user_error(message))
}

/// Reads a device file through `parse`. A missing file is `Ok(None)`. A file
/// that will not parse is not trusted and not deleted: it is renamed to
/// `<name>.bad` (one kept, a newer one replaces it), warned about once with
/// its path and the error, and answers `Ok(None)` like a missing one. Any
/// other read error (permission, IO) stays an `Err`.
pub(crate) fn read_or_set_aside<T, E: std::fmt::Display>(
    path: &Path,
    parse: impl FnOnce(&[u8]) -> Result<T, E>,
) -> std::io::Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match parse(&bytes) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(error) => {
            let mut bad = path.as_os_str().to_owned();
            bad.push(".bad");
            std::fs::rename(path, &bad)?;
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "unreadable file set aside as .bad, starting empty");
            Ok(None)
        }
    }
}

/// Takes `path` away from this process's reads, for a test of a file the
/// device cannot read. False, saying so, where the user reads a file whatever
/// its mode (root, as in a container): the case cannot be staged there.
#[cfg(test)]
pub(crate) fn unreadable_for_test(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let refused = std::fs::File::open(path).is_err();
    if !refused {
        eprintln!(
            "skipping: this user reads {} whatever its mode (root?)",
            path.display()
        );
    }
    refused
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_error_never_lets_a_cli_command_through() {
        assert_eq!(
            user_error("no wallet — run `ducktape wallet new <name>` first".into()),
            "This device's key needs attention: try again, or read without a key."
        );
        assert_eq!(
            user_error("corrupt or wrong password".into()),
            "Wrong password."
        );
    }
}
