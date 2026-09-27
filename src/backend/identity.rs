//! The identity program's client, as every flow onto an account uses it:
//! read an account, ask which account a key holds, mint a plain account,
//! submit an op signed by the seated key. `passkey` and `join` build on
//! it; the rail's foot and the sign-in screen call it directly.

use identity::{Control, Op, Query, Reply};

use super::noded::Layer;
use super::{RpcClient, query_frame, seated_frame, seated_key};

/// How long a consent stays good, in the node's milliseconds.
const CONSENT_TTL_MS: u64 = 15 * 60 * 1000;

/// A self-serve account on `network` whose one key is this device's
/// (seated) key: identity's `Create`, no passkey. Its number and name, as
/// the rail shows them. A key that already holds an account (a retry after
/// a lost answer) is not an error here: that account is the answer.
pub(crate) async fn create_plain_account(
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

pub(super) async fn create_seated(
    client: &RpcClient,
    network: &str,
    name: &str,
) -> Result<u64, String> {
    let create = Op::Create {
        name: name.to_owned(),
        scheme: abi::Scheme::Ed25519,
    };
    let output = submit_seated(client, network, &create).await?;
    abi::decode::<u64>(&output).map_err(|refusal| refusal.sentence)
}

/// The account `number` on `network`.
pub(super) async fn account_by_number(
    client: &RpcClient,
    network: &str,
    number: u64,
) -> Result<identity::Account, String> {
    match ask(client, network, Query::Get { number }).await? {
        Reply::Account(Some(account)) => Ok(account),
        _ => Err(format!("This names an account {network} does not have.")),
    }
}

/// A passkey or recovery-key flow is a person's: their own keys join their
/// own account. An agent's keys are its manager's to add, and a module's
/// account holds none, so a key that holds either is told so instead of
/// sending an `AddKey` identity would refuse.
pub(super) fn person(account: &identity::Account) -> Result<(), String> {
    let name = &account.card.name;
    match &account.control {
        Control::Person { .. } => Ok(()),
        Control::Managed { manager, .. } => Err(format!(
            "This key belongs to {name} (account {}), an agent managed by account {manager}. \
             Passkeys and recovery keys are for a person's own account: sign in with a \
             person's key.",
            account.number
        )),
        Control::Module { module } => Err(format!(
            "This key belongs to the account of the module {module}, which takes no passkey or \
             recovery key."
        )),
    }
}

pub(super) async fn ask(client: &RpcClient, network: &str, query: Query) -> Result<Reply, String> {
    let frame = query_frame(network, identity::MODULE, abi::encode(&query)).await;
    let reply = client
        .query(Layer::Preconfirmed, frame)
        .await
        .map_err(|error| node_error(error.to_string()))?;
    abi::decode(&reply).map_err(|refusal| refusal.sentence)
}

/// The account `key` belongs to on `network` — its number and name — or
/// `None` while the key acts as no account there: it holds none, or identity
/// refuses it (a suspended account). Only a node that did not answer is an
/// `Err`. The rail's foot reads it.
pub(crate) async fn account_of_key(
    client: &RpcClient,
    network: &str,
    key: Vec<u8>,
) -> Result<Option<(u64, String)>, String> {
    let frame = query_frame(
        network,
        identity::MODULE,
        abi::encode(&Query::OfKey { key }),
    )
    .await;
    let reply = match client.query(Layer::Preconfirmed, frame).await {
        Err(super::noded::Error::Refused(_)) => return Ok(None),
        reply => reply.map_err(|error| node_error(error.to_string()))?,
    };
    let number = match abi::decode(&reply).map_err(|refusal| refusal.sentence)? {
        Reply::Number(Some(number)) => number,
        _ => return Ok(None),
    };
    match ask(client, network, Query::Get { number }).await? {
        Reply::Account(Some(account)) => Ok(Some((number, account.card.name))),
        _ => Ok(None),
    }
}

pub(super) async fn generation(
    client: &RpcClient,
    network: &str,
    key: &[u8],
) -> Result<u64, String> {
    match ask(client, network, Query::Generation { key: key.to_vec() }).await? {
        Reply::Generation(generation) => Ok(generation),
        _ => Err("identity answered something other than a generation".into()),
    }
}

/// A consent's deadline, against block time (unix ms). The node's status
/// names only its genesis time, so this reads this device's clock; the TTL
/// absorbs ordinary skew.
pub(super) fn expires_at() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default();
    now + CONSENT_TTL_MS
}

pub(super) async fn submit_seated(
    client: &RpcClient,
    network: &str,
    op: &Op,
) -> Result<Vec<u8>, String> {
    let frame = seated_frame(client, network, identity::MODULE, abi::encode(op))
        .await
        .map_err(|refusal| refusal.message)?;
    submit(client, frame).await
}

pub(super) async fn submit(client: &RpcClient, frame: Vec<u8>) -> Result<Vec<u8>, String> {
    let receipt = client
        .submit(frame)
        .await
        .map_err(|error| node_error(error.to_string()))?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => Err(rejected(refusal)),
    }
}

fn rejected(refusal: abi::Refusal) -> String {
    // identity's only already_exists is "this key already belongs to an account".
    if refusal.reason == abi::reason::ALREADY_EXISTS {
        return "This device's key already belongs to an account. Unlock it instead.".into();
    }
    node_error(refusal.sentence)
}

fn node_error(sentence: String) -> String {
    // ponytail: identity refuses an expired consent as a generic `unauthorized`,
    // so the sentence is the only tell until it gets its own token.
    if sentence.contains("expired") {
        return "That took too long and the consent expired. Try again.".into();
    }
    super::user_error(sentence)
}

#[cfg(test)]
mod tests {
    use super::super::loopback::read_request;
    use super::*;
    use tokio::io::AsyncWriteExt as _;
    use tokio::net::TcpListener;

    /// A node answering every request with `status` and `body`.
    async fn fake_node(status: &'static str, body: Vec<u8>) -> RpcClient {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let _ = read_request(&mut stream).await;
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        RpcClient::new(url)
    }

    /// A suspended account's key is refused by identity: that is an answer
    /// (no account), where a node that failed is not one.
    #[tokio::test]
    async fn a_refused_key_holds_no_account_and_a_failed_node_is_no_answer() {
        let refused = abi::Refusal::new(abi::reason::UNAUTHORIZED, "account 7 is suspended");
        let client = fake_node("400 Bad Request", abi::encode(&refused)).await;
        assert_eq!(account_of_key(&client, "testkit", vec![1]).await, Ok(None));
        let client = fake_node("503 Service Unavailable", b"down".to_vec()).await;
        assert!(account_of_key(&client, "testkit", vec![1]).await.is_err());
    }

    #[test]
    fn an_identity_refusal_reads_by_its_token() {
        let taken = abi::Refusal::new(abi::reason::ALREADY_EXISTS, "reworded upstream");
        assert!(rejected(taken).starts_with("This device's key already belongs"));
        let expired = abi::Refusal::new("unauthorized", "the consent has expired");
        assert!(rejected(expired).contains("consent expired"));
    }
}
