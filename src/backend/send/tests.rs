use super::*;
use commonware_codec::DecodeExt as _;
use commonware_cryptography::{Signer as _, Verifier as _, ed25519};
use noded_client::wire::{
    FRAME_NAMESPACE, Frame, Located, Preconfirm, Submit, Submitted, Tip, Tx, route,
};
use std::io::{BufRead as _, Read as _, Write as _};
use std::sync::mpsc;

const TIP: Tip = Tip {
    height: 7,
    id: [3; 32],
};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

fn recent(url: &str, network: &str, founded: u64) -> RecentEndpoint {
    RecentEndpoint {
        url: url.into(),
        network: network.into(),
        founded,
        other_chain: false,
        ..Default::default()
    }
}

fn status(network: &str, time: u64) -> Status {
    Status {
        network: network.into(),
        time,
        block_time_ms: 1_000,
        epoch_length: 100,
        height: TIP.height,
        tip: TIP.id,
        root: abi::Root([0; 32]),
        epoch: 0,
        identity: Vec::new(),
        contract: NODE_CONTRACT,
        genesis: [0; 32],
        roles: crate::backend::test_roles(),
    }
}

fn receipt(outcome: abi::Outcome) -> Receipt {
    Receipt {
        program: "identity".into(),
        outcome,
        events: Vec::new(),
        nested: Vec::new(),
    }
}

fn located(height: u64, outcome: abi::Outcome) -> Option<Located> {
    Some(Located {
        height,
        position: 0,
        time: 0,
        tx: Tx {
            hash: [0; 32],
            signer: Vec::new(),
            anchor: TIP,
            target: "identity".into(),
            size: 0,
            payload: None,
            receipt: Some(receipt(outcome)),
        },
    })
}

type Answer = Box<dyn Fn(&str, &[u8]) -> (&'static str, Vec<u8>) + Send + Sync>;

fn fake_node(answer: Answer) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (heard, requests) = mpsc::channel();
    let answer = Arc::new(answer);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (heard, answer) = (heard.clone(), answer.clone());
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let (status, reply) = answer(&path, &body);
                let _ = heard.send((path, body));
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.len()
                )
                .unwrap();
                stream.write_all(&reply).unwrap();
            });
        }
    });
    (url, requests)
}

fn node_running(
    network: &'static str,
    time: u64,
    preconfirmed: abi::Outcome,
    carried: Option<Located>,
) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
    fake_node(Box::new(move |path, _| match path {
        route::STATUS => ("200 OK", abi::encode(&status(network, time))),
        route::SUBMIT => (
            "200 OK",
            abi::encode(&Submitted::Preconfirmed(receipt(preconfirmed.clone()))),
        ),
        route::TX => ("200 OK", abi::encode(&carried)),
        other => panic!("{other}"),
    }))
}

fn heard(requests: &mpsc::Receiver<(String, Vec<u8>)>, route: &str) -> Vec<Vec<u8>> {
    requests
        .try_iter()
        .filter(|(path, _)| path == route)
        .map(|(_, body)| body)
        .collect()
}

#[test]
fn three_arguments_are_the_network_the_program_and_the_calls_bytes() {
    let ask = Ask::parse(&args(&["duckpond", "identity", "0b15ff"])).unwrap();
    assert_eq!(
        ask,
        Ask {
            network: "duckpond".into(),
            program: "identity".into(),
            payload: vec![0x0b, 0x15, 0xff],
        }
    );
    let empty = Ask::parse(&args(&["duckpond", "identity", ""])).unwrap();
    assert!(empty.payload.is_empty());
}

#[test]
fn any_other_count_is_the_usage_before_anything_starts() {
    for wrong in [
        args(&[]),
        args(&["duckpond", "identity"]),
        args(&["duckpond", "identity", "00", "extra"]),
    ] {
        assert_eq!(Ask::parse(&wrong), Err(Wrong::Count));
        let code = cli(&wrong, || panic!("nothing starts for wrong arguments"));
        assert_eq!(code, 2);
    }
}

