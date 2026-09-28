//! The one-shot loopback listener the browser's page answers to: an
//! ephemeral 127.0.0.1 port, a secret callback path, one POST of
//! `result=<JSON>`. With it, the form codec that POST is written in.
//! `ax/http.rs` has a sync twin of [`read_request`].

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::auth_page::{Outcome, parse_result};

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
            let Ok((method, path, body)) = read_request(&mut stream).await else {
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
    reader.read_line(&mut line).await?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0usize;
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
    // the page's answer is a few KiB; refuse to buffer anything absurd
    let mut body = vec![0; length.min(64 * 1024)];
    reader.read_exact(&mut body).await?;
    Ok((method, path, body))
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
}
