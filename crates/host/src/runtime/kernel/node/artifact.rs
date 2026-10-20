//! Artifacts on this host: what a page's `duck://` address answers (`read`),
//! read-only from the connected node, and the follower that tells a page its
//! file changed (`follow`).
use super::*;
use futures::channel::mpsc;

pub struct Read {
    pub status: u16,
    pub mime: &'static str,
    pub body: Vec<u8>,
}

pub struct Following(tokio::task::JoinHandle<()>);

impl Drop for Following {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum Source {
    File(String),
    Blob(abi::BlobId),
}

fn source(address: &str) -> Option<(ducklink::ChainId, Source)> {
    let address = address.split(['?', '#']).next().unwrap_or_default();
    let link = ducklink::Link::parse(address).ok()?;
    let source = match (link.program.as_str(), link.tail.as_slice()) {
        (files::PROGRAM, tail) if !tail.is_empty() => Source::File(format!("/{}", tail.join("/"))),
        ("blobs", [hex]) => {
            let digest = noded_client::hex::hex_decode(hex).ok()?;
            Source::Blob(abi::BlobId::Sha256(digest.try_into().ok()?))
        }
        _ => return None,
    };
    Some((link.chain, source))
}

pub fn serves(address: &str) -> bool {
    source(address).is_some()
}

fn said(status: u16, words: String) -> Read {
    Read {
        status,
        mime: "text/plain; charset=utf-8",
        body: words.into_bytes(),
    }
}

fn mime(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("wasm") => "application/wasm",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn on_network(chain: &ducklink::ChainId) -> Result<Node, Read> {
    let connection = crate::runtime::connection()
        .lock()
        .expect("views rpc")
        .clone();
    let Some(client) = connection.client else {
        return Err(said(502, "the app is on no network".into()));
    };
    if chain.to_string() != connection.chain {
        return Err(said(
            404,
            format!("the app is on {}, not {chain}", connection.chain),
        ));
    }
    Ok(Node {
        client,
        network: connection.network,
        apart: None,
    })
}

async fn ask(node: &Node, query: files::Query) -> Result<files::Reply, view_wire::Error> {
    let frame = crate::backend::query_frame(
        &*node.signer(),
        &node.network,
        files::PROGRAM,
        abi::encode(&query),
    )
    .await?;
    let answer = node
        .client
        .query(crate::backend::Layer::Preconfirmed, frame)
        .await
        .map_err(crate::backend::refused)?;
    abi::decode(&answer).map_err(|refusal| {
        view_wire::Error::new(view_wire::code::UNEXPECTED_REPLY, refusal.sentence)
    })
}

fn failed(refusal: view_wire::Error) -> Read {
    match transport_failed(&refusal) {
        true => said(502, refusal.message),
        false => said(404, refusal.message),
    }
}

pub async fn read(address: String) -> Read {
    let Some((chain, source)) = source(&address) else {
        return said(404, format!("{address} names no file or blob"));
    };
    let node = match on_network(&chain) {
        Ok(node) => node,
        Err(read) => return read,
    };
    match source {
        Source::File(path) => {
            let query = files::Query::Read {
                path: path.clone(),
                at: None,
                offset: 0,
                len: u64::MAX,
            };
            match ask(&node, query).await {
                Ok(files::Reply::Contents(contents)) => Read {
                    status: 200,
                    mime: mime(&path),
                    body: contents.bytes,
                },
                Ok(_) => said(502, "files answered a read with another reply".into()),
                Err(refusal) => failed(refusal),
            }
        }
        Source::Blob(id) => match node.client.blob(id).await {
            Ok(Some(framed)) => {
                let kind = framed
                    .split(|byte| *byte == b' ')
                    .next()
                    .unwrap_or_default();
                let mime = match kind == b"artifact" {
                    true => "text/html; charset=utf-8",
                    false => "application/octet-stream",
                };
                match noded_client::wire::unframe(&framed) {
                    Some(body) => Read {
                        status: 200,
                        mime,
                        body: body.to_vec(),
                    },
                    None => said(502, "the blob has no header".into()),
                }
            }
            Ok(None) => said(404, format!("the network holds no blob {address}")),
            Err(error) => failed(crate::backend::refused(error)),
        },
    }
}

pub fn spawn_read(address: String, answer: impl FnOnce(Read) + Send + 'static) {
    crate::runtime::handle().spawn(async move { answer(read(address).await) });
}

async fn file_at(node: &Node, path: &str) -> Option<abi::BlobId> {
    let query = files::Query::Stat {
        path: path.to_owned(),
        at: None,
    };
    match ask(node, query).await {
        Ok(files::Reply::Stat {
            entry: Some(entry), ..
        }) => match entry.node {
            files::Node::File { file, .. } => Some(file),
            _ => None,
        },
        _ => None,
    }
}

pub fn follow(address: &str) -> Option<(Following, mpsc::UnboundedReceiver<()>)> {
    let (chain, Source::File(path)) = source(address)? else {
        return None;
    };
    let node = on_network(&chain).ok()?;
    let (changed, heard) = mpsc::unbounded();
    let task = crate::runtime::handle().spawn(async move {
        let mut front = super::super::in_front();
        let mut shown = file_at(&node, &path).await;
        loop {
            while !*front.borrow() {
                if front.changed().await.is_err() {
                    return;
                }
            }
            let (_feed, slot) = feed(&node, files::PROGRAM);
            loop {
                tokio::select! {
                    _ = slot.take() => {}
                    moved = front.changed() => match moved {
                        Ok(()) if !*front.borrow() => break,
                        Ok(()) => continue,
                        Err(_) => return,
                    },
                }
                let now = file_at(&node, &path).await;
                if now != shown {
                    shown = now;
                    if changed.unbounded_send(()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Some((Following(task), heard))
}