#[test]
fn bytes_that_are_not_hex_are_refused_before_anything_starts() {
    for bad in ["zz", "abc", "0x00"] {
        let wrong = args(&["duckpond", "identity", bad]);
        assert!(matches!(Ask::parse(&wrong), Err(Wrong::Hex(_))), "{bad}");
        let code = cli(&wrong, || panic!("nothing starts for wrong arguments"));
        assert_eq!(code, 2);
    }
}

#[test]
fn the_bound_networks_are_one_per_chain_at_the_node_last_used_for_it() {
    let list = [
        recent("http://new", "duckpond", 30),
        recent("http://other", "testkit", 5),
        recent("http://older", "duckpond", 30),
        recent("http://old", "duckpond", 10),
    ];
    assert_eq!(
        bound(&list),
        [
            Bound {
                network: "duckpond".into(),
                founded: 30,
                url: "http://new".into(),
            },
            Bound {
                network: "testkit".into(),
                founded: 5,
                url: "http://other".into(),
            },
            Bound {
                network: "duckpond".into(),
                founded: 10,
                url: "http://old".into(),
            },
        ]
    );
}

#[test]
fn a_bare_name_reaches_its_latest_node_and_a_full_name_its_own_chain() {
    let chains = bound(&[
        recent("http://new", "duckpond", 30),
        recent("http://old", "duckpond", 10),
    ]);
    assert_eq!(
        target(&chains, "duckpond"),
        Some(Target {
            network: "duckpond".into(),
            founded: None,
            url: "http://new".into(),
        })
    );
    assert_eq!(
        target(&chains, "duckpond+10"),
        Some(Target {
            network: "duckpond".into(),
            founded: Some(10),
            url: "http://old".into(),
        })
    );
    assert_eq!(target(&chains, "duckpond+11"), None);
    assert_eq!(target(&chains, "testkit"), None);
}

#[test]
fn an_unknown_network_is_refused_with_the_bound_ones_listed() {
    crate::backend::note_endpoint(recent("http://127.0.0.1:1", "duckpond", 30));
    crate::backend::note_endpoint(recent("http://127.0.0.1:2", "testkit", 5));
    let ask = Ask::parse(&args(&["elsewhere", "identity", "00"])).unwrap();
    let failure = send(&ask).unwrap_err();
    assert!(matches!(&failure, Failure::Unbound(asked) if asked == "elsewhere"));
    assert_eq!(failure.exit_code(), 2);
    assert!(failure.names_the_bound());
    let listed = BoundList(bound(&crate::backend::recent_endpoints())).to_string();
    for chain in [
        "testkit+5  http://127.0.0.1:2",
        "duckpond+30  http://127.0.0.1:1",
    ] {
        assert!(listed.lines().any(|line| line.trim() == chain), "{listed}");
    }
}

#[test]
fn a_node_that_does_not_answer_is_named_with_the_bound_ones() {
    crate::backend::note_endpoint(recent("http://127.0.0.1:1", "duckpond", 30));
    let ask = Ask::parse(&args(&["duckpond", "identity", "00"])).unwrap();
    let failure = send(&ask).unwrap_err();
    assert!(
        matches!(&failure, Failure::Unreached { url, .. } if url == "http://127.0.0.1:1"),
        "{failure:?}"
    );
    assert!(failure.names_the_bound());
}

#[test]
fn a_bare_name_whose_node_runs_a_chain_this_device_has_no_key_for_mints_none() {
    let remotes = &crate::backend::dirs().remotes;
    noded_client::keyring::bind(remotes, "test-network", 1).unwrap();
    let (url, _) = node_running(
        "test-network",
        2,
        abi::Outcome::Applied { output: Vec::new() },
        None,
    );
    crate::backend::note_endpoint(recent(&url, "test-network", 1));
    let ask = Ask::parse(&args(&["test-network", "identity", "00"])).unwrap();
    let failure = send(&ask).unwrap_err();
    assert!(
        matches!(&failure, Failure::NoKey { chain, .. } if chain == "test-network+2"),
        "{failure:?}"
    );
    assert!(failure.names_the_bound());
    assert!(!remotes.join("test-network+2").join("device.key").exists());
}

