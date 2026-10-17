//! Everything that crosses the process boundary and is every host's: the
//! node daemon's client is `noded_client` (`crates/noded-client`), shared
//! with the phone; here is the one signing key held in memory and the
//! frames it signs ([`session`]); the identity flows onto an account
//! ([`identity`]) and joining it from another device ([`join`]); a view's
//! wasm out of its program's blob ([`views`]); prefs.json ([`prefs`]) and
//! the recent-nodes list ([`endpoints`]); where the host keeps its files
//! ([`dirs`], the shell's word). What only the desktop runs (this device's
//! key in the OS store, the key directory, passkeys through the auth page
//! and its loopback listener, recovery keys, invite minting with the node
//! this machine runs) stays in the desktop shell's `backend/`.

mod endpoints;
pub mod identity;
pub mod join;
pub mod live;
mod prefs;
mod session;
pub mod views;

/// Where the host keeps its files: the shell's word (`contract::Dirs`).
pub fn dirs() -> &'static crate::contract::Dirs {
    crate::contract::os().dirs()
}
pub use endpoints::{RecentEndpoint, forget_endpoint, note_endpoint, recent_endpoints};
pub use noded_client::Client as RpcClient;
pub use noded_client::wire::{Layer, Status as NodeStatus};
#[cfg(any(test, feature = "test-support"))]
pub use prefs::prefs_reads;
pub use prefs::{
    Appearance, Layout, edit_prefs, load_appearance, load_debug, load_layout, load_motion,
    memory_bounds, read_prefs, save_appearance, save_debug, save_layout, save_motion,
};
#[cfg(any(test, feature = "test-support"))]
pub use session::seat_serial;
pub use session::{
    anchor, lock_signer, query_frame, query_signer, seat_key, seated, seated_frame, seated_key,
    seated_sign, seated_signer,
};

use std::path::Path;

use view_wire::methods::refusal;

/// The live auth page. Its host IS the RP ID every passkey is scoped to,
/// and its relay carries a device join.
pub const AUTH_PAGE: &str = "https://auth.ducktape.industries/";

/// The auth page this build talks to: `DUCKTAPE_AUTH_PAGE`, else the live one.
pub fn auth_page() -> String {
    std::env::var("DUCKTAPE_AUTH_PAGE").unwrap_or_else(|_| AUTH_PAGE.to_owned())
}

/// A client for the node at `base`, timed as `io.rpc_client`.
pub fn rpc_client(base: impl Into<String>) -> RpcClient {
    let _timed = crate::perf::time(crate::perf::Key::Shell, "io.rpc_client");
    RpcClient::new(base)
}

/// The roles a test node's status names.
#[cfg(any(test, feature = "test-support"))]
pub fn test_roles() -> abi::Roles {
    abi::Roles::from_iter([
        (abi::role::PROGRAMS, "programs"),
        (abi::role::VALIDATORS, "valset"),
        (abi::role::IDENTITY, "identity"),
        (abi::role::BLOBS, "blobs"),
        (abi::role::AWAITS, "awaits"),
    ])
}

/// A refusal the NODE or the PROGRAM authored, carried through with its
/// own token; a transport failure gets the app's: `rpc_client` when nothing
/// reached the node, `node_failed` when the request may have and the answer
/// is what went missing — a write's caller must not treat the two alike.
pub fn refused(error: noded_client::Error) -> view_wire::Error {
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
pub fn user_error(message: String) -> String {
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

/// A sentence for a connection attempt that never reached a node at
/// `host`: the common shapes are `noded_client::words::unreachable`'s, in
/// this device's noun; any other text falls through [`user_error`] as it is.
pub fn connect_error(host: &str, message: String) -> String {
    let device = crate::contract::os().facts().device_noun;
    noded_client::words::unreachable(host, &message, device).unwrap_or_else(|| user_error(message))
}

/// Reads a device file through `parse`. A missing file is `Ok(None)`. A file
/// that will not parse is not trusted and not deleted: it is renamed to
/// `<name>.bad` (one kept, a newer one replaces it), warned about once with
/// its path and the error, and answers `Ok(None)` like a missing one. Any
/// other read error (permission, IO) stays an `Err`.
pub fn read_or_set_aside<T, E: std::fmt::Display>(
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
#[cfg(any(test, feature = "test-support"))]
pub fn unreadable_for_test(path: &Path) -> bool {
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
