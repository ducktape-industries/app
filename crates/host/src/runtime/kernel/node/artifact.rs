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
    let chain = chain.to_string();
    let mut on = Vec::new();
    for network in crate::runtime::networks() {
        let connection = network.connection.lock().expect("views rpc").clone();
        let Some(client) = connection.client.filter(|_| !network.followed) else {
            continue;
        };
        if connection.chain == chain {
            return Ok(Node {
                client,
                network: connection.network,
                on: connection.on,
            });
        }
        on.push(connection.chain);
    }
    match on.is_empty() {
        true => Err(said(502, "the app is on no network".into())),
        false => Err(said(
            404,
            format!("the app is on {}, not {chain}", on.join(", ")),
        )),
    }
}

async fn ask(node: &Node, query: files::Query) -> Result<files::Reply, view_wire::Error> {
    let answer = crate::backend::signed_query(
        &node.client,
        crate::backend::Layer::Confirmed,
        &*node.signer(),
        &node.network,
        files::PROGRAM,
        abi::encode(&query),
    )
    .await?;
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
    let joined = node.clone();
    let join = move || {
        Box::pin(futures::stream::unfold(
            changes_feed(&joined.client, files::PROGRAM),
            |(feed, slot)| async move {
                let _ = slot.take().await;
                Some(((), (feed, slot)))
            },
        ))
    };
    let stat = move || {
        let (node, path) = (node.clone(), path.clone());
        async move { file_at(&node, &path).await }
    };
    let task =
        crate::runtime::handle().spawn(watched(super::super::in_front(), join, stat, changed));
    Some((Following(task), heard))
}

async fn watched<Writes, Stat>(
    mut front: tokio::sync::watch::Receiver<bool>,
    mut join: impl FnMut() -> Writes,
    mut stat: impl FnMut() -> Stat,
    changed: mpsc::UnboundedSender<()>,
) where
    Writes: futures::Stream<Item = ()> + Unpin,
    Stat: std::future::Future<Output = Option<abi::BlobId>>,
{
    use futures::StreamExt as _;
    let mut shown = stat().await;
    let mut back = false;
    loop {
        while !*front.borrow() {
            back = true;
            if front.changed().await.is_err() {
                return;
            }
        }
        let mut writes = join();
        loop {
            if !std::mem::take(&mut back) {
                tokio::select! {
                    written = writes.next() => if written.is_none() {
                        return;
                    },
                    moved = front.changed() => match moved {
                        Ok(()) if !*front.borrow() => break,
                        Ok(()) => continue,
                        Err(_) => return,
                    },
                }
            }
            let now = stat().await;
            if now != shown {
                shown = now;
                if changed.unbounded_send(()).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;
    use std::sync::atomic::{AtomicU8, Ordering};

    #[tokio::test]
    async fn a_file_written_in_front_or_while_away_reloads_its_page() {
        let (front, in_front) = tokio::sync::watch::channel(true);
        let version = std::sync::Arc::new(AtomicU8::new(1));
        let (joins, mut joined) = mpsc::unbounded();
        let join = move || {
            let (write, mut writes) = tokio::sync::mpsc::unbounded_channel::<()>();
            joins
                .unbounded_send(write)
                .expect("the test hears each join");
            Box::pin(futures::stream::poll_fn(move |cx| writes.poll_recv(cx)))
        };
        let stat = {
            let version = version.clone();
            move || {
                let now = version.load(Ordering::SeqCst);
                async move { Some(abi::BlobId::Sha256([now; 32])) }
            }
        };
        let (changed, mut heard) = mpsc::unbounded();
        let watching = tokio::spawn(watched(in_front, join, stat, changed));
        let first = joined.next().await.expect("it joins the feed in front");
        version.store(2, Ordering::SeqCst);
        first.send(()).expect("the follower holds the feed");
        assert_eq!(heard.next().await, Some(()), "a write in front reloads it");
        front.send(false).expect("the follower watches the front");
        first.closed().await;
        version.store(3, Ordering::SeqCst);
        front.send(true).expect("the follower watches the front");
        drop(joined.next().await.expect("it joins again in front"));
        assert_eq!(
            heard.next().await,
            Some(()),
            "a file written while away reloads it on coming back"
        );
        assert_eq!(heard.next().await, None);
        watching.await.expect("the follower ends");
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;

    #[test]
    fn an_address_is_read_on_the_network_its_chain_names() {
        let chain = ducklink::ChainId::of("artifact-tests", &[3; 32]).expect("a chain id");
        let network = crate::runtime::Network::new(false);
        let client = crate::backend::rpc_client("http://127.0.0.1:9");
        network.take(&client, "artifact-tests", &chain.to_string());
        let Ok(node) = on_network(&chain) else {
            panic!("an address on a network the app holds was refused");
        };
        assert_eq!(node.network, "artifact-tests");
    }
}
