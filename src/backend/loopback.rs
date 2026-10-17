//! The one-shot loopback listener the browser's page answers to: an
//! ephemeral 127.0.0.1 port, a secret callback path, one POST of
//! `result=<JSON>`. With it, the form codec that POST is written in.
//! `ax/http.rs` has a sync twin of [`read_request`].

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::auth_page::{Outcome, parse_result};

/// One-shot, on an ephemeral 127.0.0.1 port. The port is no secret, so the
/// callback path carries 32 random bytes and only a POST to that path ends
/// the wait. Its answer sends the browser back to `page`'s last screen.
pub(super) struct Listener {
    listener: TcpListener,
    path: String,
    page: String,
}

impl Listener {
    pub(super) async fn bind(page: &str) -> std::io::Result<Listener> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let path = format!("/cb/{}", B64.encode(rand::random::<[u8; 32]>()));
        Ok(Listener {
            listener,
            path,
            page: page.to_owned(),
        })
    }

    pub(super) fn callback_url(&self) -> String {
        let port = self
            .listener
            .local_addr()
            .map(|addr| addr.port())
            .unwrap_or_default();
        format!("http://127.0.0.1:{port}{}", self.path)
    }

    /// The page's answer. Every local connection is read at once, side by
    /// side, so one that connects and says nothing (a browser's spare
    /// preconnect, any local process) holds up no other.
    pub(super) async fn wait(self) -> Result<Outcome, String> {
        let callback: Arc<Callback> = Arc::new(Callback {
            path: self.path,
            page: self.page,
        });
        let mut reading = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted
                        .map_err(|error| format!("Lost the browser's connection: {error}"))?;
                    reading.spawn(callback.clone().answer(stream));
                }
                Some(Ok(Some(outcome))) = reading.join_next() => return outcome,
            }
        }
    }
}

/// What the page's one POST must name, and where its answer sends the
/// browser back to.
struct Callback {
    path: String,
    page: String,
}

impl Callback {
    /// One local connection: the page's outcome, if it is the POST to the
    /// callback path; anything else is answered `404` or dropped.
    async fn answer(self: Arc<Self>, mut stream: TcpStream) -> Option<Result<Outcome, String>> {
        let (method, path, body) = read_request(&mut stream).await.ok()?;
        let result = (path == self.path && method == "POST")
            .then(|| form_field(&body, "result"))
            .flatten();
        let Some(result) = result else {
            reply(&mut stream, "404 Not Found").await;
            return None;
        };
        let outcome = parse_result(&result);
        let screen = match outcome {
            Ok(_) => "done",
            Err(_) => "failed",
        };
        let back = format!("303 See Other\r\nLocation: {}#{screen}", self.page);
        reply(&mut stream, &back).await;
        Some(outcome)
    }
}

pub(super) async fn read_request(
    stream: &mut TcpStream,
) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0u64;
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = Vec::new();
    reader.take(length).read_to_end(&mut body).await?;
    Ok((method, path, body))
}

/// One reply with no body, `status` and its headers; then the connection
/// closes.
async fn reply(stream: &mut TcpStream, head: &str) {
    let reply = format!("HTTP/1.1 {head}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.flush().await;
}

/// One `application/x-www-form-urlencoded` field, decoded.
fn form_field(body: &[u8], name: &str) -> Option<String> {
    body.split(|byte| *byte == b'&').find_map(|pair| {
        let at = pair.iter().position(|byte| *byte == b'=')?;
        (form_decode(&pair[..at]) == name.as_bytes())
            .then(|| String::from_utf8(form_decode(&pair[at + 1..])).ok())
            .flatten()
    })
}

fn form_decode(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        let escaped = (value[i] == b'%')
            .then(|| value.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match (value[i], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                i += 1;
            }
            (byte, None) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyscheme::testkit;
    use noded_client::relay::url_encode;

    #[tokio::test]
    async fn the_listener_takes_one_post_on_its_path() {
        let listener = Listener::bind("https://auth.example/").await.unwrap();
        let callback = listener.callback_url();
        let waiting = tokio::spawn(listener.wait());
        let http = no_redirects();
        let wrong = callback.rsplit_once('/').unwrap().0.to_owned() + "/nope";
        assert_eq!(
            http.post(&wrong).body("").send().await.unwrap().status(),
            404
        );
        let key = testkit::passkey_pubkey(&testkit::passkey(2));
        let result = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode(&key));
        let form = format!("result={}", url_encode(&result));
        let answer = http
            .post(&callback)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 303);
        assert_eq!(answer.headers()["location"], "https://auth.example/#done");
        assert_eq!(waiting.await.unwrap(), Ok(Outcome::Created(key)));
    }

    /// The page's own last screen takes the browser, not the listener.
    fn no_redirects() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// A local client that connects first and says nothing (a browser's
    /// spare preconnect, any process on the box) holds nothing up: the
    /// page's POST behind it is answered while it is still connected.
    #[tokio::test]
    async fn a_stalled_local_client_does_not_hold_up_the_answer() {
        let listener = Listener::bind("https://auth.example/").await.unwrap();
        let callback = listener.callback_url();
        let port = callback
            .split(':')
            .nth(2)
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let waiting = tokio::spawn(listener.wait());
        let _stalled = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let key = testkit::passkey_pubkey(&testkit::passkey(3));
        let result = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode(&key));
        let answer = no_redirects()
            .post(&callback)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("result={}", url_encode(&result)))
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), 303);
        assert_eq!(waiting.await.unwrap(), Ok(Outcome::Created(key)));
    }

    /// `Session`'s real status source reaches a node over the wire: one
    /// GET of `/v1/status`, its borsh answer decoded. A node that answers
    /// otherwise is a failure with a sentence, not a status.
    #[tokio::test]
    async fn the_status_source_reads_a_node_over_loopback() {
        use crate::shell::entities::Session;
        use noded_client::wire::{NODE_CONTRACT, Status};
        let status = Status {
            network: "loopback".into(),
            time: 1,
            block_time_ms: 2,
            epoch_length: 3,
            height: 42,
            tip: [4; 32],
            root: abi::Root([5; 32]),
            epoch: 6,
            identity: vec![7],
            contract: NODE_CONTRACT,
            genesis: [8; 32],
            roles: crate::backend::test_roles(),
        };
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let served = abi::encode(&status);
        let (asked, mut paths) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (method, path, _) = read_request(&mut stream).await.unwrap();
                assert_eq!(method, "GET");
                let (status, body): (&str, &[u8]) = match path.as_str() {
                    "/v1/status" => ("200 OK", &served),
                    _ => ("404 Not Found", b"nothing here"),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
                stream.flush().await.unwrap();
                asked.send(path).unwrap();
            }
        });
        let source = Session::status_source(&origin);
        assert_eq!(source().await, Ok(status));
        assert_eq!(paths.recv().await.unwrap(), "/v1/status");
        // a node gone (nothing listens): a sentence for the screen
        let gone = Session::status_source("http://127.0.0.1:1");
        assert!(gone().await.unwrap_err().contains("error sending request"));
    }
}
