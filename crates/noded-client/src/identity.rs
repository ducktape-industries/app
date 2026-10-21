//! The identity program's client, as every flow onto an account uses it:
//! read an account, ask which account a key holds, build the admission a
//! consent signs, submit an op. [`join`](crate::join) builds on it.

use identity::{Admission, Op, Query, Reply};

use crate::wire::{Frame, Layer, salt};
use crate::{Asked, Client, Error, Signer};

/// How long a consent stays good, in the node's milliseconds.
pub const CONSENT_TTL_MS: u64 = 15 * 60 * 1000;

/// `query` asked of identity, in a query frame `signer` signs.
pub async fn ask(
    client: &Client,
    network: &str,
    query: Query,
    signer: &dyn Signer,
) -> Result<Reply, String> {
    let reply = client
        .query(
            Layer::Preconfirmed,
            signer,
            network.as_bytes(),
            identity::PROGRAM,
            abi::encode(&query),
        )
        .await
        .map_err(|asked| match asked {
            Asked::Unsigned(refused) => refused.to_string(),
            Asked::Node(error) => node_error(error.to_string()),
        })?;
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
/// refuses it as `unauthorized` (a suspended account, the one way `OfKey`
/// refuses a key). Any other refusal (a body cut short, a trap) says nothing
/// of the key: it is an `Err`, as a node that did not answer is.
pub async fn account_of_key(
    client: &Client,
    network: &str,
    key: Vec<u8>,
    signer: &dyn Signer,
) -> Result<Option<(u64, String)>, String> {
    let asked = client
        .query(
            Layer::Preconfirmed,
            signer,
            network.as_bytes(),
            identity::PROGRAM,
            abi::encode(&Query::OfKey { key }),
        )
        .await;
    let reply = match asked {
        Err(Asked::Node(Error::Refused(refusal)))
            if refusal.reason == abi::reason::UNAUTHORIZED =>
        {
            return Ok(None);
        }
        Err(Asked::Unsigned(refused)) => return Err(refused.to_string()),
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
        .preconfirm(frame)
        .await
        .map_err(|error| node_error(error.to_string()))?;
    match receipt.outcome {
        abi::Outcome::Applied { output } => Ok(output),
        abi::Outcome::Rejected(refusal) => Err(rejected(refusal)),
    }
}

pub fn rejected(refusal: abi::Refusal) -> String {
    if refusal.reason == abi::reason::ALREADY_EXISTS {
        return "This device's key already belongs to an account. Unlock it instead.".into();
    }
    node_error(refusal.sentence)
}

pub fn node_error(sentence: String) -> String {
    if sentence.contains("expired") {
        return "That took too long and the consent expired. Try again.".into();
    }
    if sentence.contains("timed out") {
        return "The node did not answer in time. Retry in a moment.".into();
    }
    sentence
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use crate::{FixedSigner, SignError};

    async fn node_answering_with_status_and_content_type(
        status: &'static str,
        kind: &'static str,
        body: Vec<u8>,
    ) -> Client {
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
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
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(&body).await;
            }
        });
        let client = Client::new(url);
        client.heard(crate::wire::Tip {
            height: 1,
            id: [1; 32],
        });
        client
    }

    const BYTES: &str = "application/octet-stream";
    const PLAIN: &str = "text/plain; charset=utf-8";

    #[tokio::test]
    async fn only_a_node_s_own_sentence_reaches_a_failure() {
        let client = node_answering_with_status_and_content_type(
            "503 Service Unavailable",
            PLAIN,
            b"the node is catching up".to_vec(),
        )
        .await;
        let error = client.status().await.unwrap_err().to_string();
        assert_eq!(error, "node answered 503: the node is catching up");
        let page = b"<html><head><title>301 Moved Permanently</title></head></html>".to_vec();
        let client =
            node_answering_with_status_and_content_type("301 Moved Permanently", "text/html", page)
                .await;
        let error = client.status().await.unwrap_err().to_string();
        assert_eq!(error, format!("node answered 301: {}", crate::NOT_A_NODE));
    }

    #[tokio::test]
    async fn a_refused_key_holds_no_account_and_a_failed_node_is_no_answer() {
        let refused = abi::Refusal::new(abi::reason::UNAUTHORIZED, "account 7 is suspended");
        let client = node_answering_with_status_and_content_type(
            "400 Bad Request",
            BYTES,
            abi::encode(&refused),
        )
        .await;
        let lookup = account_of_key(&client, "testkit", vec![1], &FixedSigner).await;
        assert_eq!(lookup, Ok(None));
        let trapped = abi::Refusal::new(abi::reason::TRAP, "out of fuel");
        let client = node_answering_with_status_and_content_type(
            "400 Bad Request",
            BYTES,
            abi::encode(&trapped),
        )
        .await;
        let lookup = account_of_key(&client, "testkit", vec![1], &FixedSigner).await;
        assert_eq!(lookup, Err("trap: out of fuel".into()));
        let client = node_answering_with_status_and_content_type(
            "503 Service Unavailable",
            PLAIN,
            b"down".to_vec(),
        )
        .await;
        let lookup = account_of_key(&client, "testkit", vec![1], &FixedSigner).await;
        assert!(lookup.is_err());
    }

    #[tokio::test]
    async fn an_admission_is_the_literal_every_flow_signed() {
        let reply = abi::encode(&abi::encode(&Reply::Generation(3)));
        let client = node_answering_with_status_and_content_type("200 OK", BYTES, reply).await;
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
            roles: abi::Roles::from_iter([
                (abi::role::PROGRAMS, "r"),
                (abi::role::VALIDATORS, "v"),
                (abi::role::IDENTITY, "i"),
                (abi::role::BLOBS, "b"),
                (abi::role::AWAITS, "a"),
            ]),
        };
        let client =
            node_answering_with_status_and_content_type("200 OK", BYTES, abi::encode(&status))
                .await;
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
