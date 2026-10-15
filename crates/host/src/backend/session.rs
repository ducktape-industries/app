//! This device, signed in: the one private key held in memory while the
//! person is signed in ([`device_key`](super::device_key) opens it, or the
//! keystore for a password-locked one) and the frames it signs. Every write
//! a view submits is signed here — a view never sees the private key or a
//! password.

use commonware_cryptography::{Signer as _, ed25519};
use noded_client::SignError;
use noded_client::hex::hex_encode;
use noded_client::wire::{Frame, Tip, salt};
use view_wire::Error;
use view_wire::methods::refusal;

use super::{RpcClient, refused};

struct Signer {
    key: ed25519::PrivateKey,
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
pub(crate) fn seated() -> bool {
    seat().is_some()
}

/// The seat is one for the process: a test that seats a key or locks it
/// holds this for its whole run, so no other test's lock or seat lands in
/// the middle of it.
#[cfg(test)]
pub(crate) fn seat_serial() -> SeatSerial {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SeatSerial(
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// One test's hold on the seat ([`seat_serial`]): held across the test's
/// awaits on purpose, which is what it is for.
#[cfg(test)]
pub(crate) struct SeatSerial(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

fn locked_seat() -> Error {
    Error::new(refusal::SESSION_LOCKED, LOCKED)
}

/// Seats `key` as the one that signs; answers its public key.
pub(crate) async fn seat_key(key: ed25519::PrivateKey) -> String {
    let pubkey = hex_encode(key.public_key().as_ref());
    *seat() = Some(Signer { key });
    pubkey
}

pub(crate) async fn lock_signer() -> bool {
    seat().take().is_some()
}

/// The seated key as the signer of a flow that waits between its
/// signatures (`noded_client::join`): made for the key seated when the flow
/// starts, it looks the seat up again at each signature, so a lock in the
/// meantime refuses the signature ([`LOCKED`]) where a copy would sign.
pub(crate) struct Seated {
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
            Some(signer) if noded_client::Signer::public_key(&signer.key) == self.public => {
                noded_client::Signer::sign(&signer.key, namespace, message)
            }
            _ => Err(SignError(LOCKED.into())),
        }
    }
}

/// The signer for the key seated now; locked, none.
pub(crate) fn seated_signer() -> Result<Seated, Error> {
    let session = seat();
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok(Seated {
        public: noded_client::Signer::public_key(&signer.key),
    })
}

pub(crate) async fn seated_frame(
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
        &key,
        network.as_bytes(),
        anchor,
        Some(salt()),
        target,
        payload,
    );
    Ok(frame.expect("a key in memory signs").encode())
}

pub(crate) async fn anchor(client: &RpcClient) -> Result<Tip, Error> {
    Ok(client.status().await.map_err(refused)?.anchor())
}

/// The seated key's public half.
pub(crate) async fn seated_key() -> Result<Vec<u8>, Error> {
    let session = seat();
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok(signer.key.public_key().as_ref().to_vec())
}

/// The seated key's public half and its signature over `message` under
/// `namespace` — a consent the device key gives (`backend::passkey`).
pub(crate) async fn seated_sign(
    namespace: &[u8],
    message: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let session = seat();
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok((
        signer.key.public_key().as_ref().to_vec(),
        signer.key.sign(namespace, message).as_ref().to_vec(),
    ))
}

pub(crate) async fn query_frame(network: &str, target: &str, payload: Vec<u8>) -> Vec<u8> {
    let session = seat();
    let key = match session.as_ref() {
        Some(signer) => &signer.key,
        None => reader_key(),
    };
    let frame = Frame::query(key, network.as_bytes(), target, payload);
    frame.expect("a key in memory signs").encode()
}

/// What signs a query: a copy of the seated key, or the reader key while
/// none is seated — the key [`query_frame`] signs with.
pub(crate) fn query_signer() -> ed25519::PrivateKey {
    seat()
        .as_ref()
        .map_or_else(|| reader_key().clone(), |signer| signer.key.clone())
}

fn reader_key() -> &'static ed25519::PrivateKey {
    static KEY: std::sync::OnceLock<ed25519::PrivateKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        use commonware_codec::DecodeExt as _;
        use rand::RngCore as _;
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        ed25519::PrivateKey::decode(seed.as_slice()).expect("32 random bytes decode")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A join's signer, made while the key is seated, refuses once the
    /// person locks (or another key is seated): the join that waited fails
    /// as locked instead of signing with a copy.
    #[tokio::test]
    async fn a_seated_signer_refuses_once_the_key_is_locked() {
        let _serial = seat_serial();
        let sign = |signer: &Seated| noded_client::Signer::sign(signer, b"ns", b"message");
        seat_key(ed25519::PrivateKey::from_seed(3)).await;
        let seated = seated_signer().unwrap();
        assert!(sign(&seated).is_ok());
        lock_signer().await;
        assert_eq!(sign(&seated), Err(SignError(LOCKED.into())));
        seat_key(ed25519::PrivateKey::from_seed(4)).await;
        assert_eq!(sign(&seated), Err(SignError(LOCKED.into())));
        lock_signer().await;
        assert!(seated_signer().is_err());
    }
}
