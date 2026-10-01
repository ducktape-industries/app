//! Everything that crosses the process boundary. The node daemon's client
//! ([`noded`]); the one signing key held in memory and the frames it signs
//! ([`session`]); this device's per-network key in the OS store
//! ([`device_key`]) and the key directory it falls back to ([`key_dir`]);
//! the identity program's client ([`identity`]) and the two ways onto an
//! account, a passkey ([`passkey`], through [`auth_page`], [`relay`] and
//! [`loopback`]) or another key ([`join`]); a view's wasm out of its
//! program's blob ([`views`]); prefs.json ([`prefs`]) and the recent-nodes
//! list ([`endpoints`]); the app's own directories ([`app_dirs`]).
//!
//! No user program is named here. Two system programs are: `identity`
//! (accounts and their keys) and `module-registry` (the roster).

mod app_dirs;
pub(crate) mod auth_page;
pub(crate) mod device_key;
mod endpoints;
pub(crate) mod identity;
pub(crate) mod join;
mod key_dir;
mod loopback;
pub(crate) mod noded;
pub(crate) mod passkey;
mod prefs;
mod relay;
mod session;
pub(crate) mod views;

pub use app_dirs::app_log_path;
pub(crate) use app_dirs::{cache_dir, config_dir, state_dir};
pub(crate) use endpoints::{
    DEFAULT_ENDPOINT, ENDPOINT_REFUSAL, RecentEndpoint, endpoint_origin, forget_endpoint, host_of,
    note_endpoint, recent_endpoints,
};
pub(crate) use key_dir::{Keyring, bind_keyring, key_exists, keystore_root, session_key_path};
pub(crate) use noded::{Client as RpcClient, Layer, Status as NodeStatus};
pub(crate) use prefs::{
    Appearance, load_appearance, load_motion, read_prefs, save_appearance, save_motion, write_prefs,
};
#[cfg(test)]
pub(crate) use session::seat_serial;
pub(crate) use session::{
    lock_signer, next_seq, query_frame, seat_key, seated_frame, seated_key, seated_sign,
};

use std::path::Path;
use std::time::Duration;

/// A refusal the NODE or the PROGRAM authored, carried through with its
/// own token; a transport failure gets the app's: `rpc_client` when nothing
/// reached the node, `node_failed` when the request may have and the answer
/// is what went missing — a write's caller must not treat the two alike.
pub(crate) fn refused(error: noded::Error) -> view_wire::Error {
    use noded::Error;
    match error {
        Error::Refused(refusal) => view_wire::Error::new(refusal.reason, refusal.sentence),
        Error::Decode(refusal) => {
            view_wire::Error::new(view_wire::code::UNEXPECTED_REPLY, refusal.sentence)
        }
        Error::Failed { status, sentence } => {
            view_wire::Error::new("node_failed", format!("{status}: {sentence}"))
        }
        Error::Unreachable(sentence) => view_wire::Error::new("rpc_client", sentence),
        Error::Transport(sentence) => {
            view_wire::Error::new("node_failed", format!("no answer came back: {sentence}"))
        }
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
        return "This device's key needs attention. Try again, or read without a key.".into();
    }
    message
}

/// A sentence for a connection attempt that never reached `origin`. The
/// common shapes — reqwest's "error sending request for url (...)", the
/// connect step's own timeout — are reworded; any other text falls through
/// [`user_error`] as it is.
pub(crate) fn connect_error(origin: &str, message: String) -> String {
    if message.contains("error sending request") {
        return format!("Can't reach {origin}. Check the address, or that the node is running.");
    }
    if message.contains("did not answer in time") {
        return format!(
            "{origin} has not answered. Check that the node is running, then Connect again."
        );
    }
    user_error(message)
}

/// Writes `bytes` to `path` whole or not at all: into a temp file beside
/// it, synced, then renamed over it. A crash or a full disk mid-write
/// leaves the old file (or none), never a torn one. `private`: only this
/// user can read it (unix).
pub(crate) fn atomic_write(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?
        .to_os_string();
    name.push(".tmp");
    let temp = path.with_file_name(name);
    // a temp left by a cut write keeps its mode when reopened: start afresh
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let written = options
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// One id, unique on this device, for a record a view mints.
pub(crate) fn fresh_id(prefix: &str) -> String {
    format!("{prefix}-{:x}-{}", epoch_nanos(), fresh_counter())
}

fn epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
}

/// Only ever climbs within this process (1, then the clock's microseconds
/// or the last value plus one, whichever is later); only [`fresh_id`] mixes
/// it in. Not a frame sequence — that is `session::next_seq`, which the
/// node dictates.
pub(crate) fn fresh_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = (epoch_nanos() / 1_000) as u64;
    LAST.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
        Some(now.max(last + 1))
    })
    .unwrap_or(now)
        + 1
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&value[at..at + 2], 16).map_err(|error| error.to_string()))
        .collect()
}

/// One second, doubling to sixteen, for a request that retries.
pub(crate) fn retry_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(4);
    Duration::from_secs(1_u64 << exponent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_failure_names_the_node_not_the_raw_error() {
        let sentence = connect_error(
            "http://127.0.0.1:33835",
            "error sending request for url (http://127.0.0.1:33835/v1/status)".into(),
        );
        assert_eq!(
            sentence,
            "Can't reach http://127.0.0.1:33835. Check the address, or that the node is running."
        );
    }

    #[test]
    fn user_error_never_lets_a_cli_command_through() {
        assert_eq!(
            user_error("no wallet — run `ducktape wallet new <name>` first".into()),
            "This device's key needs attention. Try again, or read without a key."
        );
        assert_eq!(
            user_error("corrupt or wrong password".into()),
            "Wrong password."
        );
    }

    /// A write cut short (here by a file-size limit of 0 in a child run of
    /// this test, the way a full disk cuts it) leaves the file it was
    /// replacing whole; one that lands replaces it, readable by its owner
    /// only, and leaves no temp behind.
    #[cfg(unix)]
    #[test]
    fn a_write_cut_short_leaves_the_old_file_whole() {
        const CUT: &str = "DUCKTAPE_TEST_CUT_WRITE";
        if let Some(path) = std::env::var_os(CUT) {
            let cut = atomic_write(Path::new(&path), &[1u8; 32], true);
            assert!(cut.is_err(), "the write was not cut short");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        std::fs::write(&path, [7u8; 32]).unwrap();
        let test = concat!(
            module_path!(),
            "::a_write_cut_short_leaves_the_old_file_whole"
        );
        let test = test.split_once("::").unwrap().1;
        let child = std::process::Command::new("sh")
            .args([
                "-c",
                r#"trap "" XFSZ; ulimit -f 0 && exec "$0" --exact "$1" --test-threads=1"#,
            ])
            .arg(std::env::current_exe().unwrap())
            .arg(test)
            .env(CUT, &path)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success() && said.contains("1 passed"),
            "{}: {said}",
            child.status
        );
        assert_eq!(std::fs::read(&path).unwrap(), [7u8; 32], "torn");
        assert!(!dir.path().join("device.key.tmp").exists());

        atomic_write(&path, &[1u8; 32], true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), [1u8; 32]);
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
        assert!(!dir.path().join("device.key.tmp").exists());
    }

    #[test]
    fn sequences_climb_and_hex_round_trips() {
        let first = fresh_counter();
        let second = fresh_counter();
        assert!(second > first);
        assert_eq!(
            hex_decode(&hex_encode(&[0, 255, 16])).unwrap(),
            vec![0, 255, 16]
        );
        assert!(hex_decode("abc").is_err());
    }
}
