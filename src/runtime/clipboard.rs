//! The clipboard, read and written on the window thread for one view.
use super::wire::methods::{self, Capability, refusal};
use super::{Seat, View};
use gpui_kit::{ClipboardEntry, Context};

/// Requests a view may have waiting for the window thread between two
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
    view: &mut View,
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
    // their activation, which the ask takes; a view on a clock never sees it
    match request {
        Ok(_) if !view.take_activation() => view.refuse(
            id,
            refusal::NEEDS_GESTURE,
            "the clipboard needs a press or key",
        ),
        Ok(request) => queue(view, id, request),
        Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
    true
}

fn queue(view: &mut View, id: u64, request: Request) {
    if view.clipboard.pending.len() >= MAX_PENDING {
        view.refuse(
            id,
            refusal::IN_FLIGHT_LIMIT,
            "too many pending clipboard requests",
        );
        return;
    }
    view.clipboard.pending.push((id, request));
}

pub(super) fn mount(view: &mut View, cx: &mut Context<Seat>) {
    for (id, request) in std::mem::take(&mut view.clipboard.pending) {
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
                view.reply(id, Ok(methods::encode(&methods::Clipboard { text })));
            }
            Request::Write(text) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                view.reply(id, Ok(Vec::new()));
            }
        }
    }
}
