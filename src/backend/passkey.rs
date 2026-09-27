//! Passkeys: an ACCOUNT's key, held by the person's authenticator. This
//! device's ed25519 key still signs every write; a passkey only creates the
//! account with it, and admits a new device's key into it later.
//!
//! The app speaks no WebAuthn. The ceremony runs on the auth page
//! ([`auth_page`](super::auth_page), RP ID = its host) in the system
//! browser: the request rides the URL fragment, and the result comes back
//! as a top-level form POST (`result=<JSON>`) to a one-shot loopback
//! listener ([`loopback`](super::loopback)). The verifier every answer
//! must satisfy is `keyscheme` (`Secp256r1` = the assertion envelope
//! `authenticatorData ‖ clientDataJSON ‖ sig64`, challenge
//! `SHA-256(ns ‖ preimage)`).
//!
//! The contract's `user.id` names the account NUMBER, which the identity
//! program assigns at `Create`. So a passkey account is created by this
//! device's key, and the passkey joins it next: touch 1 `create`s the
//! passkey, touch 2 signs its own `AddKey` frame (the device key consents).
//! A new device is two touches too: touch 1 asks the passkey which account
//! it holds, touch 2 consents to this device's key joining that account.
//!
//! A passkey on a phone: every touch also offers its request as a QR
//! ([`Phone`]) whose callback is the auth host's relay slot `/r/<id>`
//! ([`relay`](super::relay)). Once the person picks the phone, touches stop
//! opening this device's browser and the app polls the slot.

use identity::{CONSENT_NAMESPACE, Consent, Op, Query, Reply};
use keyscheme::KeyScheme;
use sha2::{Digest as _, Sha256};

use super::auth_page::{Assertion, Phone, Request, asserted, created};
use super::identity::{
    account_by_number, admission, ask, create_seated, person, submit, submit_seated,
};
use super::noded::{Body, FRAME_NAMESPACE, Frame};
use super::{RpcClient, next_seq, seated_key, seated_sign};

// ---------- the flows ----------

/// A new account held by a passkey, with this device's (seated) key on it
/// too. Retry-safe: a device key that already holds an account keeps it,
/// and the passkey joins that one.
pub(crate) async fn create_account(
    client: &RpcClient,
    network: &str,
    name: &str,
    phone: &Phone,
) -> Result<(), String> {
    let device = seated_key().await.map_err(|refusal| refusal.message)?;
    let number = match ask(
        client,
        network,
        Query::OfKey {
            key: device.clone(),
        },
    )
    .await?
    {
        Reply::Number(Some(number)) => {
            person(&account_by_number(client, network, number).await?)?;
            number
        }
        _ => create_seated(client, network, name).await?,
    };
    let passkey = created(
        Request::Create {
            user: user_handle(network, number),
            name: format!("{name} · {network}"),
        },
        phone,
    )
    .await?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Secp256r1,
        passkey.clone(),
        number,
    )
    .await?;
    let (device, proof) = seated_sign(CONSENT_NAMESPACE, &admission.preimage())
        .await
        .map_err(|refusal| refusal.message)?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Secp256r1,
        label: Some("Passkey".into()),
        consent: Consent {
            key: device,
            account: number,
            expires_at: admission.expires_at,
            proof,
        },
    };
    let seq = next_seq(client, &passkey)
        .await
        .map_err(|refusal| refusal.message)?;
    let body = passkey_body(&passkey, network, seq, abi::encode(&add));
    let assertion = asserted(
        keyscheme::webauthn_challenge(FRAME_NAMESPACE, &body.preimage()),
        phone,
    )
    .await?;
    let frame = passkey_frame(body, &assertion)
        .ok_or("That was a different passkey than the one just made. Choose the new one.")?;
    submit(client, frame.encode()).await.map(drop)
}

