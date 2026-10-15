//! A recovery key: an extra key on the account whose only copy is its 24
//! words on paper. Made from a device already on the account (this device
//! consents to it); typed on a new device, it consents to that device's
//! key. A phrase a password-locked key was minted with (before keys moved
//! into the OS) is such a key already. The other way onto an account
//! without a passkey, another device, is the host's `backend::join`.

use commonware_codec::DecodeExt as _;
use commonware_cryptography::{Signer as _, ed25519};
use identity::{CONSENT_NAMESPACE, Consent, Op, Query, Reply};
use noded_client::identity::{account_by_number, admission, ask, submit};
use noded_client::wire::{Frame, salt};

use super::identity::{DEVICE_KEY_LABEL, person, submit_seated};
use super::{RpcClient, anchor, query_signer, seated_key, seated_sign};

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
    match ask(client, network, Query::OfKey { key }, &*query_signer()).await? {
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
    person(&account_by_number(client, network, account, &*query_signer()).await?)?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        public,
        account,
        &*query_signer(),
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
        identity::PROGRAM,
        abi::encode(&add),
    )
    .map_err(|refused| refused.to_string())?;
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
    person(&account_by_number(client, network, account, &*query_signer()).await?)?;
    let device = seated_key().await.map_err(|refusal| refusal.message)?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        device.clone(),
        account,
        &*query_signer(),
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
    use noded_client::join;

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
