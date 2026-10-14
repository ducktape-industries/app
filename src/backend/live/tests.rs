use super::*;
use crate::backend::noded::{KeyScheme as Verifier, NODE_CONTRACT, Status};
use commonware_codec::Encode as _;
use commonware_cryptography::{Signer as _, ed25519};
use live_wire::{Challenge, Welcome};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const ED25519_PKCS8: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
/// The account the test node answers for every key.
const ACCOUNT: u64 = 7;
/// The aliases the test node gives a publication and a subscription.
const PUBLISHED: Alias = 1;
const SUBSCRIBED: Alias = 2;

/// What the test node does with a proof.
#[derive(Clone, Copy)]
enum Answer {
    Welcome,
    Refuse,
    /// No live route at all: the port's route answers 404.
    Unserved,
    /// Welcomes, then answers a subscription as a node of another wire
    /// would: a `Subscribed` without the publisher's account.
    Garble,
}

/// The node's own certificate, as halo makes it: self-signed by its key.
fn certified(key: &ed25519::PrivateKey) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let mut pkcs8 = ED25519_PKCS8.to_vec();
    pkcs8.extend_from_slice(key.encode().as_ref());
    let der = PrivatePkcs8KeyDer::from(pkcs8);
    let pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&der, &rcgen::PKCS_ED25519).unwrap();
    let cert = rcgen::CertificateParams::new(vec!["ducktape-halo".into()])
        .unwrap()
        .self_signed(&pair)
        .unwrap();
    (cert.der().clone(), PrivateKeyDer::Pkcs8(der))
}

/// A node on loopback: `/v1/status` naming `identity` and `/v1/live/port`
/// on HTTP, and on UDP a relay of one track, which it hands back to a
/// subscriber on the same session: what the client publishes under
/// [`PUBLISHED`] comes back under [`SUBSCRIBED`].
async fn node(cert_key: &ed25519::PrivateKey, identity: Vec<u8>, answer: Answer) -> RpcClient {
    node_hearing(cert_key, identity, answer).await.0
}

/// [`node`], and how each of its sessions ended, as the node hears it.
async fn node_hearing(
    cert_key: &ed25519::PrivateKey,
    identity: Vec<u8>,
    answer: Answer,
) -> (RpcClient, mpsc::UnboundedReceiver<quinn::ConnectionError>) {
    let (cert, key) = certified(cert_key);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut crypto = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    crypto.alpn_protocols = vec![live_wire::ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(crypto).unwrap();
    let server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let endpoint = quinn::Endpoint::server(server, "127.0.0.1:0".parse().unwrap()).unwrap();
    let port = endpoint.local_addr().unwrap().port();
    let (ended, ends) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let connection = incoming.await.unwrap();
            let (closing, ended) = (connection.clone(), ended.clone());
            tokio::spawn(async move {
                let _ = ended.send(closing.closed().await);
            });
            tokio::spawn(relay(connection, answer));
        }
    });
    let status = Status {
        network: "loopback".into(),
        time: 1,
        block_time_ms: 500,
        epoch_length: 100,
        height: 1,
        tip: [1; 32],
        root: abi::Root([2; 32]),
        epoch: 0,
        identity,
        contract: NODE_CONTRACT,
        genesis: [9; 32],
        roles: crate::backend::noded::test_roles(),
    };
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", http.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = http.accept().await {
            let status = abi::encode(&status);
            tokio::spawn(async move {
                let mut asked = vec![0; 4096];
                let read = stream.read(&mut asked).await.unwrap();
                let line = String::from_utf8_lossy(&asked[..read]);
                // the status and the port, and nothing else: no websocket here
                let body = if line.starts_with("GET /v1/status ") {
                    Some(status)
                } else if line.starts_with(&format!("GET {} ", live_wire::PORT_ROUTE)) {
                    (!matches!(answer, Answer::Unserved)).then(|| abi::encode(&Some(port)))
                } else {
                    None
                };
                let answer = match body {
                    Some(body) => [
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                            .into_bytes(),
                        body,
                    ]
                    .concat(),
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                };
                stream.write_all(&answer).await.unwrap();
            });
        }
    });
    (RpcClient::new(base), ends)
}

async fn read(recv: &mut quinn::RecvStream) -> Control {
    read_control(recv).await.unwrap()
}