/// This device's (seated) key joins the account a passkey holds.
pub(crate) async fn sign_in(
    client: &RpcClient,
    network: &str,
    phone: &Phone,
) -> Result<(), String> {
    let hint = asserted(rand::random(), phone).await?;
    let number = account_of_handle(network, hint.user_handle.as_deref())?;
    let account = account_by_number(client, network, number).await?;
    person(&account)?;
    let device = seated_key().await.map_err(|refusal| refusal.message)?;
    let admission = admission(
        client,
        network,
        abi::Scheme::Ed25519,
        device.clone(),
        number,
    )
    .await?;
    let preimage = admission.preimage();
    let consent = asserted(
        keyscheme::webauthn_challenge(CONSENT_NAMESPACE, &preimage),
        phone,
    )
    .await?;
    let proof = consent.proof();
    let key = consenting_key(&account, &preimage, &proof)
        .ok_or("That passkey isn't on this account. Use the same passkey both times.")?;
    let add = Op::AddKey {
        scheme: abi::Scheme::Ed25519,
        label: Some("Desktop".into()),
        consent: Consent {
            key,
            account: number,
            expires_at: admission.expires_at,
            proof,
        },
    };
    submit_seated(client, network, &add).await.map(drop)
}

/// Which of `account`'s passkeys signed `proof` over the consent `preimage`
/// — the page does not say which credential answered.
fn consenting_key(account: &identity::Account, preimage: &[u8], proof: &[u8]) -> Option<Vec<u8>> {
    account
        .keys()
        .iter()
        .filter(|held| held.scheme == abi::Scheme::Secp256r1)
        .find(|held| KeyScheme::Secp256r1.verify(&held.key, CONSENT_NAMESPACE, preimage, proof))
        .map(|held| held.key.clone())
}

/// A frame body whose signer is the passkey `pubkey`.
pub(super) fn passkey_body(pubkey: &[u8], network: &str, seq: u64, payload: Vec<u8>) -> Body {
    Body {
        scheme: KeyScheme::Secp256r1,
        signer: pubkey.to_vec(),
        network: network.as_bytes().to_vec(),
        seq,
        target: identity::MODULE.to_owned(),
        payload,
    }
}

/// The frame `body` completed by `assertion`, or `None` when the assertion
/// is not the body's signer signing it (another passkey answered).
pub(super) fn passkey_frame(body: Body, assertion: &Assertion) -> Option<Frame> {
    let proof = assertion.proof();
    KeyScheme::Secp256r1
        .verify(&body.signer, FRAME_NAMESPACE, &body.preimage(), &proof)
        .then_some(Frame { body, proof })
}

// ---------- the account a passkey names ----------

/// `user.id`: `SHA-256("ducktape:passkey-account:v1\0" ‖ network)` ‖ the
/// account number, u64 LE.
fn user_handle(network: &str, number: u64) -> [u8; 40] {
    let mut handle = [0; 40];
    handle[..32].copy_from_slice(&network_tag(network));
    handle[32..].copy_from_slice(&number.to_le_bytes());
    handle
}

fn network_tag(network: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"ducktape:passkey-account:v1\0");
    hash.update(network.as_bytes());
    hash.finalize().into()
}

