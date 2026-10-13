//! The release signature: an ed25519 key signs `RELEASE_NS ‖ manifest
//! bytes`, and the app verifies the same preimage under the key pinned in
//! this file ([`RELEASE_KEY`]) or its announced successor.
//!
//! The namespace is a fixed prefix, so the preimage is unambiguous and a
//! release signature means one thing only: the same key's signature over the
//! same bytes for any other purpose is not a release signature. The signature
//! file holds the 64 bytes as 128 hex characters.
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
/// release job's `RELEASE_SIGNING_KEY` secret. A new key takes over through a
/// signed [`crate::SuccessorKey`], never by editing this line alone.
pub const RELEASE_KEY: PublicKey = PublicKey([
    0x5b, 0x22, 0x2e, 0x31, 0x14, 0x94, 0x0c, 0xb1, 0x2f, 0x07, 0xbf, 0xe1, 0x97, 0xb1, 0xf5, 0x25,
    0x8c, 0x90, 0x5a, 0x04, 0x73, 0x40, 0x4a, 0x4a, 0xf8, 0x6c, 0x08, 0x58, 0x6c, 0x3c, 0x6f, 0x8e,
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

/// A release signature: 64 raw ed25519 bytes, hex in the `.sig` file.
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

    /// The `.sig` file's text: 128 hex characters and a newline.
    pub fn encoded(&self) -> String {
        let mut text = bytes_to_hex(&self.0);
        text.push('\n');
        text
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

    /// A throwaway signing key, deterministic per `seed`.
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

    /// The namespace is part of the preimage: the same key's plain signature
    /// over the same bytes is not a release signature.
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
        let encoded = signature.encoded();
        assert_eq!(encoded.len(), 129);
        assert_eq!(encoded.parse::<Signature>().unwrap(), signature);
        assert_eq!("abc".parse::<Signature>(), Err(SignatureParseError));
        assert_eq!("abc".parse::<PublicKey>(), Err(PublicKeyParseError));
    }

    /// The pinned key is the one the owner handed over, byte for byte.
    #[test]
    fn the_pinned_key_is_the_release_jobs_public_half() {
        assert_eq!(
            RELEASE_KEY.to_string(),
            "5b222e3114940cb12f07bfe197b1f5258c905a0473404a4af86c08586c3c6f8e"
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