#[test]
fn a_full_name_whose_node_now_runs_another_chain_is_refused() {
    let (url, requests) = node_running(
        "test-network",
        2,
        abi::Outcome::Applied { output: Vec::new() },
        None,
    );
    crate::backend::note_endpoint(recent(&url, "test-network", 1));
    let ask = Ask::parse(&args(&["test-network+1", "identity", "00"])).unwrap();
    let failure = send(&ask).unwrap_err();
    assert!(
        matches!(&failure, Failure::Elsewhere { runs, .. } if runs == "test-network+2"),
        "{failure:?}"
    );
    assert!(heard(&requests, route::SUBMIT).is_empty());
}

#[test]
fn a_send_signs_one_frame_with_the_bound_key_and_reads_its_block() {
    let _seat = crate::backend::seat_serial();
    let key = ed25519::PrivateKey::from_seed(41);
    crate::backend::device_key::save("test-network", &key).unwrap();
    let output = vec![4, 2];
    let (url, requests) = node_running(
        "test-network",
        1,
        abi::Outcome::Applied {
            output: output.clone(),
        },
        located(
            9,
            abi::Outcome::Applied {
                output: output.clone(),
            },
        ),
    );
    crate::backend::note_endpoint(recent(&url, "test-network", 1));
    let call = vec![0x0b, 0x15, 0, 0, 0, 0, 0, 0, 0];
    let ask = Ask {
        network: "test-network".into(),
        program: "identity".into(),
        payload: call.clone(),
    };
    let sent = send(&ask).unwrap();
    let submits = heard(&requests, route::SUBMIT);
    let [submit] = submits.as_slice() else {
        panic!("one frame is sent, not {}", submits.len());
    };
    let submit: Submit = abi::decode(submit).unwrap();
    assert_eq!(submit.preconfirm, Preconfirm::On);
    let frame: Frame = abi::decode(&submit.frame).unwrap();
    assert_eq!(frame.body.signer, key.public_key().as_ref());
    assert_eq!(frame.body.network, b"test-network");
    assert_eq!(frame.body.anchor, TIP);
    assert_eq!(frame.body.target, "identity");
    assert_eq!(frame.body.payload, call);
    let proof = ed25519::Signature::decode(frame.proof.as_slice()).unwrap();
    assert!(
        key.public_key()
            .verify(FRAME_NAMESPACE, &frame.body.preimage(), &proof)
    );
    let asked: Vec<TxAsk> = heard(&requests, route::TX)
        .iter()
        .map(|body| abi::decode(body).unwrap())
        .collect();
    assert!(
        asked
            .iter()
            .all(|ask| ask.tx == TxRef::Hash(frame.body.id()))
    );
    assert_eq!(
        sent,
        Sent {
            frame: frame.body.id(),
            height: Some(9),
            outcome: abi::Outcome::Applied { output },
        }
    );
    assert_eq!(sent.exit_code(), 0);
}

#[test]
fn a_frame_refused_at_preconfirm_is_its_refusal_and_waits_for_no_block() {
    let _seat = crate::backend::seat_serial();
    let key = ed25519::PrivateKey::from_seed(42);
    crate::backend::device_key::save("test-network", &key).unwrap();
    let refusal = abi::Refusal::new("unauthorized", "this key manages no such account");
    let (url, requests) = node_running(
        "test-network",
        1,
        abi::Outcome::Rejected(refusal.clone()),
        None,
    );
    crate::backend::note_endpoint(recent(&url, "test-network", 1));
    let ask = Ask::parse(&args(&["test-network", "identity", "00"])).unwrap();
    let sent = send(&ask).unwrap();
    assert_eq!(sent.height, None);
    assert_eq!(sent.outcome, abi::Outcome::Rejected(refusal));
    assert_eq!(sent.exit_code(), 1);
    assert!(heard(&requests, route::TX).is_empty());
}
