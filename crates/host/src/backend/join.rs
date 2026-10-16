//! Getting this device's key onto an account from another device. Identity's
//! `AddKey` is signed by the key joining and consented to by a key already
//! on the account, of any scheme; so any of the account's keys can let a
//! new one in: `noded_client::join`, signed by the seated key, through the
//! auth page's relay. The new device shows a short code, the person types
//! it on a device already on the account, compares a fingerprint, and
//! approves. (The desktop's other way in, a recovery key, is the desktop
//! shell's `backend/recovery.rs`.)

use noded_client::join::{self, Request};

use super::{RpcClient, auth_page, seated_signer};

/// How long the relay holds a request, and so how long a code is waited
/// on: five minutes (the auth page says so).
pub const JOIN_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// Puts this device's (seated) key up under `code`, waits for a device on
/// the account to consent, and joins with that consent; a code nobody
/// approved in [`JOIN_WAIT`] has run out.
pub async fn join_from_device(client: &RpcClient, network: &str, code: &str) -> Result<(), String> {
    let key = seated_signer().map_err(|refusal| refusal.message)?;
    let label = crate::contract::os().facts().device_label;
    let relay = auth_page();
    let joining = join::join_from_device(client, &relay, network, code, &key, label);
    tokio::time::timeout(JOIN_WAIT, joining)
        .await
        .unwrap_or_else(|_| Err("This code ran out. Make a new one.".into()))
}

/// The request a new device put up under `code`, on the auth page's relay.
pub async fn find_request(client: &RpcClient, code: &str) -> Result<Request, String> {
    join::find_request(client, &auth_page(), code).await
}

/// This device (seated, on `account`) consents to `request`'s key joining.
pub async fn approve(
    client: &RpcClient,
    network: &str,
    account: u64,
    code: &str,
    request: &Request,
) -> Result<(), String> {
    let key = seated_signer().map_err(|refusal| refusal.message)?;
    join::approve(client, &auth_page(), network, account, code, request, &key).await
}
