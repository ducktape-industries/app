//! The one-shot loopback listener the browser's page answers to: an
//! ephemeral 127.0.0.1 port, a secret callback path, one POST of
//! `result=<JSON>`. With it, the form codec that POST is written in.
//! `ax/http.rs` has a sync twin of [`read_request`].

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::auth_page::{Outcome, parse_result};

/// How long one local connection has to send its request: a client that
/// connects and says nothing (a browser's spare preconnect, any local
/// process) is dropped, and the next connection is heard.
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The longest request or header line read: the page's POST carries a few
/// short headers.
const MAX_LINE: u64 = 8 * 1024;

/// One-shot, on an ephemeral 127.0.0.1 port. The port is no secret, so the
/// callback path carries 32 random bytes and only a POST to that path ends
/// the wait.
pub(super) struct Listener {
    listener: TcpListener,
    path: String,
}

impl Listener {
    pub(super) async fn bind() -> std::io::Result<Listener> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let path = format!("/cb/{}", B64.encode(rand::random::<[u8; 32]>()));
        Ok(Listener { listener, path })
    }

    pub(super) fn callback_url(&self) -> String {
        let port = self
            .listener
            .local_addr()
            .map(|addr| addr.port())
            .unwrap_or_default();
        format!("http://127.0.0.1:{port}{}", self.path)
    }

    pub(super) async fn wait(self) -> Result<Outcome, String> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|error| format!("Lost the browser's connection: {error}"))?;
            let read = tokio::time::timeout(READ_DEADLINE, read_request(&mut stream)).await;
            let Ok(Ok((method, path, body))) = read else {
                continue;
            };
            let result = (path == self.path && method == "POST")
                .then(|| form_field(&body, "result"))
                .flatten();
            let Some(result) = result else {
                respond(&mut stream, "404 Not Found", "Not found.").await;
                continue;
            };
            let outcome = parse_result(&result);
            let said = match outcome {
                Ok(_) => "Done. You can return to Ducktape.",
                Err(_) => "That didn't finish. Ducktape has the details.",
            };
            respond(&mut stream, "200 OK", said).await;
            return outcome;
        }
    }
}

pub(super) async fn read_request(
    stream: &mut TcpStream,
) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    read_line(&mut reader, &mut line).await?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0usize;
    loop {
        line.clear();
        if read_line(&mut reader, &mut line).await? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    // the page's answer is a few KiB; refuse to buffer anything absurd
    let mut body = vec![0; length.min(64 * 1024)];
    reader.read_exact(&mut body).await?;
    Ok((method, path, body))
}

/// One line onto `line`, refused past [`MAX_LINE`] bytes.
async fn read_line(
    reader: &mut BufReader<&mut TcpStream>,
    line: &mut String,
) -> std::io::Result<usize> {
    let read = reader.take(MAX_LINE).read_line(line).await?;
    if read as u64 == MAX_LINE && !line.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "a request line past the bound",
        ));
    }
    Ok(read)
}

async fn respond(stream: &mut TcpStream, status: &str, said: &str) {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Ducktape</title>\
         <body style=\"font:16px system-ui;display:grid;place-items:center;min-height:90vh\">\
         <p>{said}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
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

pub(super) fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(
            |byte| match byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                true => (byte as char).to_string(),
                false => format!("%{byte:02X}"),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyscheme::testkit;

    #[tokio::test]
    async fn the_listener_takes_one_post_on_its_path() {
        let listener = Listener::bind().await.unwrap();
        let callback = listener.callback_url();
        let waiting = tokio::spawn(listener.wait());
        let http = reqwest::Client::new();
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
        assert_eq!(answer.status(), 200);
        assert_eq!(waiting.await.unwrap(), Ok(Outcome::Created(key)));
    }

    /// A local client that connects first and says nothing (a browser's
    /// spare preconnect, any process on the box) is dropped at its read
    /// deadline: the page's POST behind it is answered, not left waiting for
    /// the ceremony's five minutes.
    #[tokio::test]
    async fn a_stalled_local_client_does_not_hold_up_the_answer() {
        let listener = Listener::bind().await.unwrap();
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
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let key = testkit::passkey_pubkey(&testkit::passkey(3));
        let result = format!(r#"{{"op":"create","publicKey":"{}"}}"#, B64.encode(&key));
        let answer = reqwest::Client::new()
            .post(&callback)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("result={}", url_encode(&result)))
            .send();
        let within = READ_DEADLINE + std::time::Duration::from_secs(3);
        let answer = tokio::time::timeout(within, answer)
            .await
            .expect("the POST waits behind the stalled client")
            .unwrap();
        assert_eq!(answer.status(), 200);
        assert_eq!(waiting.await.unwrap(), Ok(Outcome::Created(key)));
    }

    /// A request line with no end in sight is refused at the bound, not
    /// grown until the deadline.
    #[tokio::test]
    async fn a_request_line_past_the_bound_is_refused() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(&[b'a'; 2 * MAX_LINE as usize])
            .await
            .unwrap();
        let (mut stream, _) = listener.accept().await.unwrap();
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(2), read_request(&mut stream))
                .await
                .expect("still reading the line");
        assert_eq!(read.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

    /// `Session`'s real status source reaches a node over the wire: one
    /// GET of `/v1/status`, its borsh answer decoded. A node that answers
    /// otherwise is a failure with a sentence, not a status.
    #[tokio::test]
    async fn the_status_source_reads_a_node_over_loopback() {
        use crate::backend::noded::{NODE_CONTRACT, Status};
        use crate::shell::entities::Session;
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
