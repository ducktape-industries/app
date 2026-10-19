//! The clipboard, read and written on the window thread for one view.
//! `clipboard.files` reads what a paste would take: the files and
//! pictures on it, held for the view as a pick's are (`picker`), and its
//! text, in one answer, since the paste key is one activation.
#[cfg(feature = "ui")]
use super::Seat;
use super::View;
use super::wire::methods::{self, Capability, refusal};
#[cfg(feature = "ui")]
use super::{kernel, picker};
#[cfg(feature = "ui")]
use gpui_kit::{ClipboardEntry, Context};

#[derive(Default)]
pub(super) struct Clipboard {
    pending: Vec<(u64, Request)>,
}
enum Request {
    Read,
    Write(String),
    Files,
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
        (Capability::Clipboard, "files") => methods::decode::<()>(payload).map(|()| Request::Files),
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
        Ok(request) => view.clipboard.pending.push((id, request)),
        Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
    true
}

#[cfg(feature = "ui")]
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
            Request::Files => files(view, id, cx),
        }
    }
}

/// `clipboard.files`: the clipboard's text, and its files and pictures held
/// for the view. A picture is written to an unnamed file of its own and
/// held as a file, named `image.<ext>` by its format; that and opening a
/// file run on a blocking thread, and one that fails refuses the whole ask
/// by its name alone, as a pick does.
#[cfg(feature = "ui")]
fn files(view: &mut View, id: u64, cx: &mut Context<Seat>) {
    let (mut text, mut paths, mut pictures) = (String::new(), Vec::new(), Vec::new());
    if let Some(item) = cx.read_from_clipboard() {
        for entry in item.entries() {
            match entry {
                ClipboardEntry::String(value) => text.push_str(value.text()),
                ClipboardEntry::ExternalPaths(external) => {
                    paths.extend_from_slice(external.paths())
                }
                ClipboardEntry::Image(image) => {
                    let name = format!("image.{}", image.format.extension());
                    pictures.push((name, image.bytes.clone()));
                }
            }
        }
    }
    let held = view.picker.held();
    kernel::spawn_reply(view, id, async move {
        let opened = tokio::task::spawn_blocking(move || {
            let mut files = pictures
                .into_iter()
                .map(|(name, bytes)| picker::Picked::bytes(name, &bytes))
                .collect::<Result<Vec<_>, _>>()?;
            files.extend(picker::open_paths(paths)?);
            Ok(files)
        });
        let files = opened
            .await
            .unwrap_or_else(|error| Err(picker::fault(error.to_string())))?;
        let files = held.hold(files);
        Ok(methods::encode(&methods::Pasted { files, text }))
    });
}
