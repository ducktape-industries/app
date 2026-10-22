//! The heads socket kept by pings: a node that answers them keeps it open however quiet the
//! chain, one that goes silent ends it, block time known or not; a status ends at the reads' bound.
use super::*;
use futures::SinkExt as _;

const QUIET: Duration = Duration::from_millis(20);
const IDLE: Duration = Duration::from_secs(2);

fn reading_within(base: &str, connect: Duration, idle: Duration) -> crate::Client {
    crate::Client {
        reads: Net::new(Some((connect, idle))),
        idle,
        ..crate::Client::new(base)
    }
}

async fn listening() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = listener.local_addr().unwrap();
    (listener, format!("ws://{at}/v1/heads"))
}

#[tokio::test]
async fn a_socket_whose_node_answers_its_pings_stays_open_through_a_quiet_chain() {
    let (listener, url) = listening().await;
    let served = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut pings = 0;
        while pings < 3 {
            match socket.next().await {
                Some(Ok(Message::Ping(_))) => pings += 1,
                Some(Ok(_)) => {}
                other => panic!("the client left before its third ping: {other:?}"),
            }
        }
        socket
            .send(Message::Binary(b"head".to_vec()))
            .await
            .unwrap();
        while socket.next().await.is_some() {}
    });
    let mut kept = Net::new(None)
        .socket_kept(&url, "", QUIET, IDLE)
        .await
        .unwrap();
    let first = kept.next().await;
    assert!(
        matches!(&first, Some(Ok(bytes)) if bytes == b"head"),
        "a quiet chain whose node answered every ping lost its socket: {first:?}"
    );
    drop(kept);
    served.await.unwrap();
}

#[tokio::test]
async fn a_socket_whose_node_goes_silent_ends_as_lost() {
    let (listener, url) = listening().await;
    let (hold, held) = futures::channel::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Binary(b"head".to_vec()))
            .await
            .unwrap();
        let _ = held.await;
    });
    let mut kept = Net::new(None)
        .socket_kept(&url, "", QUIET, IDLE)
        .await
        .unwrap();
    assert!(matches!(kept.next().await, Some(Ok(_))), "the first head");
    let lost = kept.next().await;
    assert!(
        matches!(lost, Some(Err(Error::Transport(_)))),
        "a node that answered no ping kept the socket: {lost:?}"
    );
    let _ = hold.send(());
    served.await.unwrap();
}

#[tokio::test]
async fn a_status_the_node_never_answers_ends_at_the_reads_bound() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (hold, held) = futures::channel::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = held.await;
        drop(stream);
    });
    let client = reading_within(&base, IDLE, IDLE);
    let error = client.status().await.err();
    assert!(
        matches!(error, Some(Error::Transport(_))),
        "a status the node never answered was waited on: {error:?}"
    );
    let _ = hold.send(());
    served.await.unwrap();
}

#[tokio::test]
async fn heads_whose_block_time_is_unknown_are_kept_by_the_reads_bound() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (hold, held) = futures::channel::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let head = crate::wire::Head {
            height: 7,
            id: [0; 32],
            time: 0,
        };
        socket
            .send(Message::Binary(abi::encode(&head)))
            .await
            .unwrap();
        let _ = held.await;
    });
    let client = reading_within(&base, IDLE, IDLE);
    let mut heads = client.heads_kept(None).await.unwrap();
    let first = heads.next().await.map(|head| head.map(|head| head.height));
    assert!(matches!(first, Some(Ok(7))), "the first head: {first:?}");
    let lost = heads.next().await.map(|head| head.map(|head| head.height));
    assert!(
        matches!(lost, Some(Err(Error::Transport(_)))),
        "a silent node kept a socket whose block time was unknown: {lost:?}"
    );
    let _ = hold.send(());
    served.await.unwrap();
}

#[tokio::test]
async fn a_changes_stream_whose_node_goes_silent_ends_as_lost() {
    let (listener, _) = listening().await;
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (hold, held) = futures::channel::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = held.await;
    });
    let client = reading_within(&base, IDLE, IDLE);
    let mut changes = client.changes("chat").await.unwrap();
    let lost = changes
        .next()
        .await
        .map(|change| change.map(|change| change.height));
    assert!(
        matches!(lost, Some(Err(Error::Transport(_)))),
        "a silent node kept a changes stream: {lost:?}"
    );
    let _ = hold.send(());
    served.await.unwrap();
}

#[tokio::test]
async fn a_watch_whose_node_goes_silent_ends_as_lost() {
    let (listener, _) = listening().await;
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (hold, held) = futures::channel::oneshot::channel::<()>();
    let served = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = held.await;
    });
    let client = reading_within(&base, IDLE, IDLE);
    let (_watching, mut pushed) = client.watch("chat").await.unwrap();
    let lost = pushed.next().await.map(|pushed| pushed.map(|_| ()));
    assert!(
        matches!(lost, Some(Err(Error::Transport(_)))),
        "a silent node kept a watch: {lost:?}"
    );
    let _ = hold.send(());
    served.await.unwrap();
}
