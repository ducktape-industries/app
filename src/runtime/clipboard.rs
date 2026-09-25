//! The clipboard, read and written on the window thread for one guest.
use super::wire::methods;
use super::{Guest, NativeModuleView};
use gpui_kit::{ClipboardEntry, Context};

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
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        ("clipboard", "read") => queue(guest, id, Request::Read),
        ("clipboard", "write") => match methods::decode::<String>(payload) {
            Ok(text) => queue(guest, id, Request::Write(text)),
            Err(error) => guest.refuse(id, "malformed_request", error),
        },
        _ => return false,
    }
    true
}

fn queue(guest: &mut Guest, id: u64, request: Request) {
    if guest.clipboard.pending.len() >= 16 {
        guest.refuse(id, "in_flight_limit", "too many pending clipboard requests");
        return;
    }
    guest.clipboard.pending.push((id, request));
}

pub(super) fn mount(guest: &mut Guest, cx: &mut Context<NativeModuleView>) {
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
