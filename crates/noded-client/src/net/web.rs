//! The node's transport in a browser: the page's fetch with redirects answered, never followed, each
//! answer's body read as it streams in, and a WebSocket for the changes socket.

use crate::{Error, Result};
use bytes::Bytes;
use futures::channel::{mpsc, oneshot};
use futures::{Stream, StreamExt as _};
use std::cell::{Cell, RefCell};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;

#[derive(Clone)]
pub(crate) struct Net {
    deadlines: Option<(Duration, Duration)>,
}

impl Net {
    pub(crate) fn new(deadlines: Option<(Duration, Duration)>) -> Self {
        Self { deadlines }
    }

    pub(crate) async fn get(&self, url: &str) -> Result<Answer> {
        self.send(url, "GET", None, None, false).await
    }

    pub(crate) async fn post(&self, url: &str, body: Vec<u8>) -> Result<Answer> {
        let body = js_sys::Uint8Array::from(&body[..]).into();
        self.send(url, "POST", Some(body), None, false).await
    }

    pub(crate) async fn post_form(&self, url: &str, form: String) -> Result<Answer> {
        let kind = Some("application/x-www-form-urlencoded");
        self.send(url, "POST", Some(JsValue::from_str(&form)), kind, false)
            .await
    }

    pub(crate) async fn post_stream(
        &self,
        url: &str,
        body: impl Stream<Item = std::io::Result<Vec<u8>>> + 'static,
    ) -> Result<Answer> {
        let streamed = url.starts_with("https://") && streams_requests();
        if streamed {
            let parts = body.map(|part| match part {
                Ok(bytes) => Ok(JsValue::from(js_sys::Uint8Array::from(&bytes[..]))),
                Err(error) => Err(JsValue::from_str(&error.to_string())),
            });
            let stream = wasm_streams::ReadableStream::from_stream(parts).into_raw();
            return self
                .send(url, "POST", Some(stream.into()), None, true)
                .await;
        }
        let blob = folded(body).await?;
        self.send(url, "POST", Some(blob.into()), None, false).await
    }

    pub(crate) async fn socket(
        &self,
        url: &str,
    ) -> Result<rt::BoxStream<'static, Result<Vec<u8>>>> {
        let socket = web_sys::WebSocket::new(url).map_err(unsent)?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let (heard, items) = mpsc::unbounded();
        let (open, opened) = oneshot::channel::<bool>();
        let open = Rc::new(RefCell::new(Some(open)));
        let on_message = Closure::<dyn FnMut(JsValue)>::new({
            let heard = heard.clone();
            move |event: JsValue| {
                let data = event.unchecked_into::<web_sys::MessageEvent>().data();
                if let Some(buffer) = data.dyn_ref::<js_sys::ArrayBuffer>() {
                    let _ = heard.unbounded_send(Ok(js_sys::Uint8Array::new(buffer).to_vec()));
                }
            }
        });
        let on_open = Closure::<dyn FnMut(JsValue)>::new({
            let open = open.clone();
            move |_: JsValue| {
                if let Some(open) = open.borrow_mut().take() {
                    let _ = open.send(true);
                }
            }
        });
        let on_close = Closure::<dyn FnMut(JsValue)>::new({
            let open = open.clone();
            move |event: JsValue| {
                if let Some(open) = open.borrow_mut().take() {
                    let _ = open.send(false);
                    return;
                }
                let close = event.unchecked_into::<web_sys::CloseEvent>();
                if !close.was_clean() {
                    let sentence = format!("the changes socket was cut ({})", close.code());
                    let _ = heard.unbounded_send(Err(Error::Transport(sentence)));
                }
                heard.close_channel();
            }
        });
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        let socket = Socket {
            socket,
            items,
            _handlers: [on_message, on_open, on_close],
        };
        match opened.await {
            Ok(true) => Ok(Box::pin(socket)),
            _ => Err(Error::Transport(
                "the node did not take the changes socket".into(),
            )),
        }
    }

    async fn send(
        &self,
        url: &str,
        method: &str,
        body: Option<JsValue>,
        kind: Option<&str>,
        duplex: bool,
    ) -> Result<Answer> {
        let init = web_sys::RequestInit::new();
        init.set_method(method);
        init.set_redirect(web_sys::RequestRedirect::Manual);
        if let Some(body) = &body {
            init.set_body(body);
        }
        if duplex {
            js_sys::Reflect::set(&init, &"duplex".into(), &"half".into()).map_err(unsent)?;
        }
        let abort = web_sys::AbortController::new().map_err(unsent)?;
        init.set_signal(Some(&abort.signal()));
        let request = web_sys::Request::new_with_str_and_init(url, &init).map_err(unsent)?;
        if let Some(kind) = kind {
            request
                .headers()
                .set("content-type", kind)
                .map_err(unsent)?;
        }
        let window =
            web_sys::window().ok_or_else(|| Error::Transport("no window to fetch from".into()))?;
        let fetched = JsFuture::from(window.fetch_with_request(&request));
        let fetched = match self.deadlines {
            Some((connect, idle)) => match rt::time::timeout(connect + idle, fetched).await {
                Ok(fetched) => fetched,
                Err(_) => {
                    abort.abort();
                    let sentence = format!("the node did not answer within {:?}", connect + idle);
                    return Err(Error::Transport(sentence));
                }
            },
            None => fetched.await,
        };
        let response = fetched
            .map_err(|error| Error::Transport(said(&error)))?
            .unchecked_into::<web_sys::Response>();
        let reader = response.body().map(|body| {
            let body = wasm_streams::ReadableStream::from_raw(body);
            Box::pin(body.into_stream()) as rt::BoxStream<'static, _>
        });
        Ok(Answer {
            response,
            reader,
            abort,
            idle: self.deadlines.map(|(_, idle)| idle),
        })
    }
}

