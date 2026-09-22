//! The app's side of the wire: a node's daemon ([`noded`]), the device's
//! key and preferences ([`session`]), and where views come from ([`views`]).
//! Nothing here names a program.

mod app_dirs;
pub(crate) mod noded;
pub(crate) mod passkey;
mod session;
pub(crate) mod views;

pub use app_dirs::app_log_path;
pub(crate) use app_dirs::{cache_dir, config_dir, state_dir};
pub(crate) use noded::{Client as RpcClient, Layer, Status as NodeStatus};
pub(crate) use session::*;

use std::time::Duration;

/// A refusal the NODE or the PROGRAM authored, carried through with its
/// own token; a transport failure gets the app's.
pub(crate) fn refused(error: noded::Error) -> view_wire::Refusal {
    use noded::Error;
    match error {
        Error::Refused(refusal) => view_wire::Refusal::new(refusal.reason, refusal.sentence),
        Error::Decode(refusal) => view_wire::Refusal::new("malformed_reply", refusal.sentence),
        Error::Failed { status, sentence } => {
            view_wire::Refusal::new("node_failed", format!("{status}: {sentence}"))
        }
        Error::Transport(sentence) => view_wire::Refusal::new("rpc_client", sentence),
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

/// A sentence for a connection attempt that never reached `origin`: the
/// raw transport error (a reqwest string, e.g. "error sending request for
/// url (...)") never reaches the screen.
pub(crate) fn connect_error(origin: &str, message: String) -> String {
    if message.contains("error sending request") {
        return format!("Can't reach {origin}. Check the address, or that the node is running.");
    }
    user_error(message)
}

/// One id, unique on this device, for a record a view mints.
pub(crate) fn fresh_id(prefix: &str) -> String {
    format!("{prefix}-{:x}-{}", epoch_nanos(), next_sequence())
}

fn epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
}

/// The next frame sequence this device signs with: time-ordered, so a
/// restart never re-uses one the node has seen.
pub(crate) fn next_sequence() -> u64 {
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

/// Whether `modifiers` hold the platform command key (⌘ on a Mac, Ctrl
/// elsewhere).
pub(crate) fn command_held(modifiers: gpui_kit::Modifiers) -> bool {
    match cfg!(target_os = "macos") {
        true => modifiers.platform,
        false => modifiers.control,
    }
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
        let first = next_sequence();
        let second = next_sequence();
        assert!(second > first);
        assert_eq!(
            hex_decode(&hex_encode(&[0, 255, 16])).unwrap(),
            vec![0, 255, 16]
        );
        assert!(hex_decode("abc").is_err());
    }
}
