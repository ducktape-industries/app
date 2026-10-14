//! This device, signed in: the one private key held in memory while the
//! person is signed in ([`device_key`](super::device_key) opens it, or the
//! keystore for a password-locked one) and the frames it signs. Every write
//! a view submits is signed here — a view never sees the private key or a
//! password.

use commonware_cryptography::{Signer as _, ed25519};
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

/// A copy of the seated key, for a flow that signs more than once across
/// awaits (`noded_client::join`); the seat itself is never held across one.
pub(crate) fn seated_signer() -> Result<ed25519::PrivateKey, Error> {
    seat()
        .as_ref()
        .map(|signer| signer.key.clone())
        .ok_or_else(locked_seat)
}

pub(crate) async fn seated_frame(
    client: &RpcClient,
    network: &str,
    target: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let key = seated_signer()?;
    let anchor = anchor(client).await?;
    Ok(Frame::sign(
        &key,
        network.as_bytes(),
        anchor,
        Some(salt()),
        target,
        payload,
    )
    .encode())
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
    Frame::query(key, network.as_bytes(), target, payload).encode()
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
