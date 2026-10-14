//! The identity program's client, as every flow onto an account uses it:
//! read an account, ask which account a key holds, build the admission a
//! consent signs, submit an op. [`join`](crate::join) builds on it.

use identity::{Admission, Op, Query, Reply};

use crate::wire::{Frame, Layer, salt};
use crate::{Client, Error, Signer};

/// How long a consent stays good, in the node's milliseconds.
pub const CONSENT_TTL_MS: u64 = 15 * 60 * 1000;

/// `query` asked of identity, in a query frame `signer` signs.
pub async fn ask(
    client: &Client,
    network: &str,
    query: Query,
    signer: &dyn Signer,
) -> Result<Reply, String> {
    let frame = Frame::query(
        signer,
        network.as_bytes(),
        identity::PROGRAM,
        abi::encode(&query),
    )
    .map_err(|refused| refused.to_string())?;
    let reply = client
        .query(Layer::Preconfirmed, frame.encode())
        .await
        .map_err(|error| node_error(error.to_string()))?;
    abi::decode(&reply).map_err(|refusal| refusal.sentence)
}

/// The account `number` on `network`.
pub async fn account_by_number(
    client: &Client,
    network: &str,
    number: u64,
    signer: &dyn Signer,
) -> Result<identity::Account, String> {
    match ask(client, network, Query::Get { number }, signer).await? {
        Reply::Account(Some(account)) => Ok(account),
        _ => Err(format!("This names an account {network} does not have.")),
    }
}

/// The account `key` belongs to on `network` — its number and name — or
/// `None` while the key acts as no account there: it holds none, or identity
/// refuses it (a suspended account). Only a node that did not answer is an
/// `Err`.
pub async fn account_of_key(
    client: &Client,
    network: &str,
    key: Vec<u8>,
    signer: &dyn Signer,
) -> Result<Option<(u64, String)>, String> {
    let frame = Frame::query(
        signer,
        network.as_bytes(),
        identity::PROGRAM,
        abi::encode(&Query::OfKey { key }),
    )
    .map_err(|refused| refused.to_string())?;
    let reply = match client.query(Layer::Preconfirmed, frame.encode()).await {
        Err(Error::Refused(_)) => return Ok(None),
        reply => reply.map_err(|error| node_error(error.to_string()))?,
    };
    let number = match abi::decode(&reply).map_err(|refusal| refusal.sentence)? {
        Reply::Number(Some(number)) => number,
        _ => return Ok(None),
    };
    match ask(client, network, Query::Get { number }, signer).await? {
        Reply::Account(Some(account)) => Ok(Some((number, account.card.name))),
        _ => Ok(None),
    }
}

pub async fn generation(
    client: &Client,
    network: &str,
    key: &[u8],
    signer: &dyn Signer,
) -> Result<u64, String> {
    let query = Query::Generation { key: key.to_vec() };
    match ask(client, network, query, signer).await? {
        Reply::Generation(generation) => Ok(generation),
        _ => Err("identity answered something other than a generation".into()),
    }
}

/// An admission of `key` (of `scheme`) onto `account`, at the generation
/// identity holds for that key, good for [`CONSENT_TTL_MS`]: the preimage a
/// key already on the account signs to consent.
pub async fn admission(
    client: &Client,
    network: &str,
    scheme: abi::Scheme,
    key: Vec<u8>,
    account: u64,
    signer: &dyn Signer,
) -> Result<Admission, String> {
    Ok(Admission {
        network: network.as_bytes().to_vec(),
        scheme,
        generation: generation(client, network, &key, signer).await?,
        key,
        account,
        expires_at: expires_at(),
    })
}

/// A consent's deadline, against block time (unix ms). The node's status
/// names only its genesis time, so this reads this device's clock; the TTL
/// absorbs ordinary skew.
pub fn expires_at() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default();
    now + CONSENT_TTL_MS
}

/// `op` in a frame `signer` signs, anchored at the node's tip, submitted.
pub async fn submit_signed(
    client: &Client,
    network: &str,
    op: &Op,
    signer: &dyn Signer,
) -> Result<Vec<u8>, String> {
    let anchor = client
        .status()
        .await
        .map_err(|error| node_error(error.to_string()))?
        .anchor();
    let frame = Frame::sign(
        signer,
        network.as_bytes(),
        anchor,
        Some(salt()),
        identity::PROGRAM,
        abi::encode(op),
    )
    .map_err(|refused| refused.to_string())?;
    submit(client, frame.encode()).await
}

pub async fn submit(client: &Client, frame: Vec<u8>) -> Result<Vec<u8>, String> {
    let receipt = client
        .submit(frame)
        .await
        .map_err(|error| node_error(error.to_string()))?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => Err(rejected(refusal)),
    }
}

