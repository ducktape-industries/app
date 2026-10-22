//! The node's transport in a process: reqwest, with no redirect followed (a redirect would carry a
//! signed frame on to another host, so it is the answer), and tokio-tungstenite for the changes socket.

use crate::{Error, Result, refusal};
use bytes::Bytes;
use futures::channel::mpsc;
use futures::{Stream, StreamExt as _};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

#[derive(Clone)]
pub(crate) struct Net(reqwest::Client);

impl Net {
    pub(crate) fn new(deadlines: Option<(Duration, Duration)>) -> Self {
        let builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        let builder = match deadlines {
            Some((connect, idle)) => builder.connect_timeout(connect).read_timeout(idle),
            None => builder,
        };
        Self(builder.build().expect("the HTTP client"))
    }

    pub(crate) async fn get(&self, url: &str) -> Result<Answer> {
        Ok(Answer(self.0.get(url).send().await?))
    }

    pub(crate) async fn post(&self, url: &str, body: Vec<u8>) -> Result<Answer> {
        Ok(Answer(self.0.post(url).body(body).send().await?))
    }

    pub(crate) async fn post_form(&self, url: &str, form: String) -> Result<Answer> {
        let request = self.0.post(url);
        let request = request.header("content-type", "application/x-www-form-urlencoded");
        Ok(Answer(request.body(form).send().await?))
    }

    pub(crate) async fn post_stream(
        &self,
        url: &str,
        body: impl Stream<Item = std::io::Result<Vec<u8>>> + Send + 'static,
    ) -> Result<Answer> {
        let request = self.0.post(url).body(reqwest::Body::wrap_stream(body));
        Ok(Answer(request.send().await?))
    }

    pub(crate) async fn socket(
        &self,
        url: &str,
    ) -> Result<rt::BoxStream<'static, Result<Vec<u8>>>> {
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(url, Some(unbounded()), false)
                .await
                .map_err(not_opened)?;
        Ok(Box::pin(binary(socket)))
    }

    pub(crate) async fn duplex(&self, url: &str) -> Result<super::Duplex> {
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(url, Some(unbounded()), false)
                .await
                .map_err(not_opened)?;
        let (sink, read) = socket.split();
        let (sender, sending) = mpsc::unbounded::<Vec<u8>>();
        let sent = sending.map(Message::Binary).map(Ok).forward(sink);
        let failed = futures::stream::once(sent).filter_map(|sent| {
            let failed = sent.err().map(|error| Error::Transport(error.to_string()));
            futures::future::ready(failed.map(|error| Some(Err(error))))
        });
        let heard = binary(read)
            .map(Some)
            .chain(futures::stream::once(futures::future::ready(None)));
        let socket =
            futures::stream::select(heard, failed).scan((), |_, item| futures::future::ready(item));
        Ok((sender, Box::pin(socket)))
    }
}

pub(crate) struct Answer(reqwest::Response);

impl Answer {
    pub(crate) fn status(&self) -> u16 {
        self.0.status().as_u16()
    }

    pub(crate) fn said(&self) -> String {
        self.0.status().to_string()
    }

    pub(crate) fn plain(&self) -> bool {
        plain(self.0.headers())
    }

    pub(crate) async fn bytes(self) -> Result<Bytes> {
        Ok(self.0.bytes().await?)
    }

    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>> {
        Ok(self.0.chunk().await?)
    }

    pub(crate) fn pieces(self) -> rt::BoxStream<'static, Result<Bytes>> {
        Box::pin(
            self.0
                .bytes_stream()
                .map(|piece| piece.map_err(Error::from)),
        )
    }
}

fn plain(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/plain"))
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        match error.is_connect() {
            true => Error::Unreachable(error.to_string()),
            false => Error::Transport(with_causes(&error)),
        }
    }
}

fn with_causes(error: &reqwest::Error) -> String {
    let mut sentence = error.to_string();
    let mut cause = std::error::Error::source(error);
    while let Some(inner) = cause {
        let said = inner.to_string();
        let new = !said.is_empty() && !sentence.ends_with(&said);
        if new {
            sentence = format!("{sentence}: {said}");
        }
        cause = inner.source();
    }
    sentence
}

fn unbounded() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: None,
        max_frame_size: None,
        ..WebSocketConfig::default()
    }
}

fn binary(
    socket: impl Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> impl Stream<Item = Result<Vec<u8>>> {
    socket.filter_map(|message| {
        futures::future::ready(match message {
            Ok(Message::Binary(bytes)) => Some(Ok(bytes)),
            Ok(_) => None,
            Err(error) => Some(Err(Error::Transport(error.to_string()))),
        })
    })
}

fn not_opened(error: tokio_tungstenite::tungstenite::Error) -> Error {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(answer) => {
            let body = answer.body().as_deref().unwrap_or_default();
            refusal(answer.status().as_u16(), plain(answer.headers()), body)
        }
        error => Error::Transport(error.to_string()),
    }
}
