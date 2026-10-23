//! `socket.open` and `socket.send`: a WebSocket from this device to a host the view's
//! manifest lists, opened once the person said yes to its hosts.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

use super::node::{Items, spawn_host_subscription, spawn_reply};
use super::*;
use crate::runtime::consent;

type Outgoing = (Vec<u8>, tokio::sync::oneshot::Sender<Answer>);
type Said = tokio::sync::mpsc::UnboundedReceiver<Outgoing>;

#[derive(Default)]
pub(in crate::runtime) struct Sockets(HashMap<u64, tokio::sync::mpsc::UnboundedSender<Outgoing>>);

impl Sockets {
    fn open(&mut self, socket: u64) -> Option<Said> {
        self.0.retain(|_, said| !said.is_closed());
        if self.0.contains_key(&socket) {
            return None;
        }
        let (said, outgoing) = tokio::sync::mpsc::unbounded_channel();
        self.0.insert(socket, said);
        Some(outgoing)
    }

    fn said(&self, socket: u64) -> Option<tokio::sync::mpsc::UnboundedSender<Outgoing>> {
        self.0
            .get(&socket)
            .filter(|said| !said.is_closed())
            .cloned()
    }
}

pub(super) fn open(view: &mut View, id: u64, payload: &[u8]) {
    let ask = match methods::decode::<methods::SocketOpen>(payload) {
        Ok(ask) => ask,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    if let Err(refused) = listed(&ask.url, &view.hosts) {
        return view.reply(id, Err(refused));
    }
    if view.connection.chain.is_empty() {
        return view.refuse(id, refusal::NOT_CONNECTED, "not connected to a network");
    }
    let Some(outgoing) = view.sockets.open(ask.socket) else {
        let open = format!("this view has socket {} open already", ask.socket);
        return view.refuse(id, refusal::MALFORMED_REQUEST, open);
    };
    let asking = Asking {
        kept: approvals(&view.connection.chain, view.program),
        hosts: view.hosts.clone(),
        pressed: view.take_activation(),
        waiting: consent::waiting(),
        program: view.program,
        instance: view.instance,
    };
    spawn_host_subscription(view, id, move |mut items| async move {
        if let Err(refused) = asking.yes().await {
            return items.last(refused);
        }
        let request = match request(&ask) {
            Ok(request) => request,
            Err(refused) => return items.last(refused),
        };
        match tokio_tungstenite::connect_async(request).await {
            Ok((socket, response)) => {
                let protocol = response
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|chosen| chosen.to_str().ok())
                    .map(str::to_owned);
                if let Some(last) = relay(socket, &mut items, protocol, outgoing).await {
                    items.end(last);
                }
            }
            Err(error) => items.last(host_fault(error)),
        }
    });
}

pub(super) fn send(view: &mut View, id: u64, payload: &[u8]) {
    let ask = match methods::decode::<methods::SocketSend>(payload) {
        Ok(ask) => ask,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    let (tell, told) = tokio::sync::oneshot::channel();
    let handed = view
        .sockets
        .said(ask.socket)
        .is_some_and(|said| said.send((ask.body, tell)).is_ok());
    if !handed {
        let closed = format!("this view has no socket {} open", ask.socket);
        return view.refuse(id, refusal::MALFORMED_REQUEST, closed);
    }
    spawn_reply(view, id, async move {
        told.await
            .unwrap_or_else(|_| Err(host_fault("the socket closed before it took the message")))
    });
}

fn listed(url: &str, hosts: &[String]) -> Result<(), wire::Error> {
    let reaches = url::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "wss"
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.port().is_none()
            && parsed
                .host_str()
                .is_some_and(|host| hosts.iter().any(|listed| listed == host))
    });
    match reaches {
        true => Ok(()),
        false => Err(wire::Error::new(
            refusal::UNDECLARED_HOST,
            format!("`socket.open` reaches a wss:// url on a host the manifest lists, not {url}"),
        )),
    }
}

