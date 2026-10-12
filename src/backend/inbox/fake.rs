//! A fake inbox for the tests: `inbox_api`'s shapes over a bare TCP
//! listener (the loopback page's request reader) and a tungstenite server
//! socket for `live`. No dependency on the inbox crate. What it was asked
//! is kept for the test to read; what it answers is set by the test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use futures::SinkExt as _;
use inbox_api::{
    Challenge, Changed, Entries, Entry, LoginReply, LoginRequest, Mutes, Push, Read, Status,
};
use tokio::io::AsyncWriteExt as _;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use crate::backend::loopback::read_request;

#[derive(Default)]
struct State {
    no_inbox: bool,
    /// Every connection is dropped unanswered.
    down: bool,
    deny_logins: bool,
    logins: usize,
    challenges: HashMap<String, u64>,
    tokens: Vec<String>,
    last_login: Option<(LoginRequest, [u8; 32], u64)>,
    entries: Vec<Entry>,
    unread: u64,
    mutes: Mutes,
    reads: Vec<Read>,
    hides: usize,
    mute_puts: Vec<Mutes>,
    stopped: Option<inbox_api::Stopped>,
}

pub(crate) struct FakeInbox {
    origin: String,
    state: Arc<Mutex<State>>,
    pushes: broadcast::Sender<Push>,
}

impl FakeInbox {
    pub(crate) async fn start() -> FakeInbox {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let (pushes, _) = broadcast::channel(64);
        let fake = FakeInbox {
            origin,
            state: state.clone(),
            pushes: pushes.clone(),
        };
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (state, pushes) = (state.clone(), pushes.clone());
                tokio::spawn(async move { serve(stream, state, pushes).await });
            }
        });
        fake
    }

    pub(crate) fn origin(&self) -> String {
        self.origin.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn set_no_inbox(&self, no_inbox: bool) {
        self.state().no_inbox = no_inbox;
    }

    /// Down: every connection dropped, the live sockets closed.
    pub(crate) fn set_down(&self, down: bool) {
        self.state().down = down;
        if down {
            let _ = self.pushes.send(Push::Hidden {
                ids: vec![i64::MIN],
            });
        }
    }

    pub(crate) fn deny_logins(&self, deny: bool) {
        self.state().deny_logins = deny;
    }

    pub(crate) fn expire_tokens(&self) {
        self.state().tokens.clear();
    }

    pub(crate) fn logins(&self) -> usize {
        self.state().logins
    }

    pub(crate) fn last_login(&self) -> (LoginRequest, [u8; 32], u64) {
        self.state().last_login.clone().expect("a login")
    }

    pub(crate) fn set_entries(&self, entries: Vec<Entry>) {
        let mut state = self.state();
        state.unread = entries.iter().filter(|entry| !entry.read).count() as u64;
        state.entries = entries;
    }

    pub(crate) fn set_stopped(&self, stopped: Option<inbox_api::Stopped>) {
        self.state().stopped = stopped;
    }

    pub(crate) fn set_mutes(&self, mutes: Mutes) {
        self.state().mutes = mutes;
    }

    pub(crate) fn reads(&self) -> Vec<Read> {
        self.state().reads.clone()
    }

    pub(crate) fn hides(&self) -> usize {
        self.state().hides
    }

    pub(crate) fn mute_puts(&self) -> Vec<Mutes> {
        self.state().mute_puts.clone()
    }

    /// To every live socket.
    pub(crate) fn push(&self, push: Push) {
        let _ = self.pushes.send(push);
    }

    /// A new entry, pushed, counted unread and listed.
    pub(crate) fn arrive(&self, entry: Entry) {
        {
            let mut state = self.state();
            state.unread += u64::from(!entry.read);
            state.entries.insert(0, entry.clone());
        }
        self.push(Push::Entry {
            entry: Box::new(entry),
        });
    }
}

async fn serve(mut stream: TcpStream, state: Arc<Mutex<State>>, pushes: broadcast::Sender<Push>) {
    if state.lock().unwrap().down {
        return;
    }
    let mut head = [0u8; 2048];
    let peeked = stream.peek(&mut head).await.unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..peeked]).into_owned();
    if head.starts_with("GET /inbox/v1/live") {
        live(stream, head, state, pushes).await;
        return;
    }
    let Ok((method, path, body)) = read_request(&mut stream).await else {
        return;
    };
    let (status, body) = answer(&method, &path, &head, &body, &state, &pushes);
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.write_all(&body).await;
}

fn bearer(head: &str) -> Option<String> {
    head.lines()
        .find_map(|line| {
            line.strip_prefix("authorization: Bearer ")
                .or_else(|| line.strip_prefix("Authorization: Bearer "))
        })
        .map(|token| token.trim().to_owned())
}

