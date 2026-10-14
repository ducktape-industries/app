//! Getting this device's key onto an account without a passkey. Identity's
//! `AddKey` is signed by the key joining and consented to by a key already
//! on the account, of any scheme; so any of the account's keys can let a
//! new one in.
//!
//! - **Another device.** `noded_client::join`, signed by the seated key,
//!   through the auth page's relay: the new device shows a short code, the
//!   person types it on a device already on the account, compares a
//!   fingerprint, and approves.
//! - **A recovery key.** An extra key on the account whose only copy is its
//!   24 words on paper. Made from a device already on the account (this
//!   device consents to it); typed on a new device, it consents to that
//!   device's key. A phrase a password-locked key was minted with (before
//!   keys moved into the OS) is such a key already.

use commonware_codec::DecodeExt as _;
use commonware_cryptography::{Signer as _, ed25519};
use identity::{CONSENT_NAMESPACE, Consent, Op, Query, Reply};
use noded_client::identity::{account_by_number, admission, ask, submit};
use noded_client::join::{self, Request};
use noded_client::wire::{Frame, salt};

use super::auth_page::auth_page;
use super::identity::{DEVICE_KEY_LABEL, person, submit_seated};
use super::{RpcClient, anchor, query_signer, seated_key, seated_sign, seated_signer};

// ---------- another device ----------

/// Puts this device's (seated) key up under `code`, waits for a device on
/// the account to consent, and joins with that consent.
pub(crate) async fn join_from_device(
    client: &RpcClient,
    network: &str,
    code: &str,
) -> Result<(), String> {
    let key = seated_signer().map_err(|refusal| refusal.message)?;
    join::join_from_device(client, &auth_page(), network, code, &key, DEVICE_KEY_LABEL).await
}

/// The request a new device put up under `code`, on the auth page's relay.
pub(crate) async fn find_request(client: &RpcClient, code: &str) -> Result<Request, String> {
    join::find_request(client, &auth_page(), code).await
}

/// This device (seated, on `account`) consents to `request`'s key joining.
pub(crate) async fn approve(
    client: &RpcClient,
    network: &str,
    account: u64,
    code: &str,
    request: &Request,
) -> Result<(), String> {
    let key = seated_signer().map_err(|refusal| refusal.message)?;
    join::approve(client, &auth_page(), network, account, code, request, &key).await
}

// ---------- recovery keys ----------

/// 24 fresh words: a recovery key not yet on any account.
pub(crate) fn new_recovery_phrase() -> String {
    keystore::userkey::mnemonic_of_seed(&rand::random::<[u8; 32]>())
}

fn key_of_phrase(phrase: &str) -> Result<ed25519::PrivateKey, String> {
    let seed = zeroize::Zeroizing::new(
        keystore::userkey::seed_of_mnemonic(phrase)
            .map_err(|_| "Those words aren't a recovery key — check each one.".to_string())?,
    );
    ed25519::PrivateKey::decode(seed.as_slice()).map_err(|_| "Those words make no key.".to_string())
}

async fn account_of(
    client: &RpcClient,
    network: &str,
    key: Vec<u8>,
) -> Result<Option<u64>, String> {
    match ask(client, network, Query::OfKey { key }, &query_signer()).await? {
        Reply::Number(number) => Ok(number),
        _ => Err("identity answered something other than an account number".into()),
    }
}

/// Puts the key `phrase` makes on the account this device's (seated) key
/// holds: this device consents, the recovery key signs its own `AddKey`.
pub(crate) async fn add_recovery_key(
    client: &RpcClient,
    network: &str,
    phrase: &str,
) -> Result<(), String> {
    let recovery = key_of_phrase(phrase)?;
    let public = recovery.public_key().as_ref().to_vec();
    let device = seated_key().await.map_err(|refusal| refusal.message)?;
    let account = account_of(client, network, device)
        .await?
        .ok_or("This device's key holds no account yet.")?;
    person(&account_by_number(client, network, account, &query_signer()).await?)?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        public,
        account,
        &query_signer(),
    )
    .await?;
    let (key, proof) = seated_sign(CONSENT_NAMESPACE, &admission.preimage())
        .await
        .map_err(|refusal| refusal.message)?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some("Recovery key".into()),
        consent: Consent {
            key,
            account,
            expires_at: admission.expires_at,
            proof,
        },
    };
    let anchor = anchor(client).await.map_err(|refusal| refusal.message)?;
    let frame = Frame::sign(
        &recovery,
        network.as_bytes(),
        anchor,
        Some(salt()),
        identity::MODULE,
        abi::encode(&add),
    );
    submit(client, frame.encode()).await.map(drop)
}

/// This device's (seated) key joins the account the key `phrase` makes is
/// on; that key consents.
pub(crate) async fn join_with_recovery_key(
    client: &RpcClient,
    network: &str,
    phrase: &str,
) -> Result<(), String> {
    let recovery = key_of_phrase(phrase)?;
    let account = account_of(client, network, recovery.public_key().as_ref().to_vec())
        .await?
        .ok_or_else(|| format!("That recovery key isn't on an account on {network}."))?;
    person(&account_by_number(client, network, account, &query_signer()).await?)?;
    let device = seated_key().await.map_err(|refusal| refusal.message)?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        device.clone(),
        account,
        &query_signer(),
    )
    .await?;
    let proof = recovery.sign(CONSENT_NAMESPACE, &admission.preimage());
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some(DEVICE_KEY_LABEL.into()),
        consent: Consent {
            key: recovery.public_key().as_ref().to_vec(),
            account,
            expires_at: admission.expires_at,
            proof: proof.as_ref().to_vec(),
        },
    };
    submit_seated(client, network, &add).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_phrase_makes_one_key_and_fingerprints_are_short() {
        let phrase = new_recovery_phrase();
        assert_eq!(phrase.split_whitespace().count(), 24);
        let a = key_of_phrase(&phrase).unwrap().public_key();
        let b = key_of_phrase(&phrase).unwrap().public_key();
        assert_eq!(a, b);
        assert!(key_of_phrase("not words").is_err());
        assert_eq!(join::fingerprint(a.as_ref()).len(), 9);
    }
}
