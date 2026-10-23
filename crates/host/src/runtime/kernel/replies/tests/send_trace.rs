use super::*;
use crate::send_trace::tests::capture;

const KEY: crate::perf::Key = crate::perf::Key::View {
    program: "test-view",
    instance: 7,
};

fn receipt() -> Answer {
    Ok(methods::encode(&methods::Submitted::Applied {
        frame: [9; 32],
        until: noded_client::wire::ANCHOR_WINDOW,
        output: b"private output".to_vec(),
    }))
}

#[test]
fn a_preconfirmation_is_traced_when_queued_before_the_view_drains_it() {
    let ((), log) = capture(|| {
        let replies = Replies {
            trace: Some(Mutex::default()),
            ..Default::default()
        };
        replies.trace_request(KEY, 3, 41, "op.submit");
        assert!(replies.answer(&Claim::default(), 41, receipt(), true));
        assert_eq!(replies.events.lock().unwrap().len(), 1);
    });
    assert!(log.contains("preconfirmation"));
    assert!(log.contains("request_id=41"));
    assert!(!log.contains("private output"));
}

#[test]
fn batched_and_unclaimed_answers_have_the_same_capture_boundary() {
    let ((), log) = capture(|| {
        let replies = Replies {
            trace: Some(Mutex::default()),
            ..Default::default()
        };
        replies.trace_request(KEY, 3, 41, "op.submit");
        replies.trace_request(KEY, 3, 42, "op.submit");
        let claim = Claim::default();
        assert_eq!(replies.items([(&claim, 41, receipt(), true)]), [true]);
        replies.item(42, receipt(), true);
    });
    assert!(
        log.lines()
            .any(|line| { line.contains("preconfirmation") && line.contains("request_id=41") })
    );
    assert!(
        log.lines()
            .any(|line| { line.contains("preconfirmation") && line.contains("request_id=42") })
    );
}

#[test]
fn a_disabled_reply_queue_keeps_its_answers_and_emits_no_capture() {
    let ((), log) = capture(|| {
        let replies = Replies {
            trace: None,
            ..Default::default()
        };
        replies.trace_request(KEY, 3, 41, "op.submit");
        replies.item(41, receipt(), true);
        let mut events = Vec::new();
        assert_eq!(replies.drain_into(&mut events), 1);
        assert!(matches!(
            events.as_slice(),
            [wire::Event::Response { id: 41, .. }]
        ));
    });
    assert!(!log.contains("ducktape::send_trace"));
}
