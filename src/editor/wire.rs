//! Native editor projections of guest-owned documents. How one edit travels:
//!
//! 1. `collect` maps each mounted `Node::Editor`, by `AuthoredPath`, to a
//!    `Field`; fields naming the same document share one `Document`.
//! 2. The store asks the guest for text it lacks (`request_document`) and
//!    the text arrives as an `IncomingTransfer`.
//! 3. Each native edit and each claimed key is queued as a `QueuedInput` and
//!    handled one at a time per document. A native edit is committed at
//!    once and waits in `Phase::Acknowledgment` until the guest's next frame
//!    shows the committed revision; a key waits in `Phase::Decision` for the
//!    guest's Apply, Noop or DefaultEditorAction.
//! 4. While it decides, the guest may ask for the host's copy: an
//!    `OutgoingMirror`.
//! 5. Any error sets `fault`, which stops the pump for good.
//!
//! Transfer assemblers and patch validation are the wire contract's own code.

use crate::render::AuthoredPath;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use view_wire::editor_document::{
    EditorDocumentMessage as DocumentMessage, EditorDocumentRef, EditorTransferId,
    EditorTransferReceiver, EditorTransferSender, MAX_EDITOR_LIVE_BYTES,
    MAX_EDITOR_PROJECTION_BYTES, editor_changed_span,
};

#[derive(Clone)]
pub struct EditorStore(Arc<Mutex<Store>>);

struct Store {
    instance: u64,
    serial: u64,
    epoch: Instant,
    fields: HashMap<AuthoredPath, Field>,
    documents: HashMap<String, Document>,
    incoming: Option<IncomingTransfer>,
    outgoing: Option<OutgoingMirror>,
    events: Vec<view_wire::Event>,
    fault: Option<String>,
}

#[derive(Clone)]
struct Field {
    reference: EditorDocumentRef,
    handler: u32,
    binding: Option<Box<view_wire::EditorBinding>>,
    placeholder: String,
    editable: bool,
}

struct Document {
    reference: EditorDocumentRef,
    text: Option<Arc<str>>,
    queue: VecDeque<QueuedInput>,
    queued_bytes: usize,
    phase: Phase,
}

// Boxing `Decision` would allocate on every structural edit inside the
// per-frame pump/drain loop below; the size difference is accepted instead.
#[allow(clippy::large_enum_variant)]
enum Phase {
    Ready,
    Decision {
        request: view_wire::EditorRequest,
        since: Instant,
    },
    Acknowledgment {
        revision: u64,
    },
    Fault,
}

struct QueuedInput {
    key: AuthoredPath,
    sequence: u64,
    /// Milliseconds since the store was made; the guest gets it as `input_time_ms`.
    input_time_ms: u64,
    input: InputKind,
}

enum InputKind {
    Native(NativeEdit),
    Request(view_wire::EditorRequestInput),
}

/// Offsets relative to the previous caret let typing queued behind a structural
/// guest key follow that key's new caret, rather than overwrite its result.
struct NativeEdit {
    start: isize,
    end: isize,
    replacement: String,
    caret: isize,
    anchor: Option<isize>,
    kind: view_wire::EditorEditKind,
}

impl NativeEdit {
    /// A move of the caret and nothing else: no bytes replaced and none
    /// removed. Two of these in a row COMPOSE — every offset is relative to the
    /// caret before the edit, and an edit that writes nothing leaves the text
    /// the next one is relative to untouched — so the queue can keep the
    /// destination and forget the way there.
    fn only_the_caret_moved(&self) -> bool {
        self.start == self.end && self.replacement.is_empty()
    }

    /// Fold a later caret move into this one. Both are measured from the caret
    /// each started at, so the way there is the sum and the anchor comes back
    /// to the earlier of the two starts.
    fn then(&mut self, next: &NativeEdit) {
        self.anchor = next.anchor.map(|anchor| anchor + self.caret);
        self.caret += next.caret;
        self.kind = next.kind;
    }
}

struct IncomingTransfer {
    id: EditorTransferId,
    target: EditorDocumentRef,
    handler: u32,
    receiver: EditorTransferReceiver,
}

