//! The node's call hub, held for a view as one subscription: `voice.hub
//! {"channel"}` upgrades `/v1/call/ws` on the connected node with the proof
//! THIS device can make — the same two roles as [`super::open_topic`] — and
//! every frame the hub sends is one item, verbatim, in the `net.stream`
//! encoding the view already reads (`{"text"}` / `{"binary"}`), after one
//! text item the host synthesizes on the upgrade: [`READY`] — the hub sends
//! no ready frame of its own, and the roster is consensus state the view
//! already reads from the chat guest. The view's
//! `net.send` on the subscription's id is the uplink. The host decodes no
//! frame: control is the hub's json, media is the hub's bytes (#2445), and
//! both are the view's to read.
//!
//! Huddle media rides the network data plane, not a gateway route (ruling
//! 2026-09-20): this is the node's own hub, so the channel is the only thing
//! a view names — the hub owns the room.

use super::{Guest, NodeSocket, host_fault, socket_failed, wire};

/// The kind names the view asks by, and the hub's path on the node — in one
/// place, because the node restates them when its hub lands.
pub(super) const CAPABILITY: &str = "voice";
pub(super) const OPERATION: &str = "hub";
const WS_PATH: &str = "/v1/call/ws";
/// The first item of a joined hub, from the host, not the hub.
const READY: &str = r#"{"type":"ready"}"#;

pub(super) fn answer(guest: &mut Guest, operation: &str, id: u64, payload: &[u8]) {
    if operation != OPERATION {
        guest.refuse(
            id,
            "malformed_request",
            format!("`{CAPABILITY}.{operation}` is not a voice operation"),
        );
        return;
    }
    guest.tasks.retain(|(_, stream)| !stream.task.is_finished());
    let channel = match channel_of(payload) {
        Ok(channel) => channel,
        Err(refusal) => {
            guest.reply(id, Err(refusal));
            return;
        }
    };
    super::hold_exchange(guest, id, Some(READY), move |client| {
        let channel = channel.clone();
        Box::pin(async move { open_hub(&client, &channel).await })
    });
}

/// The one input: a plain-token channel id, which the query string carries
/// unescaped — and the signed path+query must be the requested one.
fn channel_of(payload: &[u8]) -> Result<String, wire::Refusal> {
    let ask: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| super::malformed(format!("request is not JSON: {error}")))?;
    let channel = ask["channel"].as_str().unwrap_or_default();
    let plain = !channel.is_empty()
        && channel
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:~".contains(&byte));
    match plain {
        true => Ok(channel.to_owned()),
        false => Err(super::malformed(format!(
            "`{CAPABILITY}.{OPERATION}` names no channel"
        ))),
    }
}

/// The hub upgrade, proven as [`super::open_topic`] proves a topic: the
/// 0600 workspace token on the query when this device hosts the node, the
/// seated key's signature over `GET` + this exact path+query otherwise. The
/// hub's refusals ([`hub_failed`]) reach the view under the hub's own tokens.
async fn open_hub(
    client: &ducktape_rpc::Client,
    channel: &str,
) -> Result<NodeSocket, wire::Refusal> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
    let rpc = client.origin();
    let path = format!("{WS_PATH}?channel={channel}");
    let ws_origin = crate::backend::agent_ws_url(rpc);
    let ws_origin = ws_origin
        .strip_suffix("/v1/ws")
        .ok_or_else(|| host_fault("invalid node websocket origin"))?;
    let workspace_token = crate::backend::workspace_at(rpc)
        .and_then(|(_, workspace)| crate::backend::read_link_token(&workspace).ok());
    let mut request = match &workspace_token {
        Some(token) => format!("{ws_origin}{path}&token={token}"),
        None => format!("{ws_origin}{path}"),
    }
    .into_client_request()
    .map_err(|error| host_fault(format!("could not address the node: {error}")))?;
    if workspace_token.is_none() {
        let node_key = crate::backend::node_public_key(client).await?;
        let signed = crate::backend::seated_request_headers("GET", &path, &node_key, b"")
            .await
            .ok_or_else(crate::backend::locked_seat)?;
        for (name, value) in signed {
            let value = HeaderValue::from_str(&value).map_err(|error| {
                host_fault(format!("the signature is not a header value: {error}"))
            })?;
            request
                .headers_mut()
                .insert(HeaderName::from_static(name), value);
        }
    }
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(super::MAX_STREAM_FRAME_BYTES),
        max_frame_size: Some(super::MAX_STREAM_FRAME_BYTES),
        ..Default::default()
    };
    // Nagle off: one 20 ms audio frame per write, never coalesced.
    let (socket, _) = tokio_tungstenite::connect_async_with_config(request, Some(config), true)
        .await
        .map_err(hub_failed)?;
    Ok(socket)
}

