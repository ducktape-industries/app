//! A recovery key: an extra key on the account whose only copy is its 24
//! words on paper. Made from a device already on the account (this device
//! consents to it); typed on a new device, it consents to that device's
//! key. A phrase a password-locked key was minted with (before keys moved
//! into the OS) is such a key already. The other way onto an account
//! without a passkey, another device, is the host's `backend::join`.

use commonware_codec::DecodeExt as _;
use commonware_cryptography::{Signer as _, ed25519};
use identity::{CONSENT_NAMESPACE, Consent, Op, Query, Reply};
use noded_client::identity::{admission, ask, submit};
use noded_client::wire::{Frame, salt};

use super::identity::submit_seated;
use super::{RpcClient, anchor, query_signer, seated_key, seated_sign};

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
    match ask(
        client,
        network,
        Query::OfKey { key },
        &*query_signer(network),
    )
    .await?
    {
        Reply::Number(number) => Ok(number),
        _ => Err("identity answered something other than an account number".into()),
    }
}

pub(crate) async fn add_recovery_key(
    client: &RpcClient,
    network: &str,
    phrase: &str,
) -> Result<(), String> {
    let recovery = key_of_phrase(phrase)?;
    let public = recovery.public_key().as_ref().to_vec();
    let device = seated_key(network)
        .await
        .map_err(|refusal| refusal.message)?;
    let account = account_of(client, network, device.clone())
        .await?
        .ok_or("This device's key holds no account yet.")?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        public,
        account,
        &*query_signer(network),
    )
    .await?;
    let proof = seated_sign(&device, CONSENT_NAMESPACE, &admission.preimage())
        .await
        .map_err(|refusal| refusal.message)?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some("Recovery key".into()),
        consent: Consent {
            key: device,
            account,
            expires_at: admission.expires_at,
            proof,
        },
    };
    let anchor = anchor(client).await.map_err(|refusal| refusal.message)?;
    let target = client
        .bound(abi::role::IDENTITY)
        .await
        .map_err(|error| error.to_string())?;
    let frame = Frame::sign(
        &recovery,
        network.as_bytes(),
        anchor,
        Some(salt()),
        &target,
        abi::encode(&add),
    )
    .await
    .map_err(|refused| refused.to_string())?;
    submit(client, frame.encode()).await.map(drop)
}

pub(crate) async fn join_with_recovery_key(
    client: &RpcClient,
    network: &str,
    phrase: &str,
) -> Result<(), String> {
    let recovery = key_of_phrase(phrase)?;
    let account = account_of(client, network, recovery.public_key().as_ref().to_vec())
        .await?
        .ok_or_else(|| format!("That recovery key isn't on an account on {network}."))?;
    let device = seated_key(network)
        .await
        .map_err(|refusal| refusal.message)?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        device.clone(),
        account,
        &*query_signer(network),
    )
    .await?;
    let proof = recovery.sign(CONSENT_NAMESPACE, &admission.preimage());
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some(crate::os::DEVICE_LABEL.into()),
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