async fn write_control(send: &mut quinn::SendStream, control: Control) {
    let bytes = abi::encode(&control);
    send.write_all(&(bytes.len() as u32).to_le_bytes())
        .await
        .unwrap();
    send.write_all(&bytes).await.unwrap();
}

/// The node's side of one session: a proof that does not verify is
/// refused `bad_proof`, as core's relay refuses it.
async fn relay(connection: quinn::Connection, answer: Answer) {
    let (mut send, mut recv) = connection.accept_bi().await.unwrap();
    let Control::Hello(hello) = read(&mut recv).await else {
        panic!("a session opens with a hello")
    };
    let nonce = [3; 32];
    write_control(&mut send, Control::Challenge(Challenge { nonce })).await;
    let Control::Proof(proof) = read(&mut recv).await else {
        panic!("the challenge is answered with a proof")
    };
    let preimage = live_wire::proof_preimage(&hello.chain, &nonce, &hello.key);
    let signed = Verifier::Ed25519.verify(
        &hello.key,
        live_wire::NAMESPACE,
        &preimage,
        &proof.signature,
    );
    assert_eq!(hello.chain, [9; 32], "the hello names the genesis");
    if !signed || matches!(answer, Answer::Refuse) {
        let code = Code::BadProof;
        write_control(&mut send, Control::Refused { request: 0, code }).await;
        // the refusal is read before the session goes: the client hangs up
        send.finish().unwrap();
        connection.closed().await;
        return;
    }
    let welcome = Welcome {
        account: ACCOUNT,
        datagrams: true,
    };
    write_control(&mut send, Control::Welcome(welcome)).await;
    // groups and datagrams of the publication, back under the subscription
    let streams = connection.clone();
    tokio::spawn(async move {
        while let Ok(mut incoming) = streams.accept_uni().await {
            let streams = streams.clone();
            tokio::spawn(async move {
                let mut head = [0u8; 13];
                incoming.read_exact(&mut head).await.unwrap();
                let header: GroupHeader = abi::decode(&head).unwrap();
                assert_eq!(header.alias, PUBLISHED);
                let mut out = streams.open_uni().await.unwrap();
                let header = GroupHeader {
                    alias: SUBSCRIBED,
                    ..header
                };
                out.write_all(&abi::encode(&header)).await.unwrap();
                // as the bytes come, so a group is in flight while it runs
                let _ = tokio::io::copy(&mut incoming, &mut out).await;
                let _ = out.finish();
            });
        }
    });
    let datagrams = connection.clone();
    tokio::spawn(async move {
        while let Ok(bytes) = datagrams.read_datagram().await {
            let mut header: DatagramHeader = abi::decode(&bytes[..16]).unwrap();
            assert_eq!(header.alias, PUBLISHED);
            header.alias = SUBSCRIBED;
            let mut back = abi::encode(&header);
            back.extend_from_slice(&bytes[16..]);
            datagrams.send_datagram(back.into()).unwrap();
        }
    });
    let garble = matches!(answer, Answer::Garble);
    let mut mode = None;
    let mut track = None;
    // core's rule: one subscriber per track and one watcher per room for a
    // session, a repeat refused `duplicate_track`
    let mut following: HashMap<RequestId, Track> = HashMap::new();
    let mut watching: HashMap<RequestId, String> = HashMap::new();
    loop {
        let answer = match read(&mut recv).await {
            Control::Publish {
                request,
                mode: published,
                track: named,
                ..
            } => {
                mode = Some(published);
                track = Some(named);
                Control::Published {
                    request,
                    alias: PUBLISHED,
                }
            }
            Control::Subscribe { request, .. } if garble => {
                let subscribed = Control::Subscribed {
                    request,
                    alias: SUBSCRIBED,
                    by: ACCOUNT,
                    mode: Mode::Stream,
                    latest: None,
                };
                let mut bytes = abi::encode(&subscribed);
                // the account's 8 bytes short, as a wire without it sends
                bytes.truncate(bytes.len() - 8);
                send.write_all(&(bytes.len() as u32).to_le_bytes())
                    .await
                    .unwrap();
                send.write_all(&bytes).await.unwrap();
                continue;
            }
            Control::Subscribe {
                request,
                track: asked,
                ..
            } if following.values().any(|held| *held == asked) => Control::Refused {
                request,
                code: Code::Duplicate,
            },
            Control::Watch { request, namespace }
                if watching.values().any(|held| *held == namespace) =>
            {
                Control::Refused {
                    request,
                    code: Code::Duplicate,
                }
            }
            Control::Unsubscribe { request } => {
                following.remove(&request);
                continue;
            }
            Control::Unwatch { request } => {
                watching.remove(&request);
                continue;
            }
            Control::Subscribe {
                request,
                track: asked,
                ..
            } => match mode {
                Some(mode) => {
                    following.insert(request, asked);
                    Control::Subscribed {
                        request,
                        alias: SUBSCRIBED,
                        by: ACCOUNT,
                        mode,
                        latest: None,
                    }
                }
                None => Control::Refused {
                    request,
                    code: Code::Unknown,
                },
            },
            // a room's list: the one track up, if any
            Control::Watch { request, namespace } => {
                watching.insert(request, namespace);
                match (&track, mode) {
                    (Some(track), Some(mode)) => Control::Announce {
                        request,
                        track: track.clone(),
                        by: ACCOUNT,
                        mode,
                        up: true,
                    },
                    _ => continue,
                }
            }
            _ => continue,
        };
        write_control(&mut send, answer).await;
    }
}

