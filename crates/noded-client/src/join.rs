//! Getting a device's key onto an account from another device already on
//! it. Identity's `AddKey` is signed by the key joining and consented to by
//! a key already on the account, of any scheme; so any of the account's
//! keys can let a new one in.
//!
//! The new device shows a short code; the person types it on a device
//! already on the account, compares a fingerprint, and approves. The two
//! never meet: they pass the request and the consent through the auth
//! host's relay (`/r/<id>`, one-shot, 300 s), in two slots named from the
//! code. What rides there is public (a public key, a signature over it): a
//! forged request is caught by the fingerprint, a forged consent by the
//! node.

use identity::{CONSENT_NAMESPACE, Consent, Op};
use sha2::{Digest as _, Sha256};

use crate::hex::{hex_decode, hex_encode};
use crate::identity::{admission, submit_signed};
use crate::relay::{POLL, post, slot, take};
use crate::{Client, Signer};

const CROCKFORD_BASE32_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A fresh code to type on the other device: 8 characters, 40 bits,
/// "KQ4M-9XPT".
pub fn new_code() -> String {
    let bits: u64 = rand::random::<u64>() >> 24;
    let chars: String = (0..8)
        .map(|nth| CROCKFORD_BASE32_ALPHABET[((bits >> (35 - nth * 5)) & 31) as usize] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// A typed code as the slots name it: case, spaces and dashes dropped, and
/// the letters people confuse with digits read as those digits.
pub fn normalized(code: &str) -> Option<String> {
    let folded: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (folded.len() == 8
        && folded
            .bytes()
            .all(|b| CROCKFORD_BASE32_ALPHABET.contains(&b)))
    .then_some(folded)
}

/// A key's fingerprint, to compare by eye on both screens: "A1F3 09C2".
pub fn fingerprint(key: &[u8]) -> String {
    let digest = Sha256::digest(key);
    let hex: String = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    format!("{} {}", &hex[..4], &hex[4..])
}

/// A new device asking to join: which network, and its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub network: String,
    pub key: Vec<u8>,
}

/// Puts `signer`'s key up under `code` on the relay at `relay` (the auth
/// host's origin), then waits for a device on the account to consent, and
/// joins with that consent under `label` (how Account lists the key).
pub async fn join_from_device(
    client: &Client,
    relay: &str,
    network: &str,
    code: &str,
    signer: &dyn Signer,
    label: &str,
) -> Result<(), String> {
    let code = normalized(code).ok_or("That code is malformed.")?;
    let device = signer.public_key();
    let request = serde_json::json!({ "v": 1, "network": network, "key": hex_encode(&device) });
    post(client, &slot(relay, &code, "request")?, request.to_string()).await?;
    let answer = slot(relay, &code, "consent")?;
    let json = loop {
        if let Some(json) = take(client, &answer).await? {
            break json;
        }
        rt::time::sleep(POLL).await;
    };
    let consent = parse_consent(&json)?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some(label.into()),
        consent,
    };
    submit_signed(client, network, &add, signer).await.map(drop)
}

pub fn parse_consent(json: &str) -> Result<Consent, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "The other device's answer is unreadable.")?;
    let hex = |field: &str| {
        value[field]
            .as_str()
            .and_then(|text| hex_decode(text).ok())
            .ok_or_else(|| format!("The other device's answer has no {field}."))
    };
    Ok(Consent {
        key: hex("key")?,
        account: value["account"]
            .as_u64()
            .ok_or("The other device's answer names no account.")?,
        expires_at: value["expires_at"]
            .as_u64()
            .ok_or("The other device's answer has no deadline.")?,
        proof: hex("proof")?,
    })
}

/// The request a new device put up under `code` on the relay at `relay`,
/// asked over `client`'s connection. Taking it empties the slot: one code,
/// one look.
pub async fn find_request(client: &Client, relay: &str, code: &str) -> Result<Request, String> {
    let code = normalized(code).ok_or("A code is 8 letters and digits, like KQ4M-9XPT.")?;
    let json = take(client, &slot(relay, &code, "request")?)
        .await?
        .ok_or("No device is waiting on that code. Check it, or start again on the new device.")?;
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "The new device's request is unreadable.")?;
    let key = value["key"]
        .as_str()
        .and_then(|text| hex_decode(text).ok())
        .filter(|key| key.len() == 32)
        .ok_or("The new device's request has no key.")?;
    Ok(Request {
        network: value["network"].as_str().unwrap_or_default().to_owned(),
        key,
    })
}

/// `signer` (a key on `account`) consents to `request`'s key joining, and
/// hands the consent to the new device under `code` on the relay at
/// `relay`.
pub async fn approve(
    client: &Client,
    relay: &str,
    network: &str,
    account: u64,
    code: &str,
    request: &Request,
    signer: &dyn Signer,
) -> Result<(), String> {
    if request.network != network {
        return Err(format!(
            "That device is on {}, not {network}. Approve it from a device on {}.",
            request.network, request.network
        ));
    }
    let code = normalized(code).ok_or("That code is malformed.")?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        request.key.clone(),
        account,
        signer,
    )
    .await?;
    let proof = signer
        .sign(CONSENT_NAMESPACE, &admission.preimage())
        .map_err(|refused| refused.to_string())?;
    let consent = serde_json::json!({
        "v": 1,
        "account": account,
        "key": hex_encode(&signer.public_key()),
        "expires_at": admission.expires_at,
        "proof": hex_encode(&proof),
    });
    post(client, &slot(relay, &code, "consent")?, consent.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_reads_back_however_it_is_typed() {
        for _ in 0..50 {
            let code = new_code();
            assert_eq!(code.len(), 9);
            let folded = normalized(&code).expect("its own code parses");
            assert_eq!(
                normalized(&code.to_lowercase().replace('-', " ")),
                Some(folded)
            );
        }
        assert_eq!(
            normalized("kq4m-9xpo"),
            Some("KQ4M9XP0".into()),
            "O reads as 0"
        );
        assert_eq!(normalized("KQ4M-9XP"), None);
        assert_eq!(normalized("KQ4M-9XPU"), None, "U is not in the alphabet");
    }

    #[test]
    fn a_consent_survives_the_trip() {
        let json = serde_json::json!({
            "v": 1, "account": 17, "key": hex_encode(&[1, 2]), "expires_at": 99, "proof": hex_encode(&[3]),
        })
        .to_string();
        let consent = parse_consent(&json).unwrap();
        assert_eq!((consent.account, consent.expires_at), (17, 99));
        assert_eq!((consent.key, consent.proof), (vec![1, 2], vec![3]));
        assert!(parse_consent("{}").is_err());
    }

    #[test]
    fn a_non_ascii_key_is_refused_not_a_crash() {
        let json = serde_json::json!({
            "account": 17, "key": "a\u{e9}a", "expires_at": 99, "proof": "00",
        })
        .to_string();
        assert!(parse_consent(&json).is_err());
    }
}
