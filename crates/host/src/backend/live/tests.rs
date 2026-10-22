use super::*;
use commonware_codec::Encode as _;
use commonware_cryptography::{Signer as _, ed25519};
use futures::FutureExt as _;
use keyscheme::KeyScheme as Verifier;
use live_wire::{Challenge, Welcome};
use noded_client::wire::{NODE_CONTRACT, Status};
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
    /// Welcomes, then answers a subscription as a node of another wire
    /// would: a `Subscribed` without the publisher's account.
    Garble,
    /// Never asks for one: answers UDP with its certificate, then never
    /// speaks on QUIC.
    Mute,
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

/// The live paths a test node serves.
#[derive(Clone, Copy, PartialEq)]
enum Paths {
    /// QUIC on its UDP port, and no websocket.
    Quic,
    /// The websocket, and a UDP port that takes packets and never answers,
    /// as a node behind a tunnel that carries only TCP.
    Ws,
    Both,
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
    node_serving(cert_key, identity, answer, Paths::Quic).await
}

/// [`node_hearing`] on the live `paths`: the websocket only welcomes, and
/// relays nothing.
async fn node_serving(
    cert_key: &ed25519::PrivateKey,
    identity: Vec<u8>,
    answer: Answer,
    paths: Paths,
) -> (RpcClient, mpsc::UnboundedReceiver<quinn::ConnectionError>) {
    let (cert, key) = certified(cert_key);
    let node_key = cert_key.public_key().as_ref().to_vec();
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
    let (ended, ends) = mpsc::unbounded_channel();
    let (tls, tls_done) = watch::channel(false);
    let port = if paths == Paths::Ws {
        let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut packet = [0; 2048];
            while silent.recv(&mut packet).await.is_ok() {}
        });
        port
    } else {
        let endpoint = quinn::Endpoint::server(server, "127.0.0.1:0".parse().unwrap()).unwrap();
        let port = endpoint.local_addr().unwrap().port();
        let node_key = node_key.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let connection = incoming.await.unwrap();
                tls.send_replace(true);
                let (closing, ended) = (connection.clone(), ended.clone());
                tokio::spawn(async move {
                    let _ = ended.send(closing.closed().await);
                });
                tokio::spawn(relay(connection, node_key.clone(), answer));
            }
        });
        port
    };
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
        roles: crate::backend::test_roles(),
    };
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", http.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = http.accept().await {
            let status = abi::encode(&status);
            let node_key = node_key.clone();
            let mut tls_done = tls_done.clone();
            tokio::spawn(async move {
                let mut asked = vec![0; 4096];
                let read = stream.peek(&mut asked).await.unwrap();
                let upgrade = format!("GET {} ", live_wire::ROUTE);
                if paths != Paths::Quic && asked[..read].starts_with(upgrade.as_bytes()) {
                    // a client that drops the socket mid-upgrade (QUIC was
                    // the session) ends it
                    if matches!(answer, Answer::Mute) {
                        let _ = tls_done.wait_for(|done| *done).await;
                    }
                    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    return ws_relay(socket, node_key).await;
                }
                let read = stream.read(&mut asked).await.unwrap();
                let line = String::from_utf8_lossy(&asked[..read]);
                // the status, the port and the websocket, and nothing else
                let body = if line.starts_with("GET /v1/status ") {
                    Some(status)
                } else if line.starts_with(&format!("GET {} ", live_wire::PORT_ROUTE)) {
                    Some(abi::encode(&Some(port)))
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

/// The node's side of one session: a proof that does not verify against
/// its own key `node` in the preimage is refused `bad_proof`, as core's
/// relay refuses it.
async fn relay(connection: quinn::Connection, node: Vec<u8>, answer: Answer) {
    if matches!(answer, Answer::Mute) {
        connection.closed().await;
        return;
    }
    let (mut send, mut recv) = connection.accept_bi().await.unwrap();
    let Control::Hello(hello) = read(&mut recv).await else {
        panic!("a session opens with a hello")
    };
    let nonce = [3; 32];
    write_control(&mut send, Control::Challenge(Challenge { nonce })).await;
    let Control::Proof(proof) = read(&mut recv).await else {
        panic!("the challenge is answered with a proof")
    };
    if !proven(&hello, &node, &nonce, &proof) || matches!(answer, Answer::Refuse) {
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
                let mut head = [0u8; GroupHeader::LEN];
                incoming.read_exact(&mut head).await.unwrap();
                let header: GroupHeader = abi::decode(&head).unwrap();
                assert_eq!(header.alias, PUBLISHED);
                let mut out = streams.open_uni().await.unwrap();
                let header = GroupHeader {
                    alias: SUBSCRIBED,
                    ..header
                };
                out.write_all(&abi::encode(&header)).await.unwrap();
                // as the bytes come, so a group is in flight while it runs;
                // a group the publisher reset ends short, as core's does
                match tokio::io::copy(&mut incoming, &mut out).await {
                    Ok(_) => out.finish().unwrap(),
                    Err(_) => out.reset(0u32.into()).unwrap(),
                }
            });
        }
    });
    let datagrams = connection.clone();
    tokio::spawn(async move {
        while let Ok(bytes) = datagrams.read_datagram().await {
            let mut header: DatagramHeader = abi::decode(&bytes[..DatagramHeader::LEN]).unwrap();
            assert_eq!(header.alias, PUBLISHED);
            header.alias = SUBSCRIBED;
            let mut back = abi::encode(&header);
            back.extend_from_slice(&bytes[DatagramHeader::LEN..]);
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
            // a room that grants nothing yet: the node holds the request
            Control::Subscribe { request, track, .. } if track.name == "closed" => {
                Control::Waiting {
                    request,
                    code: Code::NotGranted,
                }
            }
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

/// Whether `proof` signs the challenge `nonce` for the node `node` with the
/// hello's key, the hello naming the test chain.
fn proven(hello: &Hello, node: &[u8], nonce: &[u8; 32], proof: &Proof) -> bool {
    assert_eq!(hello.chain, [9; 32], "the hello names the genesis");
    let preimage = live_wire::proof_preimage(&hello.chain, node, nonce, &hello.key);
    Verifier::Ed25519.verify(
        &hello.key,
        live_wire::NAMESPACE,
        &preimage,
        &proof.signature,
    )
}

/// The node's side of one websocket session: the handshake, then the
/// socket held open until the client goes; nothing is relayed. A client
/// that drops the socket mid-handshake (QUIC was the session) ends it.
async fn ws_relay(
    mut socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    node: Vec<u8>,
) {
    async fn heard(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    ) -> Option<Control> {
        let Message::Binary(bytes) = socket.next().await?.ok()? else {
            return None;
        };
        let Ok(WsFrame::Control(control)) = abi::decode(&bytes) else {
            panic!("the handshake is control frames")
        };
        Some(control)
    }
    let say = |control| Message::Binary(abi::encode(&WsFrame::Control(control)));
    let Some(Control::Hello(hello)) = heard(&mut socket).await else {
        return;
    };
    let nonce = [4; 32];
    let challenge = Control::Challenge(Challenge { nonce });
    if socket.send(say(challenge)).await.is_err() {
        return;
    }
    let Some(Control::Proof(proof)) = heard(&mut socket).await else {
        return;
    };
    assert!(proven(&hello, &node, &nonce, &proof));
    let welcome = Welcome {
        account: ACCOUNT,
        datagrams: false,
    };
    if socket.send(say(Control::Welcome(welcome))).await.is_ok() {
        while let Some(Ok(_)) = socket.next().await {}
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
    crate::backend::seat_key("loopback", Arc::new(ed25519::PrivateKey::from_seed(11))).await;
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
    let objects = Outbox::new(Mode::Stream);
    let mut published = client
        .open(Ask::Publish {
            track: track("doc"),
            mode: Mode::Stream,
            priority: 128,
            objects: objects.clone(),
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
        assert_eq!(objects.push(object), Pushed::Queued);
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
    let objects = Outbox::new(Mode::Datagram);
    let mut published = client
        .open(Ask::Publish {
            track: track("cursor/7"),
            mode: Mode::Datagram,
            priority: 128,
            objects: objects.clone(),
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
    objects.push(object);
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
        lock(&client.0.not_before).is_none(),
        "the app invents no wait: the refusal was the answer"
    );
}

/// A certificate that holds another key than the status names is not the
/// node: QUIC is refused, the websocket beside it is the one left, and
/// this node has none.
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

#[tokio::test]
async fn a_node_whose_udp_is_silent_opens_over_the_websocket_while_quic_still_waits() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(41);
    let identity = node_key.public_key().as_ref().to_vec();
    let (rpc, _) = node_serving(&node_key, identity, Answer::Welcome, Paths::Ws).await;
    let never = std::future::pending::<()>;
    let conn = dial_with_deadlines(&rpc, Weak::new(), never, never)
        .await
        .unwrap();
    assert!(
        matches!(conn.path, Path::Ws(_)),
        "the websocket is the session"
    );
}

/// A node serving both paths: QUIC and the websocket start together. On
/// loopback the websocket (no TLS, no round trip to speak of) finishes
/// first, and QUIC, its UDP having answered, is still the session: the
/// path that carries datagrams.
#[tokio::test]
async fn a_node_serving_both_paths_opens_over_quic() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(42);
    let identity = node_key.public_key().as_ref().to_vec();
    let (rpc, _) = node_serving(&node_key, identity, Answer::Welcome, Paths::Both).await;
    let conn = dial(&rpc, Weak::new()).await.unwrap();
    assert!(matches!(conn.path, Path::Quic(_)), "QUIC is the session");
}

/// A node that refuses the key on QUIC after the websocket welcomed it:
/// the welcome stands, and the websocket is the session.
#[tokio::test]
async fn a_quic_refusal_after_the_websocket_welcome_keeps_the_websocket() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(43);
    let identity = node_key.public_key().as_ref().to_vec();
    let (rpc, _) = node_serving(&node_key, identity, Answer::Refuse, Paths::Both).await;
    let conn = dial(&rpc, Weak::new()).await.unwrap();
    assert!(matches!(conn.path, Path::Ws(_)), "the welcome stands");
}

#[tokio::test]
async fn a_quic_handshake_that_stalls_after_tls_is_waited_for_until_quic_is_late() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(44);
    let identity = node_key.public_key().as_ref().to_vec();
    let (rpc, _) = node_serving(&node_key, identity, Answer::Mute, Paths::Both).await;
    let (waits, waiting) = oneshot::channel();
    let (late, quic_is_late) = oneshot::channel::<()>();
    let quic_waited_on = move || {
        let _ = waits.send(());
        async move {
            let _ = quic_is_late.await;
        }
    };
    let never = std::future::pending::<()>;
    let mut dialing = std::pin::pin!(dial_with_deadlines(
        &rpc,
        Weak::new(),
        never,
        quic_waited_on
    ));
    tokio::select! {
        _ = &mut dialing => panic!("the welcomed websocket did not wait on QUIC"),
        _ = waiting => {}
    }
    assert!(
        dialing.as_mut().now_or_never().is_none(),
        "the welcomed websocket waits on QUIC until QUIC is late"
    );
    let _ = late.send(());
    let Some(conn) = dialing.as_mut().now_or_never() else {
        panic!("the dial still waits once QUIC is late")
    };
    assert!(
        matches!(conn.unwrap().path, Path::Ws(_)),
        "the welcome stands"
    );
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
            objects: Outbox::new(Mode::Stream),
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
            objects: Outbox::new(Mode::Datagram),
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
    assert!(
        lock(&client.0.not_before).is_none(),
        "a quit is no failure: the next dial does not wait"
    );
}

/// A session the node closes is lost: its requests end (their channels
/// close), and the app invents no wait before the next dial: the node's
/// `GoAway` is the one wait, and it names it.
#[tokio::test]
async fn a_lost_session_ends_its_requests_and_the_next_dial_waits_on_nothing() {
    let _seat = seated().await;
    let node_key = ed25519::PrivateKey::from_seed(14);
    let identity = node_key.public_key().as_ref().to_vec();
    let (rpc, mut ended) = node_hearing(&node_key, identity, Answer::Welcome).await;
    let client = Client::new(rpc);
    let mut watch = client
        .open(Ask::Watch {
            namespace: "loopback#1/echo/lobby".into(),
        })
        .await
        .unwrap();
    let conn = client.0.conn.lock().await.clone().unwrap();
    let Path::Quic(connection) = &conn.path else {
        panic!("the test needs the QUIC path")
    };
    // the node's side goes: as a restart looks from here
    connection.close(1u32.into(), b"node restart");
    drop(conn);
    assert!(
        watch.heard.recv().await.is_none(),
        "the request's channel closes"
    );
    let _ = ended.recv().await;
    assert!(lock(&client.0.not_before).is_none());
}

/// A client on a test node with a stream track `doc` published, its
/// publication taken: the client, the publication and its queue.
async fn publishing(seed: u64) -> (Client, Request, Arc<Outbox>) {
    let node_key = ed25519::PrivateKey::from_seed(seed);
    let identity = node_key.public_key().as_ref().to_vec();
    let client = Client::new(node(&node_key, identity, Answer::Welcome).await);
    let objects = Outbox::new(Mode::Stream);
    let mut published = client
        .open(Ask::Publish {
            track: track("doc"),
            mode: Mode::Stream,
            priority: 128,
            objects: objects.clone(),
        })
        .await
        .unwrap();
    assert_eq!(published.heard.recv().await, Some(Heard::Published));
    (client, published, objects)
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

fn send(published: &Outbox, group: u64, object: u32, body: &[u8], end: bool) {
    let object = Object {
        group,
        object,
        body: body.to_vec(),
        end,
    };
    assert_eq!(published.push(object), Pushed::Queued);
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
    let (client, _published, published) = publishing(9).await;
    let (mut first, _) = following(&client, Start::Latest).await;
    let (mut second, _) = following(&client, Start::Latest).await;
    send(&published, 4, 0, b"a", false);
    send(&published, 4, 1, b"b", true);
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
            objects: Outbox::new(Mode::Stream),
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
    let (client, _published, published) = publishing(10).await;
    let (mut first, _) = following(&client, Start::Latest).await;
    send(&published, 4, 0, b"a", false);
    send(&published, 4, 1, b"b", false);
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
    send(&published, 4, 2, b"c", true);
    send(&published, 5, 0, b"d", false);
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

/// A window that asks `Latest` after the group ended is handed the last
/// group whole, its end with it (the node's cache rule, kept here since
/// the node's one subscription was asked once); a newer group's first
/// object replaces it.
#[tokio::test]
async fn a_later_request_after_the_group_ended_is_handed_it_whole() {
    let _seat = seated().await;
    let (client, _published, published) = publishing(21).await;
    let (mut first, _) = following(&client, Start::Latest).await;
    send(&published, 4, 0, b"state", true);
    let whole = [
        object(4, 0, b"state"),
        Data::GroupEnd {
            group: 4,
            complete: true,
        },
    ];
    assert_eq!(heard(&mut first, 2).await, whole);
    let (mut later, latest) = following(&client, Start::Latest).await;
    assert_eq!(latest, Some(4), "the node holds group 4; so does the app");
    assert_eq!(heard(&mut later, 2).await, whole);
    send(&published, 5, 0, b"next", false);
    assert_eq!(heard(&mut later, 1).await, [object(5, 0, b"next")]);
    let (mut third, latest) = following(&client, Start::Latest).await;
    assert_eq!(latest, Some(5));
    assert_eq!(heard(&mut third, 1).await, [object(5, 0, b"next")]);
}

/// A watch the node held and then granted on a room with nothing up is
/// answered by `Watching` alone: a window joining it after that hears no
/// stale Waiting (and no Announce, there being none).
#[test]
fn a_granted_watch_of_an_empty_room_ends_its_waiting() {
    let (conn, _request) = ws_conn();
    let room = "loopback#1/echo/lobby";
    {
        let mut table = lock(&conn.table);
        let entry = table.requests.get_mut(&1).unwrap();
        entry.kind = Kind::Watch {
            namespace: room.into(),
            up: Vec::new(),
        };
        table.aliases.clear();
    }
    conn.heard(Control::Waiting {
        request: 1,
        code: Code::NotGranted,
    })
    .unwrap();
    conn.heard(Control::Watching { request: 1 }).unwrap();
    let conn = Arc::new(conn);
    let mut later = conn
        .open(Ask::Watch {
            namespace: room.into(),
        })
        .unwrap();
    assert_eq!(later.heard.try_recv().ok(), None);
}

/// A window that joins an ask the node holds hears that it waits, as the
/// first did, and the node's answer when it comes.
#[test]
fn a_reader_joining_a_waiting_ask_hears_waiting() {
    let (conn, _request) = ws_conn();
    {
        let mut table = lock(&conn.table);
        let entry = table.requests.get_mut(&1).unwrap();
        let Kind::Subscribe { alias, .. } = &mut entry.kind else {
            panic!("a subscription")
        };
        *alias = None;
        table.aliases.clear();
    }
    conn.heard(Control::Waiting {
        request: 1,
        code: Code::NotGranted,
    })
    .unwrap();
    let conn = Arc::new(conn);
    let mut later = conn.open(follow(Start::Latest)).unwrap();
    assert_eq!(later.heard.try_recv().ok(), Some(Heard::Waiting));
    conn.heard(Control::Subscribed {
        request: 1,
        alias: 9,
        by: ACCOUNT,
        mode: Mode::Stream,
        latest: None,
    })
    .unwrap();
    assert!(matches!(
        later.heard.try_recv().ok(),
        Some(Heard::Subscribed { .. })
    ));
    let mut third = conn.open(follow(Start::Latest)).unwrap();
    assert!(
        matches!(third.heard.try_recv().ok(), Some(Heard::Subscribed { .. })),
        "the ask was answered: a joiner no longer waits"
    );
}

/// A window that stops following leaves the others running: the
/// subscription stays the node's until its last reader goes.
#[tokio::test]
async fn one_window_withdrawing_leaves_the_other_running() {
    let _seat = seated().await;
    let (client, _published, published) = publishing(11).await;
    let (first, _) = following(&client, Start::Latest).await;
    let (mut second, _) = following(&client, Start::Latest).await;
    drop(first);
    send(&published, 4, 0, b"a", true);
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
/// A request the node holds hears it, and the session goes on: the next
/// request is answered on it.
#[tokio::test]
async fn a_request_the_node_holds_hears_it_waits_and_the_session_goes_on() {
    let _seat = seated().await;
    let (client, _published, _objects) = publishing(9).await;
    let mut held = client
        .open(Ask::Subscribe {
            track: track("closed"),
            from: Start::Next,
        })
        .await
        .unwrap();
    assert_eq!(held.heard.recv().await, Some(Heard::Waiting));
    let (_following, latest) = following(&client, Start::Next).await;
    assert_eq!(latest, None);
}

#[tokio::test]
async fn a_later_list_of_a_room_is_handed_the_tracks_up() {
    let _seat = seated().await;
    let (client, _published, _objects) = publishing(12).await;
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
        },
        objects: vec![vec![0; live_wire::MAX_STREAM_OBJECT]],
        end: Ending::Complete,
    };
    assert!(abi::encode(&frame).len() <= live_wire::WS_MESSAGE);
}

/// A websocket session of one subscription, as the table holds it after
/// the node's `Subscribed`: the connection and the subscription's request.
fn ws_conn() -> (Conn, Request) {
    let (control, _controls) = mpsc::unbounded_channel();
    let (frames, _queue) = mpsc::unbounded_channel();
    std::mem::forget((_controls, _queue));
    let conn = Conn {
        path: Path::Ws(frames),
        udp: None,
        control,
        table: Mutex::new(Table::default()),
        next: AtomicU64::new(1),
        closed: watch::Sender::new(false),
        aliased: Notify::new(),
        tasks: Mutex::new(Vec::new()),
        client: Weak::new(),
    };
    let (request, ends) = request(|| {});
    conn.next.store(2, Ordering::SeqCst);
    let entry = Entry {
        readers: vec![Reader::new(1, &ends, Start::Latest)],
        waiting: false,
        kind: Kind::Subscribe {
            track: track("doc"),
            alias: Some(9),
            counted: HashMap::new(),
            opened: None,
            in_flight: None,
        },
    };
    {
        let mut table = lock(&conn.table);
        table.requests.insert(1, entry);
        table.aliases.insert(9, 1);
    }
    (conn, request)
}

fn drain(request: &mut Request) -> Vec<Data> {
    let mut got = Vec::new();
    while let Ok(data) = request.data.try_recv() {
        got.push(data);
    }
    got
}

/// Over the websocket the node sends a group's end in the order its
/// session loop took it, which can be after the next group's first
/// objects (the publisher FINs one and opens the next back to back). Each
/// group is counted on its own: a late end neither restarts the newer
/// group's numbering nor ends it, and ends the one it names, complete.
#[test]
fn a_late_end_of_the_old_group_leaves_the_new_groups_count_alone() {
    let (conn, mut request) = ws_conn();
    let header = |group| GroupHeader { alias: 9, group };
    heard_slice(&conn, header(1), vec![b"a".to_vec()], Ending::More);
    heard_slice(&conn, header(2), vec![b"b".to_vec()], Ending::More);
    heard_slice(&conn, header(1), vec![], Ending::Complete);
    heard_slice(&conn, header(2), vec![b"c".to_vec()], Ending::Complete);
    let object = |group, object, body: &[u8]| Data::Object {
        group,
        object,
        body: body.to_vec(),
    };
    assert_eq!(
        drain(&mut request),
        [
            object(1, 0, b"a"),
            object(2, 0, b"b"),
            Data::GroupEnd {
                group: 1,
                complete: true
            },
            object(2, 1, b"c"),
            Data::GroupEnd {
                group: 2,
                complete: true
            },
        ]
    );
}

/// The node sends `Subscribed` on the control stream and the replayed group
/// on its own stream; the group can land first. An alias not yet named is
/// "not yet": the group's reader waits for the `Subscribed` that names it,
/// then delivers. An alias whose subscription went is "nobody": the stream
/// is stopped.
#[tokio::test]
async fn a_group_of_an_alias_not_yet_subscribed_waits_for_its_name() {
    let (conn, _request) = ws_conn();
    let conn = Arc::new(conn);
    let item = Data::Object {
        group: 1,
        object: 0,
        body: vec![1],
    };
    let mut parked = std::pin::pin!(conn.readers_of(10, &item));
    assert!(
        futures::poll!(parked.as_mut()).is_pending(),
        "not yet: parked"
    );
    // a second track's request, named alias 10 by the node
    let mut second = conn
        .open(Ask::Subscribe {
            track: track("other"),
            from: Start::Latest,
        })
        .unwrap();
    let request = *lock(&conn.table)
        .requests
        .iter()
        .find(|(_, entry)| matches!(&entry.kind, Kind::Subscribe { track, .. } if track.name == "other"))
        .unwrap()
        .0;
    conn.heard(Control::Subscribed {
        request,
        alias: 10,
        by: ACCOUNT,
        mode: Mode::Stream,
        latest: Some(1),
    })
    .unwrap();
    let readers = parked.await.expect("named now");
    assert_eq!(readers.len(), 1);
    readers[0].send(item.clone()).await.unwrap();
    assert_eq!(second.data.recv().await, Some(item.clone()));
    drop(second);
    assert!(
        conn.readers_of(10, &item).await.is_none(),
        "the subscription went: nobody"
    );
    conn.close(Close::Quit);
    assert!(
        conn.readers_of(11, &item).await.is_none(),
        "a closed session parks nothing"
    );
}

/// Over the websocket a group the node cut short (an object refused, the
/// publisher gone, a replay abandoned) says so in its frame: it ends
/// incomplete, its count goes, and the next group is counted from 0.
#[test]
fn a_cut_frame_ends_the_group_short() {
    let (conn, mut request) = ws_conn();
    let header = |group| GroupHeader { alias: 9, group };
    heard_slice(&conn, header(1), vec![b"a".to_vec()], Ending::More);
    heard_slice(&conn, header(1), vec![], Ending::Cut);
    heard_slice(&conn, header(2), vec![b"b".to_vec()], Ending::Complete);
    let object = |group, body: &[u8]| Data::Object {
        group,
        object: 0,
        body: body.to_vec(),
    };
    assert_eq!(
        drain(&mut request),
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

/// A websocket reader that falls behind has its group cut, never a hole:
/// the objects it took, then `GroupEnd { complete: false }` once, and the
/// rest of the group (its real end too) is dropped for it. The next group
/// reaches it whole once it reads again.
#[test]
fn a_websocket_reader_behind_has_its_group_cut_not_a_hole() {
    let (conn, mut request) = ws_conn();
    let header = |group| GroupHeader { alias: 9, group };
    let bodies: Vec<Vec<u8>> = (0..=DATA_BACKLOG as u8).map(|i| vec![i]).collect();
    heard_slice(&conn, header(1), bodies, Ending::More);
    let got = drain(&mut request);
    // one slot of the backlog holds the group's end
    let taken = DATA_BACKLOG - 1;
    assert_eq!(got.len(), taken + 1, "{got:?}");
    assert!(got[..taken].iter().enumerate().all(|(i, data)| *data
        == Data::Object {
            group: 1,
            object: i as u32,
            body: vec![i as u8]
        }));
    assert_eq!(
        got[taken],
        Data::GroupEnd {
            group: 1,
            complete: false
        }
    );
    heard_slice(&conn, header(1), vec![vec![200]], Ending::Complete);
    assert_eq!(
        drain(&mut request),
        [],
        "the rest of a cut group is dropped"
    );
    heard_slice(&conn, header(2), vec![vec![1]], Ending::Complete);
    assert_eq!(
        drain(&mut request),
        [
            Data::Object {
                group: 2,
                object: 0,
                body: vec![1]
            },
            Data::GroupEnd {
                group: 2,
                complete: true
            },
        ]
    );
}

/// A publication's queue abandons a stream group: the writer resets the
/// group's stream, the node ends it short for its subscribers, and the
/// next group goes out whole.
#[tokio::test]
async fn an_abandoned_group_is_reset_and_ends_short_for_subscribers() {
    let _seat = seated().await;
    let (client, _published, published) = publishing(13).await;
    let (mut heard, _) = following(&client, Start::Latest).await;
    send(&published, 4, 0, b"a", false);
    assert_eq!(
        super::tests::heard(&mut heard, 1).await,
        [object(4, 0, b"a")]
    );
    // the queue at its capacity: every further object of the group cuts it
    for _ in 0..OUTBOX {
        published.push(Object {
            group: 4,
            object: 1,
            body: b"b".to_vec(),
            end: false,
        });
    }
    let overflow = Object {
        group: 4,
        object: 1,
        body: b"b".to_vec(),
        end: false,
    };
    assert_eq!(published.push(overflow.clone()), Pushed::Cut(4));
    assert_eq!(published.push(overflow), Pushed::Dropped);
    send(&published, 5, 0, b"c", true);
    assert_eq!(
        super::tests::heard(&mut heard, 3).await,
        [
            Data::GroupEnd {
                group: 4,
                complete: false
            },
            object(5, 0, b"c"),
            Data::GroupEnd {
                group: 5,
                complete: true
            },
        ]
    );
}

/// A datagram publication's queue at its capacity drops the oldest
/// datagram, so the newest goes out; the publication ending closes the
/// queue, and a push after that says so.
#[test]
fn a_full_datagram_queue_drops_the_oldest_and_a_closed_one_refuses() {
    let outbox = Outbox::new(Mode::Datagram);
    let object = |object| Object {
        group: 1,
        object,
        body: Vec::new(),
        end: false,
    };
    for i in 0..=OUTBOX as u32 {
        assert_eq!(outbox.push(object(i)), Pushed::Queued);
    }
    let first = futures::executor::block_on(outbox.take());
    assert_eq!(first, Some(Out::Object(object(1))), "object 0 went");
    outbox.close();
    assert_eq!(outbox.push(object(99)), Pushed::Closed);
    for i in 2..=OUTBOX as u32 {
        assert_eq!(
            futures::executor::block_on(outbox.take()),
            Some(Out::Object(object(i)))
        );
    }
    assert_eq!(futures::executor::block_on(outbox.take()), None);
}

/// The newest wins: a newer unread datagram replaces the held one and is
/// counted; an older one, reordered on the path, leaves the held one and
/// counts for nothing.
#[test]
fn the_newest_unread_object_replaces_the_one_before_and_is_counted() {
    let latest = Latest::default();
    let object = |group, object| Data::Object {
        group,
        object,
        body: Vec::new(),
    };
    latest.put(object(1, 0));
    latest.put(object(1, 1));
    assert_eq!(futures::executor::block_on(latest.take()), object(1, 1));
    assert_eq!(latest.dropped(), 1);
    assert_eq!(latest.dropped(), 0);
    latest.put(object(2, 5));
    latest.put(object(2, 4));
    latest.put(object(1, 9));
    assert_eq!(futures::executor::block_on(latest.take()), object(2, 5));
    assert_eq!(latest.dropped(), 0);
}

/// Over the websocket the app pings every `KEEP_ALIVE`, so no proxy on the
/// way sees the socket idle; a node that answers the pings keeps the
/// session past `IDLE`, and one that falls silent ends it at `IDLE`.
#[tokio::test(start_paused = true)]
async fn the_websocket_is_kept_alive_and_a_silent_one_ends() {
    use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};
    let (app, node) = tokio::io::duplex(1 << 16);
    let app = WebSocketStream::from_raw_socket(app, Role::Client, None).await;
    let mut node = WebSocketStream::from_raw_socket(node, Role::Server, None).await;
    let (control, controls) = mpsc::unbounded_channel();
    let (frames, queue) = mpsc::unbounded_channel();
    let conn = Arc::new(Conn {
        path: Path::Ws(frames),
        udp: None,
        control,
        table: Mutex::new(Table::default()),
        next: AtomicU64::new(1),
        closed: watch::Sender::new(false),
        aliased: Notify::new(),
        tasks: Mutex::new(Vec::new()),
        client: Weak::new(),
    });
    run_ws(app, queue, controls, conn.clone());
    // the node reads, and reading answers each ping
    for _ in 0..4 {
        let heard = tokio::time::timeout(KEEP_ALIVE * 2, node.next()).await;
        assert!(matches!(heard, Ok(Some(Ok(Message::Ping(_))))));
    }
    assert!(!conn.closed());
    // the node stops reading: its last pong left a ping ago
    tokio::time::sleep(IDLE).await;
    assert!(conn.closed());
}

/// Closing a websocket session ends its writer at once, with the socket's
/// close frame: not at the writer's next ping tick.
#[tokio::test(start_paused = true)]
async fn a_closed_websocket_session_sends_its_close_frame_at_once() {
    use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};
    let (app, node) = tokio::io::duplex(1 << 16);
    let app = WebSocketStream::from_raw_socket(app, Role::Client, None).await;
    let mut node = WebSocketStream::from_raw_socket(node, Role::Server, None).await;
    let (control, controls) = mpsc::unbounded_channel();
    let (frames, queue) = mpsc::unbounded_channel();
    let conn = Arc::new(Conn {
        path: Path::Ws(frames),
        udp: None,
        control,
        table: Mutex::new(Table::default()),
        next: AtomicU64::new(1),
        closed: watch::Sender::new(false),
        aliased: Notify::new(),
        tasks: Mutex::new(Vec::new()),
        client: Weak::new(),
    });
    run_ws(app, queue, controls, conn.clone());
    let first = tokio::time::timeout(KEEP_ALIVE * 2, node.next()).await;
    assert!(matches!(first, Ok(Some(Ok(Message::Ping(_))))));
    let closed_at = tokio::time::Instant::now();
    conn.close(Close::Quit);
    let heard = node.next().await;
    assert!(matches!(heard, Some(Ok(Message::Close(_)))), "{heard:?}");
    // on paused time an idle wait would have advanced the clock to the tick
    assert_eq!(tokio::time::Instant::now(), closed_at);
    let tasks = std::mem::take(&mut *lock(&conn.tasks));
    futures::future::join_all(tasks).await;
}

/// A closed QUIC session is freed with its close: every task of it ends
/// (the control writer waited on a queue only the session fed), so the
/// quinn connection, endpoint and UDP socket go with it, not at the
/// process's end.
#[tokio::test]
async fn a_closed_quic_session_is_freed() {
    let _seat = seated().await;
    let (client, published, _objects) = publishing(31).await;
    let conn = client.0.conn.lock().await.clone().unwrap();
    assert!(conn.udp.is_some(), "the test needs the QUIC path");
    let weak = Arc::downgrade(&conn);
    drop(conn);
    drop(published);
    // the Unpublish goes out before the close: no queued message wakes
    // the writer after it
    tokio::task::yield_now().await;
    client.quit().await;
    assert!(
        weak.upgrade().is_none(),
        "still held: strong {}",
        weak.strong_count()
    );
}