fn track(name: &str) -> Track {
    Track {
        namespace: "loopback#1/echo/lobby".into(),
        name: name.into(),
    }
}

/// The seat held for the test, a key seated in it.
async fn seated() -> impl Sized {
    let serial = crate::backend::seat_serial();
    crate::backend::seat_key(ed25519::PrivateKey::from_seed(11)).await;
    serial
}

/// The headline: over QUIC to a node whose key the status names, the
/// handshake signs the challenge with the seated key, a publication is
/// taken, and what it publishes comes back to a subscription of the same
/// session, a stream-mode group whole and in order, then its end.
#[tokio::test]
async fn a_track_published_over_quic_is_heard_back_whole() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(1);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Welcome).await);
    let mut published = client
        .open(Ask::Publish {
            track: track("doc"),
            mode: Mode::Stream,
            priority: 128,
        })
        .await
        .unwrap();
    assert_eq!(published.heard.recv().await, Some(Heard::Published));
    let mut heard = client
        .open(Ask::Subscribe {
            track: track("doc"),
            from: Start::Latest,
        })
        .await
        .unwrap();
    assert_eq!(
        heard.heard.recv().await,
        Some(Heard::Subscribed {
            by: ACCOUNT,
            mode: Mode::Stream,
            latest: None
        })
    );
    for (object, (body, end)) in [(b"a".to_vec(), false), (b"b".to_vec(), true)]
        .into_iter()
        .enumerate()
    {
        let object = Object {
            group: 4,
            object: object as u32,
            body,
            end,
        };
        published.objects.send(object).await.unwrap();
    }
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(heard.data.recv().await.unwrap());
    }
    assert_eq!(
        got,
        [
            Data::Object {
                group: 4,
                object: 0,
                body: b"a".to_vec()
            },
            Data::Object {
                group: 4,
                object: 1,
                body: b"b".to_vec()
            },
            Data::GroupEnd {
                group: 4,
                complete: true
            },
        ]
    );
}

/// A datagram-mode track: an object is one datagram, and it lands as the
/// subscription's newest.
#[tokio::test]
async fn a_datagram_object_lands_as_the_newest() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(2);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Welcome).await);
    let mut published = client
        .open(Ask::Publish {
            track: track("cursor/7"),
            mode: Mode::Datagram,
            priority: 128,
        })
        .await
        .unwrap();
    assert_eq!(published.heard.recv().await, Some(Heard::Published));
    let mut heard = client
        .open(Ask::Subscribe {
            track: track("cursor/7"),
            from: Start::Next,
        })
        .await
        .unwrap();
    assert!(matches!(
        heard.heard.recv().await,
        Some(Heard::Subscribed { .. })
    ));
    let object = Object {
        group: 5,
        object: 2,
        body: vec![1, 2, 3],
        end: false,
    };
    published.objects.send(object).await.unwrap();
    assert_eq!(
        heard.latest.take().await,
        Data::Object {
            group: 5,
            object: 2,
            body: vec![1, 2, 3]
        }
    );
}

