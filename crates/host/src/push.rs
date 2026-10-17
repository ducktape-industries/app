//! The device registry's wire: what a device tells the push server
//! (`crates/pushd`) so the server carries an account's notices to it. One
//! shape, kept here so the phone that signs it and the server that checks
//! it compile the same bytes.
//!
//! A [`Registration`] names the chain, the account, the key the device
//! holds on that account, which kind of token it is (`voip`, `apns`,
//! `fcm`) and the token itself (empty: the device is leaving). It is signed
//! by that key over [`NAMESPACE`] and [`Registration::preimage`], the way a
//! frame to the node is (`noded_client::Signer::sign`, commonware's
//! `union_unique` framing). The server accepts it when the signature
//! verifies, `at` is within its window and newer than the last it kept for
//! the key and kind (a replayed one changes nothing), and the chain's
//! identity program says the key acts as the account.
//!
//! The server's address is the device's own setting (`push_server` in the
//! prefs), the production server when none is set.

use serde::{Deserialize, Serialize};

/// What the key signs over: a registration, never a frame to a node.
pub const NAMESPACE: &[u8] = b"ducktape-push";

/// Where a registration goes, under the server's origin.
pub const ROUTE: &str = "/v1/devices";

/// The push server a device registers with when its prefs name none.
pub const DEFAULT_SERVER: &str = "https://push.ducktape.industries";

/// How far from the server's clock a registration's `at` may be, in
/// milliseconds: a phone's clock is seldom a minute off, and a message
/// older than this is refused outright, whatever the server kept.
pub const AT_WINDOW_MS: u64 = 5 * 60 * 1000;

/// One device's token on one account of one chain, signed by the device's
/// key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    /// The chain as `duck://` links spell it (`ducklink::ChainId`).
    pub chain: String,
    pub account: u64,
    /// The device's Ed25519 public key on that chain, hex.
    pub key: String,
    /// `voip` (an iPhone's PushKit token, for a call), `apns` (its
    /// remote-notification token, for a notice and a withdraw), `fcm` (an
    /// Android phone's Firebase token, for everything).
    pub kind: String,
    /// The OS's token, hex for APNs; empty when the device is leaving.
    pub token: String,
    /// `production` or `sandbox`: which of Apple's servers takes an APNs
    /// token (a build signed for development gets sandbox tokens); empty
    /// for `fcm`.
    pub env: String,
    /// When it was signed, unix milliseconds.
    pub at: u64,
    /// The key's signature over [`Self::preimage`], hex.
    pub sig: String,
}

impl Registration {
    /// The bytes the key signs: each field but the signature, one per
    /// line, in this order.
    pub fn preimage(&self) -> Vec<u8> {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.chain, self.account, self.key, self.kind, self.token, self.env, self.at
        )
        .into_bytes()
    }
}

/// The push server this device registers with: the prefs' `push_server`,
/// else [`DEFAULT_SERVER`]; never with a trailing slash.
pub fn server() -> String {
    crate::backend::read_prefs()
        .ok()
        .and_then(|prefs| prefs["push_server"].as_str().map(str::to_owned))
        .filter(|server| !server.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preimage is every field but the signature, in a fixed order:
    /// a token or an account changed changes it.
    #[test]
    fn the_preimage_names_every_field_but_the_signature() {
        let registration = Registration {
            chain: "testkit-0a1b2c3d".into(),
            account: 15,
            key: "ab".into(),
            kind: "voip".into(),
            token: "cd".into(),
            env: "sandbox".into(),
            at: 1_700_000_000_000,
            sig: "ff".into(),
        };
        assert_eq!(
            registration.preimage(),
            b"testkit-0a1b2c3d\n15\nab\nvoip\ncd\nsandbox\n1700000000000"
        );
        let other = Registration {
            token: "ce".into(),
            ..registration.clone()
        };
        assert_ne!(other.preimage(), registration.preimage());
        let signed_again = Registration {
            sig: "00".into(),
            ..registration.clone()
        };
        assert_eq!(signed_again.preimage(), registration.preimage());
    }
}