pub fn rejected(refusal: abi::Refusal) -> String {
    // identity's only already_exists is "this key already belongs to an account".
    if refusal.reason == abi::reason::ALREADY_EXISTS {
        return "This device's key already belongs to an account. Unlock it instead.".into();
    }
    node_error(refusal.sentence)
}

pub fn node_error(sentence: String) -> String {
    // ponytail: identity refuses an expired consent as a generic `unauthorized`,
    // so the sentence is the only tell until it gets its own token.
    if sentence.contains("expired") {
        return "That took too long and the consent expired. Try again.".into();
    }
    if sentence.contains("timed out") {
        return "The node did not answer in time. Retry in a moment.".into();
    }
    sentence
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FixedSigner, SignError};

    /// A node answering every request with `status` and `body`.
    async fn fake_node(status: &'static str, body: Vec<u8>) -> Client {
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                // the request is read whole first: a socket closed on unread
                // bytes resets the connection under the answer
                let mut reader = tokio::io::BufReader::new(&mut stream);
                let (mut line, mut length) = (String::new(), 0);
                while reader.read_line(&mut line).await.unwrap_or(0) > 2 {
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let _ = reader.read_exact(&mut vec![0; length]).await;
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        Client::new(url)
    }

    /// A suspended account's key is refused by identity: that is an answer
    /// (no account), where a node that failed is not one.
    #[tokio::test]
    async fn a_refused_key_holds_no_account_and_a_failed_node_is_no_answer() {
        let refused = abi::Refusal::new(abi::reason::UNAUTHORIZED, "account 7 is suspended");
        let client = fake_node("400 Bad Request", abi::encode(&refused)).await;
        let lookup = account_of_key(&client, "testkit", vec![1], &FixedSigner).await;
        assert_eq!(lookup, Ok(None));
        let client = fake_node("503 Service Unavailable", b"down".to_vec()).await;
        let lookup = account_of_key(&client, "testkit", vec![1], &FixedSigner).await;
        assert!(lookup.is_err());
    }

    /// The one builder the five consent flows sign through hands back the
    /// literal each of them used to write out: the network's bytes, the key
    /// at the generation identity answers, the account, a TTL from now.
    #[tokio::test]
    async fn an_admission_is_the_literal_every_flow_signed() {
        // a query's 200 carries the reply's bytes as one borsh Vec<u8>
        let reply = abi::encode(&abi::encode(&Reply::Generation(3)));
        let client = fake_node("200 OK", reply).await;
        let before = expires_at();
        let built = admission(
            &client,
            "testkit",
            abi::Scheme::Secp256r1,
            vec![7; 32],
            12,
            &FixedSigner,
        )
        .await
        .unwrap();
        assert!((before..=expires_at()).contains(&built.expires_at));
        assert_eq!(
            built,
            Admission {
                network: b"testkit".to_vec(),
                scheme: abi::Scheme::Secp256r1,
                key: vec![7; 32],
                generation: 3,
                account: 12,
                expires_at: built.expires_at,
            }
        );
    }

    /// A signer that refuses (a key store that failed, a seat locked while
    /// the flow waited) is the flow's answer in its own words, not a panic.
    #[tokio::test]
    async fn a_refused_signature_is_the_flows_answer() {
        struct Refusing;
        impl Signer for Refusing {
            fn public_key(&self) -> [u8; 32] {
                [1; 32]
            }
            fn sign(&self, _: &[u8], _: &[u8]) -> Result<[u8; 64], SignError> {
                Err(SignError("the key store refused".into()))
            }
        }
        let status = crate::wire::Status {
            network: "testkit".into(),
            time: 1,
            block_time_ms: 2,
            epoch_length: 3,
            height: 4,
            tip: [5; 32],
            root: abi::Root([6; 32]),
            epoch: 7,
            identity: vec![8],
            contract: crate::wire::NODE_CONTRACT,
            genesis: [9; 32],
            roles: abi::Roles {
                programs: "r".into(),
                validators: "v".into(),
                identity: "i".into(),
            },
        };
        let client = fake_node("200 OK", abi::encode(&status)).await;
        let create = Op::Create {
            name: "a".into(),
            scheme: abi::Scheme::Ed25519,
        };
        let submitted = submit_signed(&client, "testkit", &create, &Refusing).await;
        assert_eq!(submitted, Err("the key store refused".into()));
        let asked = ask(&client, "testkit", Query::Get { number: 1 }, &Refusing).await;
        assert!(matches!(asked, Err(sentence) if sentence == "the key store refused"));
    }

    #[test]
    fn an_identity_refusal_reads_by_its_token() {
        let taken = abi::Refusal::new(abi::reason::ALREADY_EXISTS, "reworded upstream");
        assert!(rejected(taken).starts_with("This device's key already belongs"));
        let expired = abi::Refusal::new("unauthorized", "the consent has expired");
        assert!(rejected(expired).contains("consent expired"));
    }
}
