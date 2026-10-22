//! The node's transport, one half per target, each serving `Client` the same calls: in a process
//! reqwest and tokio-tungstenite, in a browser the page's fetch and WebSocket.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub(crate) use native::{Answer, Net};

#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
pub(crate) use web::{Answer, Net};

pub(crate) type Duplex = (
    futures::channel::mpsc::UnboundedSender<Vec<u8>>,
    rt::BoxStream<'static, crate::Result<Vec<u8>>>,
);
