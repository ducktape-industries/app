//! The release signature: an ed25519 key signs `RELEASE_NS ‖ manifest
//! bytes`, and the app verifies the same preimage under the key pinned in
//! this file ([`RELEASE_KEY`]) or its announced successor.
//!
//! The namespace is a fixed prefix, so the preimage is unambiguous and a
//! release signature means one thing only: the same key's signature over the
//! same bytes for any other purpose is not a release signature. A channel's
//! file carries the 64 bytes as 128 hex characters beside the manifest
//! ([`crate::SignedManifest`]).
//!
//! Everything a signature binds is INSIDE the signed body: `channel`,
//! `sequence` and `successor_key` are manifest fields, so nothing rides
//! outside the signature. The bucket the files are served from is not
//! trusted: integrity is the signature plus the monotonic sequence the app
//! pins, never the transport or the path.

use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::sha::{bytes_to_hex, hex_to_array};

/// The prefix of every signed preimage.
pub const RELEASE_NS: &[u8] = b"ducktape:app-release";

/// The release key every install trusts: the public half of the seed in the
/// `RELEASE_SIGNING_KEY` secret of the repo's `release` environment. A new
/// key takes over through a signed [`crate::SuccessorKey`], never by editing
/// this line alone.
pub const RELEASE_KEY: PublicKey = PublicKey([
    0x37, 0x21, 0x17, 0x18, 0x04, 0x41, 0x1e, 0x59, 0x32, 0x81, 0x78, 0xff, 0x23, 0x46, 0x8b, 0xd7,
    0x57, 0x3b, 0x9c, 0x15, 0xc8, 0xdb, 0xaf, 0x63, 0x12, 0xb5, 0x46, 0x11, 0x27, 0x73, 0xda, 0x87,
]);

/// A release-signing public key: 32 raw ed25519 bytes, hex on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        PublicKey(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The public half of a signing key.
    pub fn of(signer: &SigningKey) -> Self {
        PublicKey(signer.verifying_key().to_bytes())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&bytes_to_hex(&self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

/// The text was not 64 hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKeyParseError;

impl fmt::Display for PublicKeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected 64 hex characters of ed25519 public key")
    }
}

impl std::error::Error for PublicKeyParseError {}

impl FromStr for PublicKey {
    type Err = PublicKeyParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        hex_to_array(text.trim())
            .map(PublicKey)
            .ok_or(PublicKeyParseError)
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// A release signature: 64 raw ed25519 bytes, hex in a channel's file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature([u8; 64]);

impl Signature {
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Signature(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    /// Sign `manifest_bytes` under [`RELEASE_NS`]. The bytes signed are the
    /// manifest file's bytes exactly as published: the verifier checks the
    /// file, not a re-serialization.
    pub fn sign(signer: &SigningKey, manifest_bytes: &[u8]) -> Self {
        Signature(signer.sign(&preimage(manifest_bytes)).to_bytes())
    }

    /// `true` when this signature is `pubkey`'s over `manifest_bytes` under
    /// [`RELEASE_NS`]. Strict: no malleable or small-order forms.
    pub fn verifies(&self, pubkey: &PublicKey, manifest_bytes: &[u8]) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(pubkey.as_bytes()) else {
            return false;
        };
        let signature = ed25519_dalek::Signature::from_bytes(&self.0);
        key.verify_strict(&preimage(manifest_bytes), &signature)
            .is_ok()
    }
}

/// 128 hex characters.
impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&bytes_to_hex(&self.0))
    }
}

fn preimage(manifest_bytes: &[u8]) -> Vec<u8> {
    [RELEASE_NS, manifest_bytes].concat()
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({})", bytes_to_hex(&self.0))
    }
}

/// The text was not 128 hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureParseError;

impl fmt::Display for SignatureParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected 128 hex characters of ed25519 signature")
    }
}

impl std::error::Error for SignatureParseError {}

impl FromStr for Signature {
    type Err = SignatureParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        hex_to_array(text.trim())
            .map(Signature)
            .ok_or(SignatureParseError)
    }
}

/// A 32-byte ed25519 seed from its 64 hex characters (the release job's
/// `RELEASE_SIGNING_KEY`), or `None`.
pub fn signing_key_from_hex(text: &str) -> Option<SigningKey> {
    hex_to_array(text.trim()).map(|seed| SigningKey::from_bytes(&seed))
}

#[cfg(test)]
pub(crate) mod testkit {
    use super::*;

    pub(crate) fn key_pair(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::key_pair;
    use super::*;

    #[test]
    fn sign_then_verify_round_trips_under_the_release_namespace() {
        let signer = key_pair(1);
        let pubkey = PublicKey::of(&signer);
        let signature = Signature::sign(&signer, b"manifest");
        assert!(signature.verifies(&pubkey, b"manifest"));
        assert!(!signature.verifies(&pubkey, b"manifesT"));
        assert!(!signature.verifies(&PublicKey::of(&key_pair(2)), b"manifest"));
    }

    #[test]
    fn a_signature_without_the_namespace_does_not_verify() {
        let signer = key_pair(1);
        let pubkey = PublicKey::of(&signer);
        let foreign = Signature::from_bytes(signer.sign(b"manifest").to_bytes());
        assert!(!foreign.verifies(&pubkey, b"manifest"));
    }

    #[test]
    fn hex_codecs_round_trip() {
        let signer = key_pair(3);
        let pubkey = PublicKey::of(&signer);
        assert_eq!(pubkey.to_string().len(), 64);
        assert_eq!(pubkey.to_string().parse::<PublicKey>().unwrap(), pubkey);
        assert_eq!(
            serde_json::from_str::<PublicKey>(&serde_json::to_string(&pubkey).unwrap()).unwrap(),
            pubkey
        );
        let signature = Signature::sign(&signer, b"x");
        let encoded = signature.to_string();
        assert_eq!(encoded.len(), 128);
        assert_eq!(encoded.parse::<Signature>().unwrap(), signature);
        assert_eq!("abc".parse::<Signature>(), Err(SignatureParseError));
        assert_eq!("abc".parse::<PublicKey>(), Err(PublicKeyParseError));
    }

    #[test]
    fn the_pinned_key_is_the_release_jobs_public_half() {
        assert_eq!(
            RELEASE_KEY.to_string(),
            "3721171804411e59328178ff23468bd7573b9c15c8dbaf6312b546112773da87"
        );
    }

    #[test]
    fn a_seed_parses_from_hex_and_signs_as_its_public_half() {
        let seed = "07".repeat(32);
        let signer = signing_key_from_hex(&format!("{seed}\n")).unwrap();
        assert_eq!(PublicKey::of(&signer), PublicKey::of(&key_pair(7)));
        assert!(signing_key_from_hex("abc").is_none());
    }
}
