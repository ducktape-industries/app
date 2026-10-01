//! The clipboard, read and written on the window thread for one guest.
use super::wire::methods::{self, Capability, refusal};
use super::{Guest, Seat};
use gpui_kit::{ClipboardEntry, Context};

/// Requests a guest may have waiting for the window thread between two
/// ticks; a view has no reason to read the clipboard sixteen times a frame.
const MAX_PENDING: usize = 16;

#[derive(Default)]
pub(super) struct Clipboard {
    pending: Vec<(u64, Request)>,
}
enum Request {
    Read,
    Write(String),
}

impl Clipboard {
    pub(super) fn cancel(&mut self, id: u64) {
        self.pending.retain(|(pending, _)| *pending != id);
    }
}

pub(super) fn answer(
    guest: &mut Guest,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    let request = match (capability, operation) {
        (Capability::Clipboard, "read") => Ok(Request::Read),
        (Capability::Clipboard, "write") => methods::decode::<String>(payload).map(Request::Write),
        _ => return false,
    };
    // what the person copied elsewhere is read, and replaced, only on
    // their press or key in this view; a view on a clock never sees it
    match request {
        _ if guest.user_activation.is_none() => guest.refuse(
            id,
            refusal::NEEDS_GESTURE,
            "the clipboard needs a press or key",
        ),
        Ok(request) => {
            guest.gesture_used = true;
            queue(guest, id, request)
        }
        Err(error) => guest.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
    true
}

fn queue(guest: &mut Guest, id: u64, request: Request) {
    if guest.clipboard.pending.len() >= MAX_PENDING {
        guest.refuse(
            id,
            refusal::IN_FLIGHT_LIMIT,
            "too many pending clipboard requests",
        );
        return;
    }
    guest.clipboard.pending.push((id, request));
}

pub(super) fn mount(guest: &mut Guest, cx: &mut Context<Seat>) {
    for (id, request) in std::mem::take(&mut guest.clipboard.pending) {
        match request {
            Request::Read => {
                let mut text = String::new();
                if let Some(item) = cx.read_from_clipboard() {
                    for entry in item.entries() {
                        if let ClipboardEntry::String(value) = entry {
                            text.push_str(value.text());
                        }
                    }
                }
                guest.reply(id, Ok(methods::encode(&methods::Clipboard { text })));
            }
            Request::Write(text) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                guest.reply(id, Ok(Vec::new()));
            }
        }
    }
}
