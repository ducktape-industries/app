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

pub struct Following(rt::task::Task);

impl Drop for Following {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Source {
    File(String),
    Member { zip_path: String, member: String },
    Blob(abi::BlobId),
}

type Archive = zip::ZipArchive<std::io::Cursor<std::sync::Arc<[u8]>>>;

#[derive(Default)]
struct Shown {
    pages: usize,
    unpacked: Option<(abi::BlobId, Archive)>,
}

type ShownKey = (String, String);

type ShownPages = std::collections::HashMap<ShownKey, Shown>;

static SHOWN: std::sync::LazyLock<std::sync::Mutex<ShownPages>> =
    std::sync::LazyLock::new(Default::default);

fn shown() -> std::sync::MutexGuard<'static, ShownPages> {
    SHOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct Showing(ShownKey);

impl Drop for Showing {
    fn drop(&mut self) {
        let mut shown = shown();
        let last = shown.get_mut(&self.0).is_some_and(|entry| {
            entry.pages -= 1;
            entry.pages == 0
        });
        if last {
            shown.remove(&self.0);
        }
    }
}

fn showing(key: ShownKey) -> Showing {
    shown().entry(key.clone()).or_default().pages += 1;
    Showing(key)
}

pub fn hold(address: &str) -> Option<Showing> {
    let (chain, Source::Member { zip_path, .. }) = source(address)? else {
        return None;
    };
    Some(showing((chain.to_string(), zip_path)))
}

fn unpacked(key: &ShownKey, file: &abi::BlobId) -> Option<Archive> {
    let shown = shown();
    let (at, archive) = shown.get(key)?.unpacked.as_ref()?;
    (at == file).then(|| archive.clone())
}

fn keep(key: &ShownKey, file: abi::BlobId, archive: &Archive) {
    if let Some(entry) = shown().get_mut(key) {
        entry.unpacked = Some((file, archive.clone()));
    }
}

fn source(address: &str) -> Option<(ducklink::ChainId, Source)> {
    let address = address.split(['?', '#']).next().unwrap_or_default();
    let folder = address.strip_suffix('/');
    let link = ducklink::Link::parse(folder.unwrap_or(address)).ok()?;
    let read = reads_as(&roster_on(&link.chain), &link.program);
    let source = match (read, link.tail.as_slice()) {
        (Some(ArtifactKind::File), tail) if !tail.is_empty() => in_files(tail, folder.is_some())?,
        (Some(ArtifactKind::Blob), [hex]) if folder.is_none() => {
            let digest = noded_client::hex::hex_decode(hex).ok()?;
            Source::Blob(abi::BlobId::Sha256(digest.try_into().ok()?))
        }
        _ => return None,
    };
    Some((link.chain, source))
}

fn in_files(tail: &[String], folder: bool) -> Option<Source> {
    let path = |segments: &[String]| format!("/{}", segments.join("/"));
    let zipped = tail
        .iter()
        .position(|segment| segment.to_ascii_lowercase().ends_with(".zip"));
    let Some(at) = zipped else {
        return (!folder).then(|| Source::File(path(tail)));
    };
    let (zip_path, inside) = tail.split_at(at + 1);
    let named = inside.join("/");
    let member = match (folder, inside.is_empty()) {
        (false, true) => return Some(Source::File(path(tail))),
        (false, false) => named,
        (true, true) => "index.html".to_owned(),
        (true, false) => format!("{named}/index.html"),
    };
    Some(Source::Member {
        zip_path: path(zip_path),
        member,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactKind {
    File,
    Blob,
}

fn roster_on(chain: &ducklink::ChainId) -> crate::runtime::Roster {
    let chain = chain.to_string();
    crate::runtime::networks()
        .into_iter()
        .find(|network| network.connection.lock().expect("views rpc").chain == chain)
        .map(|network| network.roster.clone())
        .unwrap_or_default()
}

fn reads_as(roster: &crate::runtime::Roster, program: &str) -> Option<ArtifactKind> {
    let program = roster.current(program);
    let blobs = roster
        .roles()
        .and_then(|roles| roles.bound(abi::role::BLOBS).ok().cloned())
        .unwrap_or_else(|| "blobs".to_owned());
    let files = program == roster.current(files::PROGRAM);
    let blobs = program == roster.current(&blobs);
    match (files, blobs) {
        (true, _) => Some(ArtifactKind::File),
        (_, true) => Some(ArtifactKind::Blob),
        _ => None,
    }
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
        &node.roster().current(files::PROGRAM),
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

async fn whole<Asked>(
    ask: &impl Fn(files::Query) -> Asked,
    path: &str,
    at: Option<abi::BlobId>,
) -> Result<Vec<u8>, Read>
where
    Asked: std::future::Future<Output = Result<files::Reply, view_wire::Error>>,
{
    let query = files::Query::Read {
        path: path.to_owned(),
        at,
        offset: 0,
        len: u64::MAX,
    };
    match ask(query).await {
        Ok(files::Reply::Contents(contents)) => Ok(contents.bytes),
        Ok(_) => Err(said(502, "files answered a read with another reply".into())),
        Err(refusal) => Err(failed(refusal)),
    }
}

async fn file_read<Asked>(ask: &impl Fn(files::Query) -> Asked, path: &str) -> Read
where
    Asked: std::future::Future<Output = Result<files::Reply, view_wire::Error>>,
{
    match whole(ask, path, None).await {
        Ok(body) => Read {
            status: 200,
            mime: mime(path),
            body,
        },
        Err(read) => read,
    }
}

async fn member_read<Asked>(
    ask: &impl Fn(files::Query) -> Asked,
    key: ShownKey,
    member: &str,
    most: u64,
) -> Read
where
    Asked: std::future::Future<Output = Result<files::Reply, view_wire::Error>>,
{
    let zip_path = key.1.clone();
    let stat = files::Query::Stat {
        path: zip_path.clone(),
        at: None,
    };
    let (snapshot, entry) = match ask(stat).await {
        Ok(files::Reply::Stat { snapshot, entry }) => (snapshot, entry),
        Ok(_) => return said(502, "files answered a stat with another reply".into()),
        Err(refusal) => return failed(refusal),
    };
    let file = match entry.map(|entry| entry.node) {
        Some(files::Node::File { file, .. }) => file,
        Some(_) => return file_read(ask, &format!("{zip_path}/{member}")).await,
        None => return said(404, format!("no file is at {zip_path}")),
    };
    let archive = match unpacked(&key, &file) {
        Some(archive) => archive,
        None => {
            let unpacking = match whole(ask, &zip_path, snapshot).await {
                Ok(bytes) => unpack(bytes, &zip_path),
                Err(read) => return read,
            };
            match unpacking {
                Ok(archive) => {
                    keep(&key, file, &archive);
                    archive
                }
                Err(read) => return read,
            }
        }
    };
    let (zip, named) = (zip_path.clone(), member.to_owned());
    match rt::task::blocking(move || member_of(archive, &zip, &named, most)).await {
        Ok(read) => read,
        Err(_) => said(502, format!("{zip_path}: its unpacking stopped")),
    }
}

fn unpack(bytes: Vec<u8>, zip_path: &str) -> Result<Archive, Read> {
    let bytes: std::sync::Arc<[u8]> = bytes.into();
    zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| said(404, format!("{zip_path} is no zip archive: {error}")))
}

fn member_of(mut archive: Archive, zip_path: &str, member: &str, most: u64) -> Read {
    use std::io::Read as _;
    let entry = match archive.by_name(member) {
        Ok(entry) if !entry.is_dir() => entry,
        Ok(_) | Err(zip::result::ZipError::FileNotFound) => {
            return said(404, format!("{zip_path} holds no {member}"));
        }
        Err(error) => return said(502, format!("{zip_path} does not read: {error}")),
    };
    let declared = entry.size();
    if declared > most {
        return said(
            413,
            format!(
                "{zip_path}/{member} unpacks to {declared} bytes, past the {most} a view may hold here"
            ),
        );
    }
    let mut body = Vec::new();
    let unpacked = entry
        .take(declared.saturating_add(1))
        .read_to_end(&mut body);
    let past_its_size = body.len() as u64 > declared;
    match unpacked {
        Ok(_) if past_its_size => said(
            502,
            format!("{zip_path}/{member} unpacks past the {declared} bytes it declares"),
        ),
        Ok(_) => Read {
            status: 200,
            mime: mime(member),
            body,
        },
        Err(error) => said(502, format!("{zip_path}/{member} does not unpack: {error}")),
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
    let node = &node;
    let asking = move |query: files::Query| ask(node, query);
    match source {
        Source::File(path) => file_read(&asking, &path).await,
        Source::Member { zip_path, member } => {
            let most = crate::backend::memory_bounds().view_bytes;
            member_read(&asking, (chain.to_string(), zip_path), &member, most).await
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
    rt::task::spawn(async move { answer(read(address).await) });
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
    let (chain, source) = source(address)?;
    let path = match source {
        Source::File(path) => path,
        Source::Member { zip_path, .. } => zip_path,
        Source::Blob(_) => return None,
    };
    let node = on_network(&chain).ok()?;
    let (changed, heard) = mpsc::unbounded();
    let joined = node.clone();
    let files = node.roster().current(files::PROGRAM);
    let join = move || {
        Box::pin(futures::stream::unfold(
            changes_feed(&joined.client, &files),
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
    let task = rt::task::spawn(watched(super::super::in_front(), join, stat, changed));
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

    #[tokio::test]
    async fn a_removed_file_reloads_its_page() {
        let (_front, in_front) = tokio::sync::watch::channel(true);
        let there = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (joins, mut joined) = mpsc::unbounded();
        let join = move || {
            let (write, mut writes) = tokio::sync::mpsc::unbounded_channel::<()>();
            joins
                .unbounded_send(write)
                .expect("the test hears each join");
            Box::pin(futures::stream::poll_fn(move |cx| writes.poll_recv(cx)))
        };
        let stat = {
            let there = there.clone();
            move || {
                let file = there
                    .load(Ordering::SeqCst)
                    .then_some(abi::BlobId::Sha256([1; 32]));
                async move { file }
            }
        };
        let (changed, mut heard) = mpsc::unbounded();
        let watching = tokio::spawn(watched(in_front, join, stat, changed));
        let feed = joined.next().await.expect("it joins the feed in front");
        there.store(false, Ordering::SeqCst);
        feed.send(()).expect("the follower holds the feed");
        assert_eq!(
            heard.next().await,
            Some(()),
            "a removed file reloads its page, which its read then answers 404"
        );
        drop(feed);
        assert_eq!(heard.next().await, None);
        watching.await.expect("the follower ends");
    }
}

#[cfg(test)]
mod zip_tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::atomic::Ordering;

    fn one_member(body: &[u8]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("index.html", deflated).expect("a member");
        zip.write_all(body).expect("its bytes");
        zip.finish().expect("the archive").into_inner()
    }

    fn declared_as(mut bytes: Vec<u8>, size: u32) -> Vec<u8> {
        for (signature, at) in [
            ([0x50, 0x4b, 0x03, 0x04], 22),
            ([0x50, 0x4b, 0x01, 0x02], 24),
        ] {
            let header = bytes
                .windows(4)
                .position(|window| window == signature)
                .expect("the member's header");
            bytes[header + at..header + at + 4].copy_from_slice(&size.to_le_bytes());
        }
        bytes
    }

    fn served(
        zip: Vec<u8>,
        reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> impl Fn(files::Query) -> std::future::Ready<Result<files::Reply, view_wire::Error>> {
        move |query: files::Query| {
            std::future::ready(match query {
                files::Query::Stat { path, .. } => Ok(files::Reply::Stat {
                    snapshot: Some(abi::BlobId::Sha256([5; 32])),
                    entry: Some(files::EntryInfo {
                        path,
                        node: files::Node::File {
                            file: abi::BlobId::Sha256([6; 32]),
                            size: zip.len() as u64,
                            exec: false,
                        },
                        meta: Default::default(),
                    }),
                }),
                files::Query::Read { .. } => {
                    reads.fetch_add(1, Ordering::SeqCst);
                    Ok(files::Reply::Contents(files::Contents {
                        snapshot: abi::BlobId::Sha256([5; 32]),
                        size: zip.len() as u64,
                        offset: 0,
                        bytes: zip.clone(),
                        eof: true,
                    }))
                }
                query => panic!("unexpected files query: {query:?}"),
            })
        }
    }

    #[test]
    fn a_member_unpacks_to_no_more_than_it_declares_nor_past_what_a_view_may_hold() {
        let page = vec![b'a'; 1 << 16];
        let Ok(honest) = unpack(one_member(&page), "/shared/site.zip") else {
            panic!("the site did not read as a zip");
        };
        let read = member_of(honest.clone(), "/shared/site.zip", "index.html", 1 << 16);
        assert_eq!((read.status, read.body.len()), (200, 1 << 16));
        let read = member_of(honest, "/shared/site.zip", "index.html", (1 << 16) - 1);
        assert_eq!(read.status, 413, "past what a view may hold here");
        let Ok(bomb) = unpack(declared_as(one_member(&page), 16), "/shared/bomb.zip") else {
            panic!("the bomb did not read as a zip");
        };
        let read = member_of(bomb, "/shared/bomb.zip", "index.html", u64::MAX);
        assert_eq!(
            (read.status, read.mime),
            (502, "text/plain; charset=utf-8"),
            "a member that unpacks past the size it declares is refused, its bytes unserved"
        );
    }

    #[tokio::test]
    async fn a_shown_zip_is_read_whole_once_for_every_member_it_serves() {
        let key = ("zip-reads".to_owned(), "/shared/site.zip".to_owned());
        let reads = std::sync::Arc::default();
        let ask = served(site(), std::sync::Arc::clone(&reads));
        let _shown = showing(key.clone());
        for member in ["index.html", "assets/app.js", "docs/index.html"] {
            let read = member_read(&ask, key.clone(), member, u64::MAX).await;
            assert_eq!(read.status, 200, "{member}");
        }
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "one whole read serves every member while a page shows the zip"
        );
    }

    #[tokio::test]
    async fn a_removed_zip_or_file_answers_404() {
        let ask = |query: files::Query| {
            std::future::ready(match query {
                files::Query::Stat { .. } => Ok(files::Reply::Stat {
                    snapshot: Some(abi::BlobId::Sha256([5; 32])),
                    entry: None,
                }),
                files::Query::Read { path, .. } => Err(view_wire::Error::new(
                    "not_found",
                    format!("no file is at {path}"),
                )),
                query => panic!("unexpected files query: {query:?}"),
            })
        };
        let key = ("zip-removed".to_owned(), "/shared/site.zip".to_owned());
        assert_eq!(
            member_read(&ask, key, "index.html", u64::MAX).await.status,
            404
        );
        assert_eq!(file_read(&ask, "/shared/plan.html").await.status, 404);
    }

    const SITE: &str = "duck://duckpond-725bcad1/files/shared/site.zip";

    fn site() -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, options, body) in [
            ("index.html", deflated, "<!doctype html><title>site</title>"),
            ("assets/app.js", stored, "document.title = 'ran';"),
            (
                "docs/index.html",
                deflated,
                "<!doctype html><title>docs</title>",
            ),
        ] {
            zip.start_file(name, options).expect("a member");
            zip.write_all(body.as_bytes()).expect("its bytes");
        }
        zip.add_directory("empty/", stored).expect("a folder");
        zip.finish().expect("the archive").into_inner()
    }

    #[test]
    fn a_link_inside_a_zip_names_its_member_and_a_folder_its_index() {
        let member = |zip_path: &str, member: &str| {
            Some(Source::Member {
                zip_path: zip_path.into(),
                member: member.into(),
            })
        };
        let file = |path: &str| Some(Source::File(path.into()));
        let on = |path: &str| format!("duck://duckpond-725bcad1/files/{path}");
        for (address, read) in [
            (
                format!("{SITE}/index.html"),
                member("/shared/site.zip", "index.html"),
            ),
            (
                format!("{SITE}/assets/app.js?v=2"),
                member("/shared/site.zip", "assets/app.js"),
            ),
            (format!("{SITE}/"), member("/shared/site.zip", "index.html")),
            (
                format!("{SITE}/docs/"),
                member("/shared/site.zip", "docs/index.html"),
            ),
            (
                on("shared/Site.ZIP/index.html"),
                member("/shared/Site.ZIP", "index.html"),
            ),
            (SITE.to_owned(), file("/shared/site.zip")),
            (on("shared/plan.html"), file("/shared/plan.html")),
            (on("shared/"), None),
            (on("shared/plan.html/"), None),
        ] {
            assert_eq!(
                source(&address).map(|(_, source)| source),
                read,
                "{address}"
            );
        }
    }

    #[test]
    fn a_zip_answers_each_member_with_its_type_and_nothing_else() {
        let Ok(archive) = unpack(site(), "/shared/site.zip") else {
            panic!("the site did not read as a zip");
        };
        let read = |member: &str| {
            let read = member_of(archive.clone(), "/shared/site.zip", member, u64::MAX);
            let body = String::from_utf8(read.body).unwrap_or_default();
            (read.status, read.mime, body)
        };
        assert_eq!(
            read("index.html"),
            (
                200,
                "text/html; charset=utf-8",
                "<!doctype html><title>site</title>".to_owned()
            )
        );
        assert_eq!(
            read("assets/app.js"),
            (
                200,
                "text/javascript; charset=utf-8",
                "document.title = 'ran';".to_owned()
            )
        );
        assert_eq!(read("docs/index.html").0, 200);
        for missing in ["missing.html", "empty/", "assets"] {
            assert_eq!(read(missing).0, 404, "{missing}");
        }
        let Err(refused) = unpack(b"<!doctype html>".to_vec(), "/shared/page.zip") else {
            panic!("a page read as a zip");
        };
        assert_eq!(refused.status, 404);
    }

    #[test]
    fn a_zip_stays_unpacked_while_a_page_shows_it_and_only_as_that_file() {
        let key = ("zip-tests".to_owned(), "/shared/site.zip".to_owned());
        let Ok(archive) = unpack(site(), &key.1) else {
            panic!("the site did not read as a zip");
        };
        let first = || abi::BlobId::Sha256([1; 32]);
        let second = || abi::BlobId::Sha256([2; 32]);
        keep(&key, first(), &archive);
        assert!(
            unpacked(&key, &first()).is_none(),
            "nothing is kept while no page shows it"
        );
        let shown = showing(key.clone());
        let again = showing(key.clone());
        keep(&key, first(), &archive);
        assert!(unpacked(&key, &first()).is_some());
        assert!(
            unpacked(&key, &second()).is_none(),
            "a new file is unpacked again"
        );
        drop(shown);
        assert!(
            unpacked(&key, &first()).is_some(),
            "kept while one page still shows it"
        );
        drop(again);
        assert!(
            unpacked(&key, &first()).is_none(),
            "let go with the last page"
        );
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

#[cfg(test)]
mod reads_tests {
    use super::*;

    #[test]
    fn a_link_written_before_a_rename_reads_as_it_did() {
        let roster = crate::runtime::Roster::default();
        assert_eq!(reads_as(&roster, files::PROGRAM), Some(ArtifactKind::File));
        assert_eq!(reads_as(&roster, "blobs"), Some(ArtifactKind::Blob));
        roster.renamed(vec![
            (files::PROGRAM.into(), "owner/files".into()),
            ("blobs".into(), "owner/blobs".into()),
        ]);
        roster.bind(Some(abi::Roles::from_iter([(
            abi::role::BLOBS,
            "owner/blobs",
        )])));
        for (program, kind) in [
            (files::PROGRAM, Some(ArtifactKind::File)),
            ("owner/files", Some(ArtifactKind::File)),
            ("blobs", Some(ArtifactKind::Blob)),
            ("owner/blobs", Some(ArtifactKind::Blob)),
            ("chat", None),
        ] {
            assert_eq!(reads_as(&roster, program), kind, "{program}");
        }
    }
}
