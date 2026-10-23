//! Opt-in timing metadata for view input, host requests and painted nodes.

use crate::perf::Key;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use view_wire::{Event, methods};

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("DUCKTAPE_SEND_TRACE")
            .ok()
            .is_some_and(|value| value.trim() == "1")
    })
}

pub(crate) fn elapsed_us() -> u64 {
    static START: OnceLock<rt::time::Instant> = OnceLock::new();
    START
        .get_or_init(rt::time::Instant::now)
        .elapsed()
        .as_micros() as u64
}

pub(crate) fn input(key: Key, revision: u64, event: &Event) {
    if !enabled() {
        return;
    }
    let Some((kind, handler)) = input_route(event) else {
        return;
    };
    tracing::debug!(
        target: "ducktape::send_trace",
        stage = "input_dispatch",
        elapsed_us = elapsed_us(),
        ?key,
        revision,
        kind,
        handler,
    );
}

fn input_route(event: &Event) -> Option<(&'static str, u32)> {
    match event {
        Event::Message(handler) => Some(("message", *handler)),
        Event::Click { handler, .. } => Some(("click", *handler)),
        Event::KeyDown { handler, .. } => Some(("key_down", *handler)),
        Event::Text { handler, .. } => Some(("text_changed", *handler)),
        Event::AuxClick { handler, .. } => Some(("aux_click", *handler)),
        Event::A11yAction { handler, .. } => Some(("a11y_action", *handler)),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Submit,
    Query,
    Watch,
    Changes,
    Heads,
    Id,
}

impl Kind {
    fn of(kind: &str) -> Option<Self> {
        match kind {
            "op.submit" => Some(Self::Submit),
            "program.query" => Some(Self::Query),
            "program.watch" => Some(Self::Watch),
            "program.changes" => Some(Self::Changes),
            "chain.heads" => Some(Self::Heads),
            "host.id" => Some(Self::Id),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Call {
    key: Key,
    revision: u64,
    kind: Kind,
}

#[derive(Default)]
pub(crate) struct Requests {
    calls: BTreeMap<u64, Call>,
}

impl Requests {
    pub(crate) fn request(&mut self, key: Key, revision: u64, id: u64, kind: &str) {
        let Some(kind) = Kind::of(kind) else {
            return;
        };
        self.calls.insert(
            id,
            Call {
                key,
                revision,
                kind,
            },
        );
        tracing::debug!(
            target: "ducktape::send_trace",
            stage = "request",
            elapsed_us = elapsed_us(),
            ?key,
            revision,
            request_id = id,
            ?kind,
        );
    }

    pub(crate) fn cancel(&mut self, id: u64) {
        let Some(call) = self.calls.remove(&id) else {
            return;
        };
        tracing::debug!(
            target: "ducktape::send_trace",
            stage = "cancel",
            elapsed_us = elapsed_us(),
            key = ?call.key,
            request_id = id,
        );
    }

    pub(crate) fn reply(
        &mut self,
        id: u64,
        result: &Result<Vec<u8>, view_wire::Error>,
        done: bool,
    ) {
        let call = match done {
            true => self.calls.remove(&id),
            false => self.calls.get(&id).copied(),
        };
        let Some(call) = call else {
            return;
        };
        let details = details(call.kind, result);
        tracing::debug!(
            target: "ducktape::send_trace",
            stage = details.stage,
            elapsed_us = elapsed_us(),
            key = ?call.key,
            revision = call.revision,
            request_id = id,
            kind = ?call.kind,
            outcome = details.outcome,
            frame_id = ?details.frame,
            height = ?details.height,
            done,
        );
    }
}

struct Details {
    stage: &'static str,
    outcome: &'static str,
    frame: Option<String>,
    height: Option<u64>,
}

fn details(kind: Kind, result: &Result<Vec<u8>, view_wire::Error>) -> Details {
    let mut details = Details {
        stage: "reply",
        outcome: "ok",
        frame: None,
        height: None,
    };
    let Ok(bytes) = result else {
        details.outcome = "error";
        return details;
    };
    match kind {
        Kind::Submit => submission(bytes),
        Kind::Heads => {
            details.height = methods::decode::<methods::Head>(bytes)
                .ok()
                .map(|head| head.height);
            details
        }
        Kind::Changes => {
            details.height = methods::decode::<Option<methods::Change>>(bytes)
                .ok()
                .flatten()
                .map(|change| change.height);
            details
        }
        Kind::Query | Kind::Watch | Kind::Id => details,
    }
}

fn submission(bytes: &[u8]) -> Details {
    let mut details = Details {
        stage: "submission_unread",
        outcome: "unread",
        frame: None,
        height: None,
    };
    let Ok(submitted) = methods::decode::<methods::Submitted>(bytes) else {
        return details;
    };
    let (stage, outcome, frame) = match submitted {
        methods::Submitted::Applied { frame, .. } => ("preconfirmation", "applied", frame),
        methods::Submitted::Refused { frame, .. } => ("preconfirmation", "refused", frame),
        methods::Submitted::InDoubt { frame, .. } => ("submission_in_doubt", "unknown", frame),
    };
    details.stage = stage;
    details.outcome = outcome;
    details.frame = Some(noded_client::hex::hex_encode(&frame));
    details
}

#[cfg(test)]
pub(crate) mod tests;
