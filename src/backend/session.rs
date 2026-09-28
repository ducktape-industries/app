//! This device, signed in: the one private key held in memory while the
//! person is signed in ([`device_key`](super::device_key) opens it, or the
//! keystore for a password-locked one) and the frames it signs. Every write
//! a view submits is signed here — a view never sees the private key or a
//! password.

use commonware_cryptography::{Signer as _, ed25519};
use view_wire::Error;

use super::noded::{Frame, Layer};
use super::{RpcClient, hex_encode, refused};

/// The host namespace that holds each signer's next sequence.
const SIGNERS: &str = "$signers";

struct Signer {
    key: ed25519::PrivateKey,
}

/// The seated key: the one that signs. "Seat" here is the backend's word
/// for loading this key and "lock" for dropping it — not the runtime's
/// seat, the slot a program's view is mounted in.
static SIGNER: tokio::sync::Mutex<Option<Signer>> = tokio::sync::Mutex::const_new(None);

const LOCKED: &str = "this device's key is locked; unlock it first";

fn locked_seat() -> Error {
    Error::new("session_locked", LOCKED)
}

/// Seats `key` as the one that signs; answers its public key.
pub(crate) async fn seat_key(key: ed25519::PrivateKey) -> String {
    let pubkey = hex_encode(key.public_key().as_ref());
    *SIGNER.lock().await = Some(Signer { key });
    pubkey
}

pub(crate) async fn lock_signer() -> bool {
    SIGNER.lock().await.take().is_some()
}

/// A write: signed with the seated key at the sequence the node says is
/// that signer's next.
pub(crate) async fn seated_frame(
    client: &RpcClient,
    network: &str,
    target: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let session = SIGNER.lock().await;
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    let seq = next_seq(client, signer.key.public_key().as_ref()).await?;
    Ok(Frame::sign(&signer.key, network.as_bytes(), seq, target, payload).encode())
}

/// The sequence the node expects next from `signer`.
pub(crate) async fn next_seq(client: &RpcClient, signer: &[u8]) -> Result<u64, Error> {
    Ok(client
        .get(Layer::Preconfirmed, SIGNERS, signer)
        .await
        .map_err(refused)?
        .map(|bytes| abi::decode::<u64>(&bytes))
        .transpose()
        .map_err(|refusal| Error::new(view_wire::code::UNEXPECTED_REPLY, refusal.sentence))?
        .unwrap_or(0))
}

/// The seated key's public half.
pub(crate) async fn seated_key() -> Result<Vec<u8>, Error> {
    let session = SIGNER.lock().await;
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok(signer.key.public_key().as_ref().to_vec())
}

/// The seated key's public half and its signature over `message` under
/// `namespace` — a consent the device key gives (`backend::passkey`).
pub(crate) async fn seated_sign(
    namespace: &[u8],
    message: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let session = SIGNER.lock().await;
    let signer = session.as_ref().ok_or_else(locked_seat)?;
    Ok((
        signer.key.public_key().as_ref().to_vec(),
        signer.key.sign(namespace, message).as_ref().to_vec(),
    ))
}

/// A read: a query still travels as a signed frame (the program hears who
/// asks), but the node checks no sequence on it. Signed with the seated
/// key, or with this process's reader key while nobody is signed in.
pub(crate) async fn query_frame(network: &str, target: &str, payload: Vec<u8>) -> Vec<u8> {
    let session = SIGNER.lock().await;
    let key = match session.as_ref() {
        Some(signer) => &signer.key,
        None => reader_key(),
    };
    Frame::sign(key, network.as_bytes(), 0, target, payload).encode()
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

    #[tokio::test]
    async fn a_query_frame_names_the_target_and_the_network() {
        let frame = query_frame("net", "demo", vec![1]).await;
        let decoded: Frame = abi::decode(&frame).unwrap();
        assert_eq!(decoded.body.target, "demo");
        assert_eq!(decoded.body.network, b"net");
        assert_eq!(decoded.body.seq, 0);
    }
}