pub(crate) struct Answer {
    response: web_sys::Response,
    reader: Option<rt::BoxStream<'static, std::result::Result<JsValue, JsValue>>>,
    abort: web_sys::AbortController,
    idle: Option<Duration>,
}

impl Answer {
    pub(crate) fn status(&self) -> u16 {
        self.response.status()
    }

    pub(crate) fn said(&self) -> String {
        format!("{} {}", self.response.status(), self.response.status_text())
            .trim_end()
            .to_owned()
    }

    pub(crate) fn plain(&self) -> bool {
        self.response
            .headers()
            .get("content-type")
            .ok()
            .flatten()
            .is_some_and(|value| value.starts_with("text/plain"))
    }

    pub(crate) async fn bytes(mut self) -> Result<Bytes> {
        let mut whole = Vec::new();
        while let Some(piece) = self.chunk().await? {
            whole.extend_from_slice(&piece);
        }
        Ok(whole.into())
    }

    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>> {
        let Some(reader) = &mut self.reader else {
            return Ok(None);
        };
        let next = match self.idle {
            Some(idle) => match rt::time::timeout(idle, reader.next()).await {
                Ok(next) => next,
                Err(_) => {
                    self.abort.abort();
                    let sentence = format!("nothing came from the node for {idle:?}");
                    return Err(Error::Transport(sentence));
                }
            },
            None => reader.next().await,
        };
        match next {
            None => Ok(None),
            Some(Ok(piece)) => Ok(Some(js_sys::Uint8Array::new(&piece).to_vec().into())),
            Some(Err(error)) => Err(Error::Transport(said(&error))),
        }
    }

    pub(crate) fn pieces(self) -> rt::BoxStream<'static, Result<Bytes>> {
        Box::pin(futures::stream::unfold(Some(self), |answer| async move {
            let mut answer = answer?;
            match answer.chunk().await {
                Ok(Some(piece)) => Some((Ok(piece), Some(answer))),
                Ok(None) => None,
                Err(error) => Some((Err(error), None)),
            }
        }))
    }
}

struct Socket {
    socket: web_sys::WebSocket,
    items: mpsc::UnboundedReceiver<Result<Vec<u8>>>,
    _handlers: [Closure<dyn FnMut(JsValue)>; 3],
}

impl Stream for Socket {
    type Item = Result<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.poll_next_unpin(cx)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.socket.set_onmessage(None);
        self.socket.set_onopen(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close();
    }
}

async fn folded(body: impl Stream<Item = std::io::Result<Vec<u8>>>) -> Result<web_sys::Blob> {
    let mut body = std::pin::pin!(body);
    let mut sofar = web_sys::Blob::new().map_err(unsent)?;
    while let Some(part) = body.next().await {
        let part = part.map_err(|error| Error::Transport(error.to_string()))?;
        let parts = js_sys::Array::of2(&sofar, &js_sys::Uint8Array::from(&part[..]));
        sofar = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(unsent)?;
    }
    Ok(sofar)
}

fn streams_requests() -> bool {
    thread_local! {
        static STREAMS: bool = asks_for_duplex();
    }
    STREAMS.with(|streams| *streams)
}

fn asks_for_duplex() -> bool {
    let asked = Rc::new(Cell::new(false));
    let getter = Closure::<dyn FnMut() -> JsValue>::new({
        let asked = asked.clone();
        move || {
            asked.set(true);
            JsValue::from_str("half")
        }
    });
    let init = js_sys::Object::new();
    let descriptor = js_sys::Object::new();
    let built = web_sys::ReadableStream::new().ok().and_then(|stream| {
        js_sys::Reflect::set(&init, &"method".into(), &"POST".into()).ok()?;
        js_sys::Reflect::set(&init, &"body".into(), &stream).ok()?;
        js_sys::Reflect::set(&descriptor, &"get".into(), getter.as_ref()).ok()?;
        js_sys::Object::define_property(&init, &"duplex".into(), &descriptor);
        web_sys::Request::new_with_str_and_init("", init.unchecked_ref()).ok()
    });
    let typed = built.is_none_or(|request| request.headers().has("content-type").unwrap_or(true));
    asked.get() && !typed
}

fn said(error: &JsValue) -> String {
    match error.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}

fn unsent(error: JsValue) -> Error {
    Error::Transport(said(&error))
}

#[cfg(test)]
mod tests;
