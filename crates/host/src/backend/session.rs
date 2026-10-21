//! This device, signed in: the key seated for each network the person is
//! signed in to, and the frames it signs. Every write a view submits is
//! signed here — a view never sees a private key or a password.

use noded_client::SignError;
use noded_client::hex::hex_encode;
use noded_client::wire::{Frame, Tip, salt};
use std::collections::BTreeMap;
use std::sync::Arc;
use view_wire::Error;
use view_wire::methods::refusal;

use super::{RpcClient, refused};

struct Signer {
    key: Arc<dyn noded_client::Signer>,
}

static SIGNERS: std::sync::Mutex<BTreeMap<String, Signer>> = std::sync::Mutex::new(BTreeMap::new());

fn seats() -> std::sync::MutexGuard<'static, BTreeMap<String, Signer>> {
    SIGNERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn key_of(network: &str) -> Option<Arc<dyn noded_client::Signer>> {
    seats().get(network).map(|signer| signer.key.clone())
}

const LOCKED: &str = "this device's key is locked; unlock it first";

/// Whether a key is seated: what a write would be signed with.
pub fn seated(network: &str) -> bool {
    seats().contains_key(network)
}

/// The seat is one for the process: a test that seats a key or locks it
/// holds this for its whole run, so no other test's lock or seat lands in
/// the middle of it.
#[cfg(any(test, feature = "test-support"))]
pub fn seat_serial() -> SeatSerial {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SeatSerial(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// One test's hold on the seat ([`seat_serial`]): held across the test's
/// awaits on purpose, which is what it is for.
#[cfg(any(test, feature = "test-support"))]
pub struct SeatSerial(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

fn locked_seat() -> Error {
    Error::new(refusal::SESSION_LOCKED, LOCKED)
}

pub async fn seat_key(network: &str, key: Arc<dyn noded_client::Signer>) -> String {
    let pubkey = hex_encode(&key.public_key());
    seats().insert(network.to_owned(), Signer { key });
    pubkey
}

pub async fn lock_signer() -> bool {
    let mut seats = seats();
    let held = !seats.is_empty();
    seats.clear();
    held
}

pub fn unseat(network: &str) -> bool {
    seats().remove(network).is_some()
}

/// The seated key as the signer of a flow that waits between its
/// signatures (`noded_client::join`): made for the key seated when the flow
/// starts, it looks the seat up again at each signature, so a lock in the
/// meantime refuses the signature ([`LOCKED`]) where a copy would sign.
pub struct Seated {
    network: String,
    public: [u8; 32],
}

impl noded_client::Signer for Seated {
    fn public_key(&self) -> [u8; 32] {
        self.public
    }

    /// Refused, as locked, also when another key was seated since: its
    /// signature would not be this signer's.
    fn sign(&self, namespace: &[u8], message: &[u8]) -> Result<[u8; 64], SignError> {
        match key_of(&self.network) {
            Some(key) if key.public_key() == self.public => key.sign(namespace, message),
            _ => Err(SignError(LOCKED.into())),
        }
    }
}

/// The signer for the key seated now; locked, none.
pub fn seated_signer(network: &str) -> Result<Seated, Error> {
    let key = key_of(network).ok_or_else(locked_seat)?;
    Ok(Seated {
        network: network.to_owned(),
        public: key.public_key(),
    })
}

/// `payload` for `target` in a frame the seated key signs, anchored at the
/// node's tip.
pub async fn seated_frame(
    client: &RpcClient,
    network: &str,
    target: &str,
    payload: Vec<u8>,
) -> Result<Frame, Error> {
    let key = key_of(network).ok_or_else(locked_seat)?;
    let anchor = anchor(client).await?;
    let frame = Frame::sign(
        &*key,
        network.as_bytes(),
        anchor,
        Some(salt()),
        target,
        payload,
    );
    frame.map_err(|refusal| Error::new(refusal::SESSION_LOCKED, refusal.0))
}

pub async fn anchor(client: &RpcClient) -> Result<Tip, Error> {
    Ok(client.status().await.map_err(refused)?.anchor())
}

/// The seated key's public half.
pub async fn seated_key(network: &str) -> Result<Vec<u8>, Error> {
    let key = key_of(network).ok_or_else(locked_seat)?;
    Ok(key.public_key().to_vec())
}

pub async fn seated_sign(key: &[u8], namespace: &[u8], message: &[u8]) -> Result<Vec<u8>, Error> {
    let signer = seats()
        .values()
        .map(|signer| signer.key.clone())
        .find(|signer| signer.public_key().as_slice() == key)
        .ok_or_else(locked_seat)?;
    let proof = signer
        .sign(namespace, message)
        .map_err(|refusal| Error::new(refusal::SESSION_LOCKED, refusal.0))?;
    Ok(proof.to_vec())
}

/// A query `signer` signs: [`query_signer`] for the network in hand, the
/// reader for another (a followed set's, `runtime::notifier::follow`).
pub async fn query_frame(
    signer: &dyn noded_client::Signer,
    network: &str,
    target: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let frame = Frame::query(signer, network.as_bytes(), target, payload);
    Ok(frame
        .map_err(|refusal| Error::new(refusal::SESSION_LOCKED, refusal.0))?
        .encode())
}

pub fn query_signer(network: &str) -> Arc<dyn noded_client::Signer> {
    key_of(network).unwrap_or_else(|| crate::contract::os().reader())
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::{Signer as _, ed25519};

    #[tokio::test]
    async fn a_seated_signer_refuses_once_the_key_is_locked() {
        let _serial = seat_serial();
        let sign = |signer: &Seated| noded_client::Signer::sign(signer, b"ns", b"message");
        seat_key("a", Arc::new(ed25519::PrivateKey::from_seed(3))).await;
        let seated = seated_signer("a").unwrap();
        assert!(sign(&seated).is_ok());
        lock_signer().await;
        assert_eq!(sign(&seated), Err(SignError(LOCKED.into())));
        seat_key("a", Arc::new(ed25519::PrivateKey::from_seed(4))).await;
        assert_eq!(sign(&seated), Err(SignError(LOCKED.into())));
        lock_signer().await;
        assert!(seated_signer("a").is_err());
    }

    #[tokio::test]
    async fn each_network_signs_with_its_own_key_and_one_lock_locks_every_one() {
        let _serial = seat_serial();
        let public =
            |seed| noded_client::Signer::public_key(&ed25519::PrivateKey::from_seed(seed)).to_vec();
        let reader = noded_client::Signer::public_key(&*crate::contract::os().reader()).to_vec();
        seat_key("a", Arc::new(ed25519::PrivateKey::from_seed(5))).await;
        seat_key("b", Arc::new(ed25519::PrivateKey::from_seed(6))).await;
        assert_eq!(seated_key("a").await.unwrap(), public(5));
        assert_eq!(seated_key("b").await.unwrap(), public(6));
        assert!(seated_key("c").await.is_err(), "no key was seated for c");
        let queries = |network| noded_client::Signer::public_key(&*query_signer(network)).to_vec();
        assert_eq!((queries("b"), queries("c")), (public(6), reader));
        assert!(seated_sign(&public(5), b"ns", b"message").await.is_ok());
        assert!(unseat("a"));
        assert!(!seated("a") && seated("b"), "leaving a keeps b's key");
        assert!(seated_sign(&public(5), b"ns", b"message").await.is_err());
        seat_key("a", Arc::new(ed25519::PrivateKey::from_seed(5))).await;
        assert!(lock_signer().await);
        assert!(!seated("a") && !seated("b"), "one Lock locks every network");
    }
}