struct OutgoingMirror {
    handler: u32,
    sender: EditorTransferSender,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Projection {
    reference: EditorDocumentRef,
    // `pub(crate)`: a regression test outside this module reads the text an
    // AX-driven edit committed, to check that it reached the guest's store.
    pub(crate) text: Option<Arc<str>>,
    binding: Option<Box<view_wire::EditorBinding>>,
    placeholder: String,
    editable: bool,
    pending: bool,
    fault: Option<String>,
}

impl EditorStore {
    pub fn new(instance: u64) -> Self {
        Self(Arc::new(Mutex::new(Store {
            instance,
            serial: 0,
            epoch: Instant::now(),
            fields: HashMap::new(),
            documents: HashMap::new(),
            incoming: None,
            outgoing: None,
            events: Vec::new(),
            fault: None,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Store> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Validate the complete candidate before changing the accepted projections.
    pub fn validate(&self, root: &view_wire::Node) -> Result<(), String> {
        let fields = collect_checked(root)?;
        self.lock().validate_budget(&fields)
    }

    /// Refuses a hot-swap unless the old store is idle, the new one holds
    /// every document, and neither has faulted. A document whose reference
    /// and text the restored store reproduced exactly shares the old
    /// allocation. The old store is untouched.
    pub fn retain_restored_projections(
        &self,
        old: &Self,
        root: &view_wire::Node,
    ) -> Result<(), String> {
        self.validate(root)?;
        if old.pending() {
            return Err("the previous editor has pending work".into());
        }
        if !self.ready()? {
            return Err("replacement editor documents are incomplete".into());
        }
        let old = old.lock();
        old.check()?;
        let mut restored = self.lock();
        for (id, document) in &mut restored.documents {
            let Some(previous) = old.documents.get(id) else {
                continue;
            };
            let identical =
                document.reference == previous.reference && document.text == previous.text;
            if identical {
                document.text = previous.text.clone();
            }
        }
        restored.check()
    }

    pub fn replace(&self, root: &view_wire::Node) -> Result<(), String> {
        let fields = collect_checked(root)?;
        let mut store = self.lock();
        store.validate_budget(&fields)?;
        store.replace(fields);
        store.check()
    }

    /// Called after adopting the complete root, including unchanged-tree frames.
    /// `Ok(true)` when the frame moved a document a mounted field shows: a
    /// decision, a document message, or an acknowledgment that settled one
    /// (`TextEditor::sync` reads the projection at the tree's render, so a
    /// frame that moved one needs that render even with the tree unchanged).
    pub fn frame(&self, frame: &view_wire::Frame) -> Result<bool, String> {
        let mut store = self.lock();
        let settled = !frame.busy && store.acknowledge();
        for message in &frame.editor_documents {
            store.document_message(message);
        }
        for response in &frame.editor_decisions {
            store.decide(response);
        }
        store.pump();
        store.check()?;
        Ok(settled || !frame.editor_documents.is_empty() || !frame.editor_decisions.is_empty())
    }

    pub fn drain(&self) -> Vec<view_wire::Event> {
        let mut store = self.lock();
        store.pump();
        std::mem::take(&mut store.events)
    }

    pub fn ready(&self) -> Result<bool, String> {
        let store = self.lock();
        store.check()?;
        Ok(store.incoming.is_none() && store.documents.values().all(|d| d.text.is_some()))
    }

    pub fn pending(&self) -> bool {
        let store = self.lock();
        store.incoming.is_some()
            || store.outgoing.is_some()
            || !store.events.is_empty()
            || store
                .documents
                .values()
                .any(|d| !d.queue.is_empty() || !matches!(d.phase, Phase::Ready))
    }

    /// `pub(crate)`, not private: a regression test outside this module
    /// checks that the `AuthoredPath` the renderer mounts a native editor
    /// under (walked from `native_root`'s wrapped tree) is the one this
    /// resolves — the exact contract that broke when the wrapper carried
    /// an id of its own.
    pub(crate) fn projection(&self, key: &[view_wire::ElementIdWire]) -> Option<Projection> {
        let store = self.lock();
        let field = store.fields.get(key)?;
        let document = store.documents.get(&field.reference.document)?;
        Some(Projection {
            reference: document.reference.clone(),
            text: document.text.clone(),
            binding: field.binding.clone(),
            placeholder: field.placeholder.clone(),
            editable: field.editable,
            pending: !document.queue.is_empty(),
            fault: store.fault.clone(),
        })
    }

    /// A toolbar press on an editor. It is not an edit: only the guest knows
    /// what its tag means, so it reaches the field's binding as an
    /// interaction and the decision comes back the way a key's does.
    pub(crate) fn act(&self, key: &[view_wire::ElementIdWire], tag: String) {
        self.request(
            key,
            view_wire::EditorRequestInput::Interaction {
                action: view_wire::EditorInteraction::Action { tag },
            },
        );
    }

    /// A key pressed in the editor at `key`, as `TextEditor` sends one.
    #[cfg(test)]
    pub(crate) fn key_for_test(
        &self,
        key: &[view_wire::ElementIdWire],
        state: view_wire::keyboard::KeyState,
    ) {
        self.request(
            key,
            view_wire::EditorRequestInput::Key {
                key: state,
                repeat: false,
            },
        );
    }

    fn request(&self, key: &[view_wire::ElementIdWire], input: view_wire::EditorRequestInput) {
        let mut store = self.lock();
        store.enqueue(key, InputKind::Request(input));
        store.pump();
    }

    fn native(
        &self,
        key: &[view_wire::ElementIdWire],
        before: &str,
        previous: view_wire::EditorCursor,
        after: &str,
        next: view_wire::EditorCursor,
        kind: view_wire::EditorEditKind,
    ) {
        let result = native_edit(before, previous, after, next, kind);
        let mut store = self.lock();
        match result {
            Ok(edit) => store.enqueue(key, InputKind::Native(edit)),
            Err(error) => store.fault = Some(error),
        }
        store.pump();
    }
}

/// The root's editor fields, their document references checked against
/// each other; the byte budget is the store's to check, under its lock.
fn collect_checked(root: &view_wire::Node) -> Result<HashMap<AuthoredPath, Field>, String> {
    let mut fields = HashMap::new();
    collect(root, &mut fields)?;
    view_wire::editor_document::validate_editor_document_refs(
        fields.values().map(|f| &f.reference),
    )
    .map_err(|error| format!("invalid editor references: {error:?}"))?;
    Ok(fields)
}

fn collect(
    node: &view_wire::Node,
    fields: &mut HashMap<AuthoredPath, Field>,
) -> Result<(), String> {
    fn walk(
        node: &view_wire::Node,
        path: &mut AuthoredPath,
        fields: &mut HashMap<AuthoredPath, Field>,
    ) -> Result<(), String> {
        let entered = crate::render::enter_scope(node, path);
        if let view_wire::Node::Editor {
            document,
            on_document,
            binding,
            placeholder,
            editable,
            ..
        } = node
        {
            let duplicate = fields
                .insert(
                    path.clone(),
                    Field {
                        reference: document.clone(),
                        handler: *on_document,
                        binding: binding.clone(),
                        placeholder: placeholder.clone(),
                        editable: *editable,
                    },
                )
                .is_some();
            if duplicate {
                return Err("duplicate editor projection key".into());
            }
        }
        for child in node.children() {
            walk(child, path, fields)?;
        }
        if entered {
            path.pop();
        }
        Ok(())
    }
    walk(node, &mut Vec::new(), fields)
}

mod native;
mod protocol;
mod store;
use native::*;

/// Seeds `text` into `store` by answering the document request the store
/// emits for a document it has no text for, with the Begin/Chunk/Complete
/// transfer a guest sends. An empty text sends no Chunk: a zero-byte
/// assembler is complete at Begin, and even an empty Chunk is out of order.
#[cfg(test)]
pub(crate) fn seed_editor_text(store: &EditorStore, text: &str) {
    use view_wire::editor_document::{EditorDocumentMessage as Message, EditorTransfer};
    let asked = store.drain().into_iter().find_map(|event| match event {
        view_wire::Event::EditorDocument {
            message: Message::Request { id, target },
            ..
        } => Some((id, target)),
        _ => None,
    });
    let (id, target) = asked.expect("the store asks for a document it has no text for");
    let mut editor_documents = vec![Message::Transfer(EditorTransfer::Begin {
        id: id.clone(),
        target,
    })];
    if !text.is_empty() {
        editor_documents.push(Message::Transfer(EditorTransfer::Chunk {
            id: id.clone(),
            index: 0,
            bytes: text.as_bytes().to_vec(),
        }));
    }
    editor_documents.push(Message::Transfer(EditorTransfer::Complete { id }));
    store
        .frame(&view_wire::Frame {
            editor_documents,
            ..Default::default()
        })
        .expect("the answer to the store's own request");
}

#[path = "text.rs"]
mod text;
pub use text::TextEditor;