/// A node that refuses the proof: the request is refused in the node's
/// word, and nothing is open.
#[tokio::test]
async fn a_refused_proof_is_the_nodes_word() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(3);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Refuse).await);
    let refused = client
        .open(Ask::Watch {
            namespace: "loopback#1/echo/lobby".into(),
        })
        .await
        .err()
        .unwrap();
    assert_eq!(refused.code, Code::BadProof.as_str());
    assert!(
        lock(&client.0.not_before).is_some_and(|until| until > Instant::now()),
        "a refusal is redialed at once"
    );
}

/// A certificate that holds another key than the status names is not the
/// node: QUIC is refused, the websocket is tried, and this node has none.
#[tokio::test]
async fn a_certificate_of_another_key_is_not_the_node() {
    let _seat = seated().await;
    let (node_key, other) = (
        ed25519::PrivateKey::from_seed(4),
        ed25519::PrivateKey::from_seed(5),
    );
    let identity = other.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Welcome).await);
    let refused = client
        .open(Ask::Watch {
            namespace: "loopback#1/echo/lobby".into(),
        })
        .await
        .err()
        .unwrap();
    assert_eq!(refused.code, refusal::LIVE_UNAVAILABLE);
}

/// A node that answers the port's route 404 serves no live tracks: the
/// request is refused `live_unavailable`, not the node's failure, and
/// nothing is dialed.
#[tokio::test]
async fn a_node_without_the_live_route_is_live_unavailable() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(6);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Unserved).await);
    let refused = client
        .open(Ask::Watch {
            namespace: "loopback#1/echo/lobby".into(),
        })
        .await
        .err()
        .unwrap();
    assert_eq!(refused.code, refusal::LIVE_UNAVAILABLE);
    assert_eq!(refused.message, "this node serves no live tracks");
}

/// A control message that does not decode is a node of another wire, not
/// a closed stream: every request of the session ends refused
/// `node_failed` in words that say so, and the next dial waits instead of
/// failing the same way at once.
#[tokio::test]
async fn a_message_that_does_not_decode_ends_the_session_as_a_wire_mismatch() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(7);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Garble).await);
    let mut published = client
        .open(Ask::Publish {
            track: track("doc"),
            mode: Mode::Stream,
            priority: 128,
        })
        .await
        .unwrap();
    assert_eq!(published.heard.recv().await, Some(Heard::Published));
    let mut heard = client
        .open(Ask::Subscribe {
            track: track("doc"),
            from: Start::Latest,
        })
        .await
        .unwrap();
    for request in [&mut heard, &mut published] {
        let Some(Heard::Closed(refused)) = request.heard.recv().await else {
            panic!("the request ended without a word")
        };
        assert_eq!(refused.code, refusal::NODE_FAILED);
        assert!(
            refused
                .message
                .starts_with("the node speaks another live wire: control message #"),
            "{}",
            refused.message
        );
    }
    assert!(
        lock(&client.0.not_before).is_some_and(|until| until > Instant::now()),
        "the next dial does not wait"
    );
}

/// An app that quits closes its QUIC session itself: the node hears the
/// app close it, not its idle limit 30 s on, so a quit publisher's tracks
/// end for everyone at once.
#[tokio::test]
async fn a_quitting_app_closes_its_session_and_the_node_hears_it_at_once() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(8);
    let identity = node_key.public_key().as_ref().to_vec();
    let (rpc, mut ended) = node_hearing(&node_key, identity, Answer::Welcome).await;
    let client = Client::new(rpc);
    let mut published = client
        .open(Ask::Publish {
            track: track("presence/7"),
            mode: Mode::Datagram,
            priority: 128,
        })
        .await
        .unwrap();
    assert_eq!(published.heard.recv().await, Some(Heard::Published));
    client.quit().await;
    // the client's keep-alive holds an unclosed session open for good: a
    // bound far under the idle limit tells "closed" from "never closed"
    let heard = tokio::time::timeout(Duration::from_secs(5), ended.recv())
        .await
        .expect("the node did not hear the app quit")
        .unwrap();
    assert!(
        matches!(&heard, quinn::ConnectionError::ApplicationClosed(close) if close.reason == "quit"),
        "the node heard {heard:?}"
    );
}

