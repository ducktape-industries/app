//! The browser half against a fake node (`tests/web_node.py`, at `NODED_FAKE` when built), in Chrome
//! through wasm-bindgen-test-runner: `cargo test --target wasm32-unknown-unknown -p noded-client --lib`.

use crate::wire::Submitted;
use crate::{Client, Error, hex::hex_encode, net::Net};
use abi::BlobId;
use futures::StreamExt as _;
use std::time::Duration;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

const NODE: &str = env!("NODED_FAKE");
const ID: BlobId = BlobId::Sha256([0; 32]);

fn answering(bytes: &[u8]) -> Client {
    Client::new(format!("{NODE}/answer/{}", hex_encode(bytes)))
}

#[wasm_bindgen_test]
async fn a_blob_streams_whole_and_a_short_one_fails() {
    let blob = b"program 9\0\0asm\x01\0\0\0!".to_vec();
    let whole = abi::encode(&Some(blob.clone()));
    let mut stream = answering(&whole).blob_stream(ID).await.unwrap().unwrap();
    let mut read = Vec::new();
    while let Some(piece) = stream.next().await.unwrap() {
        read.extend_from_slice(&piece);
    }
    assert_eq!(read, blob);
    let short = answering(&whole[..whole.len() - 2]);
    let mut stream = short.blob_stream(ID).await.unwrap().unwrap();
    let ended = loop {
        match stream.next().await {
            Ok(Some(_)) => continue,
            other => break other,
        }
    };
    assert!(matches!(ended, Err(Error::Decode(_))), "{ended:?}");
    let none = answering(&abi::encode(&None::<Vec<u8>>));
    assert!(none.blob_stream(ID).await.unwrap().is_none());
}

#[wasm_bindgen_test]
async fn a_blobs_answer_reads_each_blob_in_order() {
    let (first, second) = (b"program 3\0abc".to_vec(), b"program 1\0z".to_vec());
    let answer = [
        3u32.to_le_bytes().to_vec(),
        abi::encode(&Some(first.clone())),
        vec![0],
        abi::encode(&Some(second.clone())),
    ]
    .concat();
    let mut blobs = answering(&answer)
        .blobs_stream(&[ID, ID, ID])
        .await
        .unwrap();
    let mut read = Vec::new();
    while let Some(held) = blobs.next_blob().await.unwrap() {
        let mut bytes = Vec::new();
        while let Some(piece) = blobs.piece().await.unwrap() {
            bytes.extend_from_slice(&piece);
        }
        read.push(held.then_some(bytes));
    }
    assert_eq!(read, [Some(first), None, Some(second)]);
}

#[wasm_bindgen_test]
async fn a_read_gives_up_on_a_node_that_says_nothing() {
    let deadlines = |connect, idle| Net::new(Some((connect, idle)));
    let silent = Client {
        reads: deadlines(Duration::from_secs(5), Duration::from_millis(200)),
        ..Client::new(format!("{NODE}/silent"))
    };
    let read = silent.blob(ID).await;
    assert!(matches!(read, Err(Error::Transport(_))), "{read:?}");
    let mute = Client {
        reads: deadlines(Duration::from_millis(100), Duration::from_millis(100)),
        ..Client::new(format!("{NODE}/mute"))
    };
    let read = mute.blob(ID).await;
    assert!(matches!(read, Err(Error::Transport(_))), "{read:?}");
}

#[wasm_bindgen_test]
async fn a_failure_a_redirect_and_no_node_read_as_the_native_half_reads_them() {
    let busy = Client::new(format!("{NODE}/busy")).status().await;
    assert!(
        matches!(&busy, Err(Error::Failed { status: 503, sentence }) if sentence == "busy"),
        "{busy:?}"
    );
    let moved = Client::new(format!("{NODE}/redirect")).status().await;
    assert!(
        matches!(moved, Err(Error::Failed { status: 0, .. })),
        "{moved:?}"
    );
    let nobody = Client::new("http://127.0.0.1:9").status().await;
    assert!(matches!(nobody, Err(Error::Transport(_))), "{nobody:?}");
}

#[wasm_bindgen_test]
async fn an_upload_is_one_request_the_frame_by_its_length_then_each_blob_in_order() {
    let pooled = abi::encode(&Submitted::Pooled);
    let client = Client::new(format!("{NODE}/keep/upload/{}", hex_encode(&pooled)));
    let blobs = [b"chunk 3\0abc".to_vec(), b"chunk 0\0".to_vec()];
    let parts = futures::stream::iter(blobs.clone().map(Ok));
    let submitted = client.put_blobs(vec![7; 300], parts).await.unwrap();
    assert_eq!(submitted, Submitted::Pooled);
    let seen = client.http().get(&format!("{NODE}/seen/upload")).await;
    let seen = seen.unwrap().bytes().await.unwrap();
    let mut sent = 300u32.to_le_bytes().to_vec();
    sent.extend([7; 300]);
    sent.extend(blobs.concat());
    assert_eq!(&seen[..], &sent[..]);
}

#[wasm_bindgen_test]
async fn a_duplex_the_node_has_no_route_for_is_its_404_and_any_other_failure_the_transport() {
    let net = Net::new(None);
    let ws = NODE.replacen("http", "ws", 1);
    let item = hex_encode(b"one");
    let (_sender, mut socket) = net.duplex(&format!("{ws}/socket/{item}")).await.unwrap();
    assert_eq!(socket.next().await.unwrap().unwrap(), b"one");
    assert!(socket.next().await.is_none());
    let none = net.duplex(&format!("{ws}/nosocket")).await.err();
    assert!(
        matches!(none, Some(Error::Failed { status: 404, .. })),
        "{none:?}"
    );
    let busy = net.duplex(&format!("{ws}/busy")).await.err();
    assert!(matches!(busy, Some(Error::Transport(_))), "{busy:?}");
}

#[wasm_bindgen_test]
async fn a_socket_hands_on_each_message_and_ends_when_the_node_closes_it() {
    let net = Net::new(None);
    let ws = NODE.replacen("http", "ws", 1);
    let items = format!("{},{}", hex_encode(b"one"), hex_encode(b"two"));
    let mut socket = net.socket(&format!("{ws}/socket/{items}")).await.unwrap();
    assert_eq!(socket.next().await.unwrap().unwrap(), b"one");
    assert_eq!(socket.next().await.unwrap().unwrap(), b"two");
    assert!(socket.next().await.is_none());
    let refused = net.socket(&format!("{ws}/nosocket")).await;
    assert!(matches!(refused, Err(Error::Transport(_))));
}
