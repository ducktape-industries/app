//! The identity program as this app's own flows onto an account use it, on
//! top of `noded_client::identity`: mint a plain account, submit an op
//! signed by the seated key.
//! `passkey` and `join` build on it; the rail's foot and the sign-in screen
//! call it directly.

use identity::Op;
use noded_client::identity::submit;

use super::{RpcClient, query_signer, seated_frame, seated_key};

/// A self-serve account on `network` whose one key is this device's
/// (seated) key: identity's `Create`, no passkey. Its number and name, as
/// the rail shows them. A key that already holds an account (a retry after
/// a lost answer) is not an error here: that account is the answer.
pub async fn create_plain_account(
    client: &RpcClient,
    network: &str,
    name: &str,
) -> Result<(u64, String), String> {
    let device = seated_key().await.map_err(|refusal| refusal.message)?;
    if let Some(account) = account_of_key(client, network, device).await? {
        return Ok(account);
    }
    let number = create_seated(client, network, name).await?;
    Ok((number, name.trim().to_owned()))
}

pub async fn create_seated(client: &RpcClient, network: &str, name: &str) -> Result<u64, String> {
    let create = Op::Create {
        name: name.to_owned(),
        scheme: abi::Scheme::Ed25519,
    };
    let output = submit_seated(client, network, &create).await?;
    abi::decode::<u64>(&output).map_err(|refusal| refusal.sentence)
}

/// `noded_client::identity::account_of_key`, asked in a frame the seated
/// key (or, locked, the reader key) signs. The rail's foot reads it.
pub async fn account_of_key(
    client: &RpcClient,
    network: &str,
    key: Vec<u8>,
) -> Result<Option<(u64, String)>, String> {
    noded_client::identity::account_of_key(client, network, key, &*query_signer()).await
}

pub async fn submit_seated(client: &RpcClient, network: &str, op: &Op) -> Result<Vec<u8>, String> {
    let frame = seated_frame(client, network, identity::PROGRAM, abi::encode(op))
        .await
        .map_err(|refusal| refusal.message)?;
    submit(client, frame.encode()).await
}
