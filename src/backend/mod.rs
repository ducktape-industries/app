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

use std::time::Duration;

use view_wire::methods::refusal;

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
        return "This device's key needs attention — try Restore or New key.".into();
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
            "This device's key needs attention — try Restore or New key."
        );
        assert_eq!(
            user_error("corrupt or wrong password".into()),
            "Wrong password."
        );
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