/// A client on a test node with a stream track `doc` published, its
/// publication taken.
async fn publishing(seed: u64) -> (Client, Request) {
    let node_key = ed25519::PrivateKey::from_seed(seed);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Welcome).await);
    let mut published = client
        .open(Ask::Publish {
            track: track("doc"),
            mode: Mode::Stream,
            priority: 128,
        })
        .await
        .unwrap();
    assert_eq!(published.heard.recv().await, Some(Heard::Published));
    (client, published)
}

fn follow(from: Start) -> Ask {
    Ask::Subscribe {
        track: track("doc"),
        from,
    }
}

/// A subscription the node answered, its `Subscribed` read.
async fn following(client: &Client, from: Start) -> (Request, Option<u64>) {
    let mut request = client.open(follow(from)).await.unwrap();
    let Some(Heard::Subscribed { latest, .. }) = request.heard.recv().await else {
        panic!("the subscription was not answered")
    };
    (request, latest)
}

async fn send(published: &Request, group: u64, object: u32, body: &[u8], end: bool) {
    let object = Object {
        group,
        object,
        body: body.to_vec(),
        end,
    };
    published.objects.send(object).await.unwrap();
}

/// The next `count` items, bounded: an item that never comes fails the
/// test instead of holding it.
async fn heard(request: &mut Request, count: usize) -> Vec<Data> {
    let mut got = Vec::new();
    for _ in 0..count {
        let next = tokio::time::timeout(Duration::from_secs(5), request.data.recv());
        got.push(next.await.expect("an object never came").unwrap());
    }
    got
}

fn object(group: u64, object: u32, body: &[u8]) -> Data {
    Data::Object {
        group,
        object,
        body: body.to_vec(),
    }
}

/// Two windows that follow one track share the session's one subscription
/// (the node holds one subscriber per track and session) and hear the same
/// objects. A second publisher of the track in the app is refused: one
/// publisher per track.
#[tokio::test]
async fn two_requests_of_one_track_hear_the_same_objects() {
    let _seat = seated().await;
    let (client, published) = publishing(9).await;
    let (mut first, _) = following(&client, Start::Latest).await;
    let (mut second, _) = following(&client, Start::Latest).await;
    send(&published, 4, 0, b"a", false).await;
    send(&published, 4, 1, b"b", true).await;
    let whole = [
        object(4, 0, b"a"),
        object(4, 1, b"b"),
        Data::GroupEnd {
            group: 4,
            complete: true,
        },
    ];
    assert_eq!(heard(&mut first, 3).await, whole);
    assert_eq!(heard(&mut second, 3).await, whole);
    let again = client
        .open(Ask::Publish {
            track: track("doc"),
            mode: Mode::Stream,
            priority: 128,
        })
        .await
        .err()
        .unwrap();
    assert_eq!(again.code, refusal::DUPLICATE_TRACK);
}

/// A window that follows a track later, from the latest group, is handed
/// the group in flight from the app's copy, then hears it go on with the
/// others. One that asks for the next group hears nothing of it.
#[tokio::test]
async fn a_later_request_is_handed_the_group_in_flight() {
    let _seat = seated().await;
    let (client, published) = publishing(10).await;
    let (mut first, _) = following(&client, Start::Latest).await;
    send(&published, 4, 0, b"a", false).await;
    send(&published, 4, 1, b"b", false).await;
    assert_eq!(
        heard(&mut first, 2).await,
        [object(4, 0, b"a"), object(4, 1, b"b")]
    );
    let (mut later, latest) = following(&client, Start::Latest).await;
    assert_eq!(
        latest,
        Some(4),
        "the later window was not told the group in hand"
    );
    assert_eq!(
        heard(&mut later, 2).await,
        [object(4, 0, b"a"), object(4, 1, b"b")]
    );
    let (mut next, latest) = following(&client, Start::Next).await;
    assert_eq!(latest, None);
    send(&published, 4, 2, b"c", true).await;
    send(&published, 5, 0, b"d", false).await;
    let rest = [
        object(4, 2, b"c"),
        Data::GroupEnd {
            group: 4,
            complete: true,
        },
    ];
    assert_eq!(heard(&mut first, 2).await, rest);
    assert_eq!(heard(&mut later, 2).await, rest);
    assert_eq!(
        heard(&mut next, 1).await,
        [object(5, 0, b"d")],
        "a window that asked for the next group heard the one in flight"
    );
}

