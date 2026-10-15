//! This device, signed in: the one signer held while the person is signed
//! in (the shell opens it: the desktop's device key, or its keystore for a
//! password-locked one) and the frames it signs. Every write a view submits
//! is signed here — a view never sees a private key or a password.

use noded_client::SignError;
use noded_client::hex::hex_encode;
use noded_client::wire::{Frame, Tip, salt};
use std::sync::Arc;
use view_wire::Error;
use view_wire::methods::refusal;

use super::{RpcClient, refused};

struct Signer {
    key: Arc<dyn noded_client::Signer>,
}

/// The seated key: the one that signs. "Seat" here is the backend's word
/// for loading this key and "lock" for dropping it — not the runtime's
/// seat, the slot a program's view is mounted in. Never held across an
/// await: a write takes its copy of the key and lets go before it asks the
/// node anything, so a seat or a lock never waits on a write's ask (and a
/// task on the shell's executor is never woken from a runtime thread for
/// it).
static SIGNER: std::sync::Mutex<Option<Signer>> = std::sync::Mutex::new(None);

fn seat() -> std::sync::MutexGuard<'static, Option<Signer>> {
    SIGNER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const LOCKED: &str = "this device's key is locked; unlock it first";

/// Whether a key is seated: what a write would be signed with.
pub fn seated() -> bool {
    seat().is_some()
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

/// Seats `key` as the one that signs; answers its public key.
pub async fn seat_key(key: Arc<dyn noded_client::Signer>) -> String {
    let pubkey = hex_encode(&key.public_key());
    *seat() = Some(Signer { key });
    pubkey
}

pub async fn lock_signer() -> bool {
    seat().take().is_some()
}

/// The seated key as the signer of a flow that waits between its
/// signatures (`noded_client::join`): made for the key seated when the flow
/// starts, it looks the seat up again at each signature, so a lock in the
/// meantime refuses the signature ([`LOCKED`]) where a copy would sign.
pub struct Seated {
    public: [u8; 32],
}

impl noded_client::Signer for Seated {
    fn public_key(&self) -> [u8; 32] {
        self.public
    }

    /// Refused, as locked, also when another key was seated since: its
    /// signature would not be this signer's.
    fn sign(&self, namespace: &[u8], message: &[u8]) -> Result<[u8; 64], SignError> {
        let session = seat();
        match session.as_ref() {
            Some(signer) if signer.key.public_key() == self.public => {
                signer.key.sign(namespace, message)
            }
            _ => Err(SignError(LOCKED.into())),
        }
    }
}

/// The signer for the key seated now; locked, none.
pub fn seated_signer() -> Result<Seated, Error> {
    let session = seat();
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok(Seated {
        public: signer.key.public_key(),
    })
}

pub async fn seated_frame(
    client: &RpcClient,
    network: &str,
    target: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let key = seat()
        .as_ref()
        .map(|signer| signer.key.clone())
        .ok_or_else(locked_seat)?;
    let anchor = anchor(client).await?;
    let frame = Frame::sign(
        &*key,
        network.as_bytes(),
        anchor,
        Some(salt()),
        target,
        payload,
    );
    Ok(frame
        .map_err(|refusal| Error::new(refusal::SESSION_LOCKED, refusal.0))?
        .encode())
}

pub async fn anchor(client: &RpcClient) -> Result<Tip, Error> {
    Ok(client.status().await.map_err(refused)?.anchor())
}

/// The seated key's public half.
pub async fn seated_key() -> Result<Vec<u8>, Error> {
    let session = seat();
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok(signer.key.public_key().to_vec())
}

/// The seated key's public half and its signature over `message` under
/// `namespace` — a consent the device key gives (`backend::passkey`).
pub async fn seated_sign(namespace: &[u8], message: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let session = seat();
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    let proof = signer
        .key
        .sign(namespace, message)
        .map_err(|refusal| Error::new(refusal::SESSION_LOCKED, refusal.0))?;
    Ok((signer.key.public_key().to_vec(), proof.to_vec()))
}

/// A query signed by the seated key, or by the shell's reader while none
/// is seated.
pub async fn query_frame(network: &str, target: &str, payload: Vec<u8>) -> Result<Vec<u8>, Error> {
    let frame = Frame::query(&*query_signer(), network.as_bytes(), target, payload);
    Ok(frame
        .map_err(|refusal| Error::new(refusal::SESSION_LOCKED, refusal.0))?
        .encode())
}

/// What signs a query: the seated key, or the shell's reader
/// (`OsServices::reader`) while none is seated — the key [`query_frame`]
/// signs with.
pub fn query_signer() -> Arc<dyn noded_client::Signer> {
    seat().as_ref().map_or_else(
        || crate::contract::os().reader(),
        |signer| signer.key.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::{Signer as _, ed25519};

    /// A join's signer, made while the key is seated, refuses once the
    /// person locks (or another key is seated): the join that waited fails
    /// as locked instead of signing with a copy.
    #[tokio::test]
    async fn a_seated_signer_refuses_once_the_key_is_locked() {
        let _serial = seat_serial();
        let sign = |signer: &Seated| noded_client::Signer::sign(signer, b"ns", b"message");
        seat_key(Arc::new(ed25519::PrivateKey::from_seed(3))).await;
        let seated = seated_signer().unwrap();
        assert!(sign(&seated).is_ok());
        lock_signer().await;
        assert_eq!(sign(&seated), Err(SignError(LOCKED.into())));
        seat_key(Arc::new(ed25519::PrivateKey::from_seed(4))).await;
        assert_eq!(sign(&seated), Err(SignError(LOCKED.into())));
        lock_signer().await;
        assert!(seated_signer().is_err());
    }
}