fn json<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn answer(
    method: &str,
    path: &str,
    head: &str,
    body: &[u8],
    state: &Mutex<State>,
    pushes: &broadcast::Sender<Push>,
) -> (&'static str, Vec<u8>) {
    let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let call = path.strip_prefix("/inbox/v1/").unwrap_or("");
    if state.no_inbox || call.is_empty() {
        return ("404 Not Found", Vec::new());
    }
    match (method, call) {
        ("GET", "status") => (
            "200 OK",
            json(&Status {
                chain: "net#0".into(),
                indexed_height: 9,
                tip: 10,
                stopped: state.stopped.clone(),
                stalled: None,
            }),
        ),
        ("POST", "challenge") => {
            let nonce: [u8; 32] = rand::random();
            let nonce = abi::hex(&nonce);
            state.challenges.insert(nonce.clone(), 5_000);
            (
                "200 OK",
                json(&Challenge {
                    nonce,
                    expires: 5_000,
                }),
            )
        }
        ("POST", "login") => {
            state.logins += 1;
            let request: LoginRequest = serde_json::from_slice(body).unwrap();
            // stateless on the real inbox: the login hands the expiry back
            if state.challenges.remove(&request.nonce).is_none() {
                return (
                    "401 Unauthorized",
                    b"this challenge is unknown or used".to_vec(),
                );
            }
            let expires = request.expires;
            if state.deny_logins {
                return ("401 Unauthorized", b"this key acts as no account".to_vec());
            }
            let nonce: [u8; 32] = abi::unhex(&request.nonce).unwrap().try_into().unwrap();
            state.last_login = Some((request, nonce, expires));
            let token = abi::hex(&rand::random::<[u8; 32]>());
            state.tokens.push(token.clone());
            ("200 OK", json(&LoginReply { token, account: 7 }))
        }
        _ => {
            let token = bearer(head);
            if !token.is_some_and(|token| state.tokens.contains(&token)) {
                return ("401 Unauthorized", b"sign in first".to_vec());
            }
            match (method, call) {
                ("GET", "entries") => {
                    let before: Option<i64> = query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("before="))
                        .and_then(|before| before.parse().ok());
                    let entries = state
                        .entries
                        .iter()
                        .filter(|entry| before.is_none_or(|before| entry.id < before))
                        .cloned()
                        .collect();
                    let unread = state.unread;
                    ("200 OK", json(&Entries { entries, unread }))
                }
                ("POST", "read") => {
                    let read: Read = serde_json::from_slice(body).unwrap();
                    state.reads.push(read);
                    ("200 OK", json(&Changed { ids: Vec::new() }))
                }
                ("POST", "hide") => {
                    state.hides += 1;
                    ("200 OK", json(&Changed { ids: Vec::new() }))
                }
                ("GET", "prefs") => ("200 OK", json(&state.mutes)),
                ("PUT", "prefs") => {
                    let mut mutes: Mutes = serde_json::from_slice(body).unwrap();
                    state.mute_puts.push(mutes.clone());
                    if mutes.programs.len() > inbox_api::MAX_MUTES {
                        return (
                            "400 Bad Request",
                            format!("a mutes list holds at most {} names", inbox_api::MAX_MUTES)
                                .into_bytes(),
                        );
                    }
                    mutes.programs.sort();
                    mutes.programs.dedup();
                    state.mutes = mutes.clone();
                    let _ = pushes.send(Push::Prefs {
                        mutes: mutes.clone(),
                    });
                    ("200 OK", json(&mutes))
                }
                _ => ("404 Not Found", Vec::new()),
            }
        }
    }
}

async fn live(
    stream: TcpStream,
    head: String,
    state: Arc<Mutex<State>>,
    pushes: broadcast::Sender<Push>,
) {
    let authorized =
        bearer(&head).is_some_and(|token| state.lock().unwrap().tokens.contains(&token));
    // the handshake's refusal is a boxed response: clippy's large-Err rule
    #[allow(clippy::result_large_err)]
    fn gate(
        authorized: bool,
    ) -> impl FnOnce(
        &tokio_tungstenite::tungstenite::handshake::server::Request,
        tokio_tungstenite::tungstenite::handshake::server::Response,
    ) -> Result<
        tokio_tungstenite::tungstenite::handshake::server::Response,
        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
    > {
        move |_, response| match authorized {
            true => Ok(response),
            false => {
                let mut denied =
                    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(Some(
                        "sign in first".into(),
                    ));
                *denied.status_mut() =
                    tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                Err(denied)
            }
        }
    }
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, gate(authorized)).await else {
        return;
    };
    let mut pushes = pushes.subscribe();
    while let Ok(push) = pushes.recv().await {
        // `set_down` closes every socket with this sentinel
        if push
            == (Push::Hidden {
                ids: vec![i64::MIN],
            })
        {
            break;
        }
        let text = serde_json::to_string(&push).unwrap();
        if socket
            .send(tokio_tungstenite::tungstenite::Message::Text(text))
            .await
            .is_err()
        {
            break;
        }
    }
    let _ = socket.close(None).await;
}
