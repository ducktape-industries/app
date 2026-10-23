use super::*;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

pub(crate) fn capture<R>(run: impl FnOnce() -> R) -> (R, String) {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(output.clone())
        .finish();
    let result = tracing::subscriber::with_default(subscriber, run);
    let bytes = output.0.lock().unwrap().clone();
    (result, String::from_utf8(bytes).unwrap())
}

const KEY: Key = Key::View {
    program: "test-view",
    instance: 7,
};

#[test]
fn a_submission_receipt_links_the_request_and_frame_without_its_output() {
    let ((), log) = capture(|| {
        let mut requests = Requests::default();
        requests.request(KEY, 3, 41, "op.submit");
        let reply = methods::Submitted::Applied {
            frame: [9; 32],
            until: noded_client::wire::ANCHOR_WINDOW,
            output: b"private message output".to_vec(),
        };
        requests.reply(41, &Ok(methods::encode(&reply)), true);
        assert!(requests.calls.is_empty());
    });
    assert!(log.contains("preconfirmation"));
    assert!(log.contains("request_id=41"));
    assert!(log.contains("revision=3"));
    assert!(log.contains(&noded_client::hex::hex_encode(&[9; 32])));
    assert!(!log.contains("private message output"));
}

#[test]
fn a_refused_prediction_is_distinct_from_a_lost_submission_answer() {
    let refused = methods::Submitted::Refused {
        frame: [1; 32],
        until: noded_client::wire::ANCHOR_WINDOW,
        refusal: view_wire::Error::new("private reason", "private refusal text"),
    };
    let refused = details(Kind::Submit, &Ok(methods::encode(&refused)));
    assert_eq!(refused.stage, "preconfirmation");
    assert_eq!(refused.outcome, "refused");
    let doubt = methods::Submitted::InDoubt {
        frame: [2; 32],
        until: noded_client::wire::ANCHOR_WINDOW,
    };
    let doubt = details(Kind::Submit, &Ok(methods::encode(&doubt)));
    assert_eq!(doubt.stage, "submission_in_doubt");
    assert_eq!(doubt.outcome, "unknown");
    let ((), log) = capture(|| {
        let mut requests = Requests::default();
        requests.request(KEY, 3, 41, "op.submit");
        let refusal = methods::Submitted::Refused {
            frame: [1; 32],
            until: noded_client::wire::ANCHOR_WINDOW,
            refusal: view_wire::Error::new("private reason", "private refusal text"),
        };
        requests.reply(41, &Ok(methods::encode(&refusal)), true);
    });
    assert!(!log.contains("private reason"));
    assert!(!log.contains("private refusal text"));
}

#[test]
fn streaming_replies_keep_their_request_until_completion_or_cancellation() {
    let mut requests = Requests::default();
    requests.request(KEY, 3, 41, "program.changes");
    let reply = Some(methods::Change {
        height: 19,
        keys: vec![b"private key".to_vec()],
    });
    let result = Ok(methods::encode(&reply));
    assert_eq!(details(Kind::Changes, &result).height, Some(19));
    requests.reply(41, &result, false);
    assert!(requests.calls.contains_key(&41));
    requests.cancel(41);
    assert!(requests.calls.is_empty());
    requests.request(KEY, 4, 41, "chain.heads");
    let reply = methods::Head {
        height: 20,
        time: 1000,
        id: [3; 32],
    };
    let result = Ok(methods::encode(&reply));
    assert_eq!(details(Kind::Heads, &result).height, Some(20));
    requests.reply(41, &result, true);
    assert!(requests.calls.is_empty());
}

#[test]
fn program_replies_are_opaque_and_unknown_method_names_are_not_logged() {
    let ((), log) = capture(|| {
        let mut requests = Requests::default();
        requests.request(KEY, 3, 41, "program.query");
        requests.reply(41, &Ok(b"private message text".to_vec()), true);
        requests.request(KEY, 3, 42, "private unknown method");
        requests.reply(42, &Ok(b"more private text".to_vec()), true);
        assert!(requests.calls.is_empty());
    });
    assert!(log.contains("request_id=41"));
    assert!(!log.contains("private"));
    assert!(!log.contains("request_id=42"));
}

#[test]
fn malformed_or_failed_submissions_are_not_called_preconfirmations() {
    let malformed = details(Kind::Submit, &Ok(vec![]));
    assert_eq!(malformed.stage, "submission_unread");
    assert!(malformed.frame.is_none());
    let failed = details(
        Kind::Submit,
        &Err(view_wire::Error::new("private code", "private failure")),
    );
    assert_eq!(failed.stage, "reply");
    assert_eq!(failed.outcome, "error");
    assert!(failed.frame.is_none());
}

#[test]
fn input_metadata_names_only_the_route() {
    assert_eq!(input_route(&Event::Message(41)), Some(("message", 41)));
    assert_eq!(
        input_route(&Event::Viewport {
            width: 1.,
            height: 1.,
        }),
        None
    );
}
