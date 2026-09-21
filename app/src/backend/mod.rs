//! The app's side of the wire: a node's daemon ([`noded`]), the device's
//! key and preferences ([`session`]), and where views come from ([`views`]).
//! Nothing here names a program.

mod app_dirs;
pub(crate) mod noded;
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
        return "That password did not open this device's key. Check it and try again.".into();
    }
    let node_slow = message.contains("timed out");
    if node_slow {
        return "The node did not answer in time. Retry in a moment.".into();
    }
    message
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