/// A window that stops following leaves the others running: the
/// subscription stays the node's until its last reader goes.
#[tokio::test]
async fn one_window_withdrawing_leaves_the_other_running() {
    let _seat = seated().await;
    let (client, published) = publishing(11).await;
    let (first, _) = following(&client, Start::Latest).await;
    let (mut second, _) = following(&client, Start::Latest).await;
    drop(first);
    send(&published, 4, 0, b"a", true).await;
    assert_eq!(
        heard(&mut second, 2).await,
        [
            object(4, 0, b"a"),
            Data::GroupEnd {
                group: 4,
                complete: true
            }
        ]
    );
}

/// Two windows that list one room share the session's one watcher: the
/// later is handed the tracks up now.
#[tokio::test]
async fn a_later_list_of_a_room_is_handed_the_tracks_up() {
    let _seat = seated().await;
    let (client, _published) = publishing(12).await;
    let watch = || Ask::Watch {
        namespace: "loopback#1/echo/lobby".into(),
    };
    let up = Heard::Announce {
        track: track("doc"),
        by: ACCOUNT,
        mode: Mode::Stream,
        up: true,
    };
    let mut first = client.open(watch()).await.unwrap();
    assert_eq!(first.heard.recv().await, Some(up.clone()));
    let mut later = client.open(watch()).await.unwrap();
    assert_eq!(later.heard.recv().await, Some(up));
}

/// The websocket door closes a socket on a message over one object's cap
/// and 1 KiB: the largest object the kernel lets through
/// (`MAX_STREAM_OBJECT`), framed with its group, fits.
#[test]
fn the_largest_object_framed_fits_the_nodes_websocket_cap() {
    let frame = WsFrame::Group {
        header: GroupHeader {
            alias: u32::MAX,
            group: u64::MAX,
            priority: u8::MAX,
        },
        objects: vec![vec![0; live_wire::MAX_STREAM_OBJECT]],
        end: true,
    };
    assert!(abi::encode(&frame).len() <= live_wire::MAX_STREAM_OBJECT + (1 << 10));
}

/// Over the websocket a group the node cut short (an object refused, the
/// track ended) has no end of its own: the next group starts. It ends
/// incomplete, not malformed.
#[test]
fn a_group_the_next_one_cuts_short_ends_incomplete() {
    let (control, _controls) = mpsc::unbounded_channel();
    let (frames, _queue) = mpsc::unbounded_channel();
    let conn = Conn {
        path: Path::Ws(frames),
        udp: None,
        control,
        table: Mutex::new(Table::default()),
        next: AtomicU64::new(1),
        closed: AtomicBool::new(false),
        client: Weak::new(),
    };
    let (mut request, ends) = request(|| {});
    let mut entry = Entry::new(
        Kind::Subscribe,
        follow(Start::Latest),
        Reader::new(1, &ends, false),
    );
    entry.established = true;
    entry.alias = Some(9);
    {
        let mut table = lock(&conn.table);
        table.requests.insert(1, entry);
        table.aliases.insert(9, 1);
    }
    let header = |group| GroupHeader {
        alias: 9,
        group,
        priority: 0,
    };
    heard_slice(&conn, header(1), vec![b"a".to_vec()], false);
    heard_slice(&conn, header(2), vec![b"b".to_vec()], true);
    let mut got = Vec::new();
    while let Ok(data) = request.data.try_recv() {
        got.push(data);
    }
    let object = |group, body: &[u8]| Data::Object {
        group,
        object: 0,
        body: body.to_vec(),
    };
    assert_eq!(
        got,
        [
            object(1, b"a"),
            Data::GroupEnd {
                group: 1,
                complete: false
            },
            object(2, b"b"),
            Data::GroupEnd {
                group: 2,
                complete: true
            },
        ]
    );
}

#[test]
fn the_newest_unread_object_replaces_the_one_before_and_is_counted() {
    let latest = Latest::default();
    let object = |object| Data::Object {
        group: 1,
        object,
        body: Vec::new(),
    };
    latest.put(object(0));
    latest.put(object(1));
    assert_eq!(futures::executor::block_on(latest.take()), object(1));
    assert_eq!(latest.dropped(), 1);
    assert_eq!(latest.dropped(), 0);
}