/// The upgrade the hub refused, under the hub's own token — `key_without_account`
/// and `not_in_huddle` are the node's 403 body verbatim, a 503 (the node runs
/// no hub) is `no_call_hub` — or a node that never answered, `node_unreachable`.
/// The view names each case; the frame cap stays the transport's own.
fn hub_failed(error: tokio_tungstenite::tungstenite::Error) -> wire::Refusal {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Http(response) => {
            if response.status() == ducktape_rpc::StatusCode::SERVICE_UNAVAILABLE {
                return wire::Refusal::new("no_call_hub", "this node runs no voice hub");
            }
            let body = response.body().clone().unwrap_or_default();
            super::refused(ducktape_rpc::refusal(response.status(), &body))
        }
        Error::Capacity(_) => socket_failed(error),
        error => wire::Refusal::new(
            "node_unreachable",
            format!("{}: {error}", crate::module_view::NODE_UNREACHABLE),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Replies, held_exchange, runtime};
    use super::*;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn a_hub_ask_names_one_plain_channel_or_is_malformed() {
        assert_eq!(channel_of(br#"{"channel":"eng"}"#).unwrap(), "eng");
        for payload in [
            &b"{}"[..],
            br#"{"channel":""}"#,
            br#"{"channel":"a b"}"#,
            br#"{"channel":"eng&token=x"}"#,
            b"not json",
        ] {
            let refusal = channel_of(payload).unwrap_err();
            assert_eq!(refusal.reason, "malformed_request", "{payload:?}");
        }
    }

    /// A fake hub on a loopback port: `/v1/status` answers the node key the
    /// signature binds to, and `/v1/call/ws?channel=eng` is admitted only
    /// under the seated key's headers. It sends one text and one binary
    /// frame, reads one frame back, then reports how the socket ended.
    struct Hub {
        rpc: String,
        served: tokio::task::JoinHandle<(String, bool, Message, bool)>,
    }

    impl Hub {
        // the handshake callback's `Err` is tungstenite's own response type.
        #[allow(clippy::result_large_err)]
        fn start() -> Self {
            use futures::{SinkExt as _, StreamExt as _};
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
            let rpc = format!("http://{}", listener.local_addr().expect("its address"));
            listener.set_nonblocking(true).expect("nonblocking");
            let served = runtime().spawn(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                loop {
                    let (mut stream, _) = listener.accept().await.expect("a connection");
                    let mut head = [0u8; 4096];
                    let peeked = stream.peek(&mut head).await.expect("a request head");
                    let asked = String::from_utf8_lossy(&head[..peeked]).to_ascii_lowercase();
                    if !asked.contains("upgrade: websocket") {
                        let mut sink = [0u8; 4096];
                        let _ = stream.read(&mut sink).await;
                        let body = serde_json::json!({ "height": 0, "public_key": "00".repeat(32) }).to_string();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        continue;
                    }
                    let mut path = String::new();
                    let mut signed = false;
                    let mut socket = tokio_tungstenite::accept_hdr_async(
                        stream,
                        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                         response| {
                            path = request.uri().to_string();
                            signed = ["x-ducktape-key", "x-ducktape-ts", "x-ducktape-sig"]
                                .iter()
                                .all(|name| request.headers().contains_key(*name));
                            Ok(response)
                        },
                    )
                    .await
                    .expect("a ws upgrade");
                    socket
                        .send(Message::Text(r#"{"type":"peer_beacon","peer":"a"}"#.into()))
                        .await
                        .unwrap();
                    socket.send(Message::Binary(vec![1, 2, 3])).await.unwrap();
                    let heard = socket.next().await.expect("a frame").expect("readable");
                    // the subscription's end reaches the hub as its socket ending
                    let ended = loop {
                        match socket.next().await {
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break true,
                            Some(Ok(_)) => continue,
                        }
                    };
                    return (path, signed, heard, ended);
                }
            });
            Hub { rpc, served }
        }
    }

    /// The hub's socket, opened under the seated key, yields the host's
    /// `ready` first, then the hub's frames in order and in the view's
    /// encoding, carries one `net.send` frame up, and ends at the hub when
    /// the subscription's task is dropped.
    #[test]
    fn the_hub_socket_carries_frames_both_ways_and_ends_with_the_subscription() {
        let _turn = crate::module_view::tests::blocking_connection_turn();
        let hub = Hub::start();
        let client = crate::backend::rpc_client(&hub.rpc).expect("a client");
        runtime().block_on(crate::backend::seat_test_signer(2216));
        let replies = std::sync::Arc::new(Replies::default());
        let running = replies.clone();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let task = runtime().spawn(async move {
            let open = |client: ducktape_rpc::Client| -> super::super::Opening {
                Box::pin(async move { open_hub(&client, "eng").await })
            };
            held_exchange(&running, 7, Some(READY), client, open, &mut receiver).await
        });
        runtime()
            .block_on(sender.send(Message::Text(r#"{"type":"recipients","peers":[]}"#.into())))
            .unwrap();
        let mut changed = replies.changes();
        let mut landed = Vec::new();
        while landed.len() < 3 {
            runtime()
                .block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(30), changed.changed())
                        .await
                })
                .expect("the hub's items land")
                .expect("the replies outlive the wait");
            replies.drain_into(&mut landed).expect("reply budget");
        }
        let items: Vec<&[u8]> = landed
            .iter()
            .map(|item| match item {
                wire::Event::Response {
                    id: 7,
                    result: Ok(bytes),
                    done: false,
                } => bytes.as_slice(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            items,
            [
                &br#"{"text":"{\"type\":\"ready\"}"}"#[..],
                br#"{"text":"{\"type\":\"peer_beacon\",\"peer\":\"a\"}"}"#,
                br#"{"binary":[1,2,3]}"#,
            ]
        );
        // the view drops the subscription: its task goes, and the socket with it
        task.abort();
        let (path, signed, heard, ended) = runtime()
            .block_on(hub.served)
            .expect("the hub saw the whole exchange");
        assert_eq!(path, "/v1/call/ws?channel=eng");
        assert!(signed, "the upgrade carried no seated signature");
        assert_eq!(
            heard,
            Message::Text(r#"{"type":"recipients","peers":[]}"#.into())
        );
        assert!(ended);
        runtime().block_on(crate::backend::lock_signer());
    }

    #[test]
    fn the_hubs_refusals_reach_the_view_under_its_own_tokens() {
        use tokio_tungstenite::tungstenite::Error;
        let http = |status: u16, body: &str| {
            let response = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(status)
                .body(Some(body.as_bytes().to_vec()))
                .unwrap();
            hub_failed(Error::Http(response)).reason
        };
        assert_eq!(http(503, "calls are not available"), "no_call_hub");
        assert_eq!(
            http(403, r#"{"error":"no","reason":"not_in_huddle"}"#),
            "not_in_huddle"
        );
        assert_eq!(
            http(403, r#"{"error":"no","reason":"key_without_account"}"#),
            "key_without_account"
        );
        assert_eq!(
            hub_failed(Error::ConnectionClosed).reason,
            "node_unreachable"
        );
    }
}
