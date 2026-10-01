//! The store's half of a document transfer, against a guest that keeps its
//! half the way view-guest does.
use super::*;
use view_wire::editor_document::EditorTransferError;

fn reference(document: &str, reset: u64) -> EditorDocumentRef {
    EditorDocumentRef {
        document: document.into(),
        reset,
        text_revision: 0,
        revision: 0,
        cursor: Default::default(),
        byte_len: 0,
    }
}

/// A view whose whole tree is one empty composer on `document`.
fn composer(document: &EditorDocumentRef) -> view_wire::Node {
    view_wire::Node::Editor {
        binding: None,
        id: view_wire::ElementIdWire::Name(format!("{}/editor", document.document).into()),
        style: Default::default(),
        placeholder: String::new(),
        label: None,
        document: document.clone(),
        on_document: 5,
        editable: true,
    }
}

fn documents(events: Vec<view_wire::Event>) -> Vec<DocumentMessage> {
    events
        .into_iter()
        .filter_map(|event| match event {
            view_wire::Event::EditorDocument { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

/// The guest's half of a transfer as view-guest's `slots/editor.rs` keeps
/// it: one sender, which an `Acknowledged` or a `Failed` naming its id ends
/// and nothing else does; a request under another id while it is open is
/// refused with `Limit`.
#[derive(Default)]
struct Guest {
    sender: Option<EditorTransferSender>,
}

impl Guest {
    /// One tick of a view showing the empty draft `shown`: the host's
    /// events, then the one document message a frame carries. A refusal
    /// takes the frame's place, and a sender is driven only while the view
    /// shows its document.
    fn tick(
        &mut self,
        events: Vec<view_wire::Event>,
        shown: &EditorDocumentRef,
    ) -> view_wire::Frame {
        let mut refused = None;
        for message in documents(events) {
            match message {
                DocumentMessage::Acknowledged { id } | DocumentMessage::Failed { id, .. } => {
                    if self.sender.as_ref().is_some_and(|open| open.id() == &id) {
                        self.sender = None;
                    }
                }
                DocumentMessage::Request { id, target } if self.sender.is_none() => {
                    self.sender = Some(
                        EditorTransferSender::new(id, target).expect("the store's own request"),
                    );
                }
                DocumentMessage::Request { id, .. } => {
                    refused = Some(DocumentMessage::Failed {
                        id,
                        reason: EditorTransferError::Limit,
                    });
                }
                DocumentMessage::Transfer(_) => {}
            }
        }
        let message = refused.or_else(|| {
            self.sender
                .as_mut()
                .filter(|open| open.id().document == shown.document)
                .and_then(|open| open.next_frame(shown, "").expect("an empty draft"))
                .map(DocumentMessage::Transfer)
        });
        view_wire::Frame {
            editor_documents: message.into_iter().collect(),
            ..Default::default()
        }
    }
}

/// app#199. Chat keeps one draft per channel, `draft-<channel>`, so opening
/// another channel replaces the document the store is still receiving. The
/// store dropped that transfer without a word, the guest's sender for it
/// stayed open, and the request for the next draft came back refused: the
/// reader saw "This view stopped: editor document transfer refused: Limit".
#[test]
fn opening_another_channel_while_a_draft_is_on_its_way_does_not_stop_the_view() {
    let store = EditorStore::new(3);
    let mut guest = Guest::default();
    let general = reference("draft-general", 1);
    let random = reference("draft-random", 1);
    store.replace(&composer(&general)).unwrap();
    let begun = guest.tick(store.drain(), &general);
    store.frame(&begun).unwrap();
    // The reader opens the other channel before the first draft is complete.
    store.replace(&composer(&random)).unwrap();
    for _ in 0..3 {
        let frame = guest.tick(store.drain(), &random);
        if let Err(fault) = store.frame(&frame) {
            panic!("This view stopped: {fault}");
        }
    }
    assert_eq!(store.ready(), Ok(true), "the other channel's draft is here");
    assert!(guest.sender.is_none(), "the guest holds no transfer open");
}

/// A draft replaced under its own name (a new reset) is a new document to
/// the store just as another channel's draft is. Either way the guest hears
/// that the transfer it was answering is over before it is asked again.
#[test]
fn a_document_replaced_mid_transfer_ends_the_transfer_before_the_next_request() {
    for next in [reference("draft-random", 1), reference("draft-general", 2)] {
        let store = EditorStore::new(3);
        store
            .replace(&composer(&reference("draft-general", 1)))
            .unwrap();
        let asked = documents(store.drain());
        let [DocumentMessage::Request { id: first, .. }] = &asked[..] else {
            panic!("one request for the draft: {asked:?}");
        };
        store.replace(&composer(&next)).unwrap();
        let said = documents(store.drain());
        assert!(
            matches!(
                &said[..],
                [
                    DocumentMessage::Failed {
                        id,
                        reason: EditorTransferError::Aborted,
                    },
                    DocumentMessage::Request { target, .. },
                ] if id == first && target == &next
            ),
            "a Failed for the first transfer, then the next request: {said:?}"
        );
    }
}