/// The account a passkey's (unsigned) `userHandle` names on `network`. A
/// hint only: the consent that follows must verify against a key the
/// account holds.
fn account_of_handle(network: &str, handle: Option<&[u8]>) -> Result<u64, String> {
    let handle = handle
        .and_then(|bytes| <&[u8; 40]>::try_from(bytes).ok())
        .ok_or("That passkey wasn't made by Ducktape. Choose a Ducktape passkey.")?;
    if handle[..32] != network_tag(network) {
        return Err(format!(
            "That passkey belongs to another network. Choose one for {network}."
        ));
    }
    Ok(u64::from_le_bytes(
        handle[32..].try_into().expect("8 bytes"),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::auth_page::{Outcome, parse_result};
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use identity::Admission;
    use identity::Control;
    use keyscheme::testkit;

    const RP: &str = "auth.ducktape.industries";

    /// The page's `get` answer, as JSON, for an assertion made the way an
    /// authenticator makes it.
    fn page_answer(
        sk: &p256::ecdsa::SigningKey,
        ns: &[u8],
        preimage: &[u8],
        handle: &[u8],
    ) -> Assertion {
        let (authenticator_data, client_data_json, signature) =
            testkit::passkey_assertion_parts(sk, RP, ns, preimage);
        let json = serde_json::json!({
            "op": "get",
            "credentialId": B64.encode([9; 16]),
            "authenticatorData": B64.encode(authenticator_data),
            "clientDataJSON": B64.encode(client_data_json),
            "signature": B64.encode(signature),
            "userHandle": B64.encode(handle),
        });
        match parse_result(&json.to_string()).unwrap() {
            Outcome::Asserted(assertion) => assertion,
            Outcome::Created(_) => unreachable!(),
        }
    }

    #[test]
    fn a_passkey_signed_frame_verifies_as_the_node_verifies_it() {
        let sk = testkit::passkey(3);
        let pubkey = testkit::passkey_pubkey(&sk);
        let body = passkey_body(&pubkey, "testkit", 0, vec![1, 2, 3]);
        let assertion = page_answer(&sk, FRAME_NAMESPACE, &body.preimage(), &[]);
        let frame = passkey_frame(body, &assertion).expect("its own signer");
        // the node decodes the bytes and runs `scheme.verify(signer, NS, preimage, proof)`
        let decoded: Frame = abi::decode(&frame.encode()).unwrap();
        assert_eq!(decoded.body.scheme, KeyScheme::Secp256r1);
        assert!(decoded.body.scheme.verify(
            &decoded.body.signer,
            FRAME_NAMESPACE,
            &decoded.body.preimage(),
            &decoded.proof
        ));

        // another passkey answering is caught before the node sees it
        let other = page_answer(
            &testkit::passkey(4),
            FRAME_NAMESPACE,
            &decoded.body.preimage(),
            &[],
        );
        assert!(passkey_frame(decoded.body, &other).is_none());
    }

    #[test]
    fn a_passkey_consent_verifies_and_names_the_key_that_gave_it() {
        let sk = testkit::passkey(5);
        let admission = Admission {
            network: b"testkit".to_vec(),
            scheme: abi::Scheme::Ed25519,
            key: vec![7; 32],
            generation: 0,
            account: 12,
            expires_at: 99,
        };
        let preimage = admission.preimage();
        let assertion = page_answer(
            &sk,
            CONSENT_NAMESPACE,
            &preimage,
            &user_handle("testkit", 12),
        );
        assert_eq!(
            account_of_handle("testkit", assertion.user_handle.as_deref()),
            Ok(12)
        );
        let key = |sk| identity::Key {
            scheme: abi::Scheme::Secp256r1,
            key: testkit::passkey_pubkey(sk),
            label: None,
            added_at: 0,
        };
        let account = identity::Account {
            number: 12,
            card: identity::Card {
                name: "ada".into(),
                avatar: None,
                bio: None,
                updated_at: 0,
            },
            control: Control::Person {
                keys: vec![key(&testkit::passkey(6)), key(&sk)],
            },
        };
        let proof = assertion.proof();
        let signer = consenting_key(&account, &preimage, &proof).unwrap();
        assert_eq!(signer, testkit::passkey_pubkey(&sk));
        assert!(KeyScheme::Secp256r1.verify(&signer, CONSENT_NAMESPACE, &preimage, &proof));
        // bound to its admission: another account's preimage does not verify
        let elsewhere = Admission {
            account: 13,
            ..admission
        }
        .preimage();
        assert!(consenting_key(&account, &elsewhere, &proof).is_none());
    }

    #[test]
    fn a_device_consent_admits_a_passkey() {
        use commonware_cryptography::Signer as _;
        let device = commonware_cryptography::ed25519::PrivateKey::from_seed(1);
        let preimage = b"admission".to_vec();
        let proof = device.sign(CONSENT_NAMESPACE, &preimage);
        assert!(KeyScheme::Ed25519.verify(
            device.public_key().as_ref(),
            CONSENT_NAMESPACE,
            &preimage,
            proof.as_ref()
        ));
    }

    #[test]
    fn a_user_handle_names_its_network_and_account() {
        let handle = user_handle("testkit", 258);
        assert_eq!(account_of_handle("testkit", Some(&handle)), Ok(258));
        assert!(account_of_handle("other", Some(&handle)).is_err());
        assert!(account_of_handle("testkit", None).is_err());
        assert!(account_of_handle("testkit", Some(&[1; 39])).is_err());
    }
}