fn request(
    ask: &methods::SocketOpen,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, wire::Error> {
    let mut request = ask.url.as_str().into_client_request().map_err(malformed)?;
    if !ask.protocols.is_empty() {
        let offered = HeaderValue::from_str(&ask.protocols.join(", ")).map_err(malformed)?;
        request
            .headers_mut()
            .insert("sec-websocket-protocol", offered);
    }
    Ok(request)
}

fn approvals(chain: &str, program: &str) -> PathBuf {
    let sockets = crate::backend::dirs().config.join("sockets");
    crate::runtime::store::path(&sockets, chain, program)
}

fn approved(path: &Path) -> Vec<String> {
    crate::backend::read_or_set_aside(path, methods::decode::<Vec<String>>)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn approve(path: &Path, hosts: &[String]) -> std::io::Result<()> {
    crate::contract::os()
        .disk()
        .write(path, &methods::encode(&hosts.to_vec()), false)
}

struct Asking {
    kept: PathBuf,
    hosts: Vec<String>,
    pressed: bool,
    waiting: &'static consent::Queue,
    program: &'static str,
    instance: u64,
}

impl Asking {
    async fn yes(self) -> Result<(), wire::Error> {
        let Asking {
            kept,
            hosts,
            pressed,
            waiting,
            program,
            instance,
        } = self;
        let read = kept.clone();
        let allowed = rt::task::blocking(move || approved(&read))
            .await
            .unwrap_or_default();
        if !asks(&allowed, &hosts) {
            return Ok(());
        }
        if !pressed {
            return Err(wire::Error::new(
                refusal::NEEDS_GESTURE,
                "a program's first `socket.open` to these hosts asks the person, and needs a press or key",
            ));
        }
        let refused = |message: &str| wire::Error::new(refusal::CONSENT_REFUSED, message);
        let words = consent::connect(program, &hosts);
        let Some(mut told) = consent::queue_in(waiting, program, instance, words) else {
            return Err(refused(
                "another confirmation for this view is still waiting",
            ));
        };
        consent::moved();
        if told.answer().await != Some(true) {
            return Err(refused("the person did not allow the connection"));
        }
        rt::task::blocking(move || approve(&kept, &hosts))
            .await
            .map_err(host_fault)?
            .map_err(host_fault)
    }
}

fn asks(allowed: &[String], hosts: &[String]) -> bool {
    let mut allowed = allowed.to_vec();
    let mut hosts = hosts.to_vec();
    allowed.sort_unstable();
    allowed.dedup();
    hosts.sort_unstable();
    hosts.dedup();
    allowed != hosts
}

trait Out {
    fn item(&mut self, item: Answer) -> impl Future<Output = bool> + Send + '_;
}

impl Out for Items {
    fn item(&mut self, item: Answer) -> impl Future<Output = bool> + Send + '_ {
        self.send(item)
    }
}

fn said(item: &methods::SocketItem) -> Answer {
    Ok(methods::encode(item))
}

fn closed(frame: Option<CloseFrame<'_>>) -> methods::SocketItem {
    match frame {
        Some(frame) => methods::SocketItem::Closed {
            code: frame.code.into(),
            reason: frame.reason.into_owned(),
        },
        None => methods::SocketItem::Closed {
            code: 1005,
            reason: String::new(),
        },
    }
}

async fn relay<S, O>(
    socket: WebSocketStream<S>,
    out: &mut O,
    protocol: Option<String>,
    mut outgoing: Said,
) -> Option<Answer>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
    O: Out,
{
    if !out
        .item(said(&methods::SocketItem::Opened { protocol }))
        .await
    {
        return None;
    }
    let (mut sink, mut stream) = socket.split();
    let heard = async {
        loop {
            let item = match stream.next().await {
                Some(Ok(Message::Binary(body))) => methods::SocketItem::Binary(body),
                Some(Ok(Message::Text(text))) => methods::SocketItem::Text(text),
                Some(Ok(Message::Close(frame))) => return Some(said(&closed(frame))),
                Some(Ok(_)) => continue,
                Some(Err(error)) => return Some(Err(host_fault(error))),
                None => {
                    return Some(said(&methods::SocketItem::Closed {
                        code: 1006,
                        reason: String::new(),
                    }));
                }
            };
            if !out.item(said(&item)).await {
                return None;
            }
        }
    };
    let sent = async {
        while let Some((body, tell)) = outgoing.recv().await {
            let sent = sink.send(Message::Binary(body)).await;
            let _ = tell.send(sent.map(|()| methods::encode(&())).map_err(host_fault));
        }
        std::future::pending::<Option<Answer>>().await
    };
    tokio::select! {
        last = heard => last,
        last = sent => last,
    }
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    use super::*;

    struct Heard(tokio::sync::mpsc::UnboundedSender<Answer>);

    impl Out for Heard {
        fn item(&mut self, item: Answer) -> impl Future<Output = bool> + Send + '_ {
            let kept = self.0.send(item).is_ok();
            async move { kept }
        }
    }

    fn item(answer: Option<Answer>) -> methods::SocketItem {
        methods::decode(&answer.expect("an item").expect("not a refusal")).unwrap()
    }

    #[test]
    fn a_socket_reaches_only_a_listed_host_over_wss() {
        let hosts = vec!["venus.web.telegram.org".to_owned()];
        assert!(listed("wss://venus.web.telegram.org/apiws", &hosts).is_ok());
        assert!(listed("wss://VENUS.web.telegram.org/apiws", &hosts).is_ok());
        for refused in [
            "ws://venus.web.telegram.org/apiws",
            "https://venus.web.telegram.org/apiws",
            "wss://flora.web.telegram.org/apiws",
            "wss://venus.web.telegram.org:8443/apiws",
            "wss://someone@venus.web.telegram.org/apiws",
            "wss://venus.web.telegram.org.example/apiws",
            "venus.web.telegram.org",
            "",
        ] {
            let refusal = listed(refused, &hosts).unwrap_err();
            assert_eq!(refusal.code, refusal::UNDECLARED_HOST, "{refused}");
        }
        assert!(listed("wss://venus.web.telegram.org/apiws", &[]).is_err());
    }

    #[test]
    fn a_socket_is_open_once_and_again_once_it_closed() {
        let mut sockets = Sockets::default();
        let first = sockets.open(1).expect("opens");
        assert!(sockets.open(1).is_none(), "open twice");
        assert!(sockets.said(1).is_some());
        assert!(sockets.said(2).is_none());
        drop(first);
        assert!(sockets.said(1).is_none(), "a closed socket takes nothing");
        assert!(sockets.open(1).is_some(), "opens again once closed");
    }

    #[test]
    fn the_hosts_allowed_are_kept_on_the_device() {
        let folder = tempfile::tempdir().unwrap();
        let kept = crate::runtime::store::path(folder.path(), "duckpond#1", "telegram");
        assert!(approved(&kept).is_empty());
        let hosts = vec![
            "venus.web.telegram.org".to_owned(),
            "flora.web.telegram.org".to_owned(),
        ];
        approve(&kept, &hosts).unwrap();
        assert_eq!(approved(&kept), hosts);
        let other = crate::runtime::store::path(folder.path(), "duckpond#1", "chat");
        assert!(approved(&other).is_empty());
    }

    #[test]
    fn a_changed_host_list_asks_again_and_the_old_yes_does_not_carry() {
        let hosts = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        let allowed = hosts(&["venus.web.telegram.org", "flora.web.telegram.org"]);
        assert!(!asks(
            &allowed,
            &hosts(&["flora.web.telegram.org", "venus.web.telegram.org"])
        ));
        assert!(asks(
            &allowed,
            &hosts(&[
                "venus.web.telegram.org",
                "flora.web.telegram.org",
                "pluto.web.telegram.org"
            ])
        ));
        assert!(asks(&allowed, &hosts(&["venus.web.telegram.org"])));
        assert!(asks(
            &allowed,
            &hosts(&["venus.web.telegram.org", "evil.example"])
        ));
        assert!(asks(&[], &hosts(&["venus.web.telegram.org"])));
        let folder = tempfile::tempdir().unwrap();
        let kept = crate::runtime::store::path(folder.path(), "duckpond#1", "telegram");
        approve(&kept, &allowed).unwrap();
        let changed = hosts(&["venus.web.telegram.org"]);
        approve(&kept, &changed).unwrap();
        assert_eq!(
            approved(&kept),
            changed,
            "a yes to the new list replaces the old"
        );
    }

    #[test]
    fn the_card_names_the_program_and_every_host() {
        let hosts = vec![
            "venus.web.telegram.org".to_owned(),
            "flora.web.telegram.org".to_owned(),
        ];
        let words = consent::connect("telegram", &hosts);
        let id = words.id.clone().expect("the program's id");
        assert_eq!(&words.said[id], "telegram");
        assert_eq!(
            words.shown.as_deref(),
            Some("venus.web.telegram.org\nflora.web.telegram.org")
        );
        assert!(!words.described);
        assert!(
            words
                .card
                .facts
                .iter()
                .any(|fact| fact.value.contains("flora"))
        );
    }

    #[tokio::test]
    async fn a_socket_carries_both_ways_and_ends_with_the_servers_close() {
        let (near, far) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(async move {
            let mut socket = tokio_tungstenite::accept_async(far).await.unwrap();
            socket.send(Message::Binary(vec![1, 2])).await.unwrap();
            let heard = socket.next().await.unwrap().unwrap();
            socket.send(Message::Text("ok".into())).await.unwrap();
            let bye = CloseFrame {
                code: CloseCode::Normal,
                reason: "bye".into(),
            };
            socket.close(Some(bye)).await.unwrap();
            heard
        });
        let (socket, _) =
            tokio_tungstenite::client_async("ws://venus.web.telegram.org/apiws", near)
                .await
                .unwrap();
        let (said, outgoing) = tokio::sync::mpsc::unbounded_channel();
        let (items, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let relayed = tokio::spawn(async move {
            relay(socket, &mut Heard(items), Some("binary".into()), outgoing).await
        });
        assert_eq!(
            item(heard.recv().await),
            methods::SocketItem::Opened {
                protocol: Some("binary".into())
            }
        );
        assert_eq!(
            item(heard.recv().await),
            methods::SocketItem::Binary(vec![1, 2])
        );
        let (tell, told) = tokio::sync::oneshot::channel();
        said.send((vec![9, 8], tell)).unwrap();
        assert_eq!(told.await.unwrap(), Ok(methods::encode(&())));
        assert_eq!(server.await.unwrap(), Message::Binary(vec![9, 8]));
        assert_eq!(
            item(heard.recv().await),
            methods::SocketItem::Text("ok".into())
        );
        assert_eq!(
            item(relayed.await.unwrap()),
            methods::SocketItem::Closed {
                code: 1000,
                reason: "bye".into()
            }
        );
    }
}
