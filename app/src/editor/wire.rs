//! Native editor projections of guest-owned documents. A key waits for its
//! decision, and an accepted edit waits for the guest's observed revision.
//! Transfer assemblers and patch validation are the wire contract's own code.

use crate::view_tree::AuthoredPath;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use view_wire as wire;
use wire::editor_document::{
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
    incoming: Option<Incoming>,
    outgoing: Option<Outgoing>,
    events: Vec<wire::Event>,
    fault: Option<String>,
}

#[derive(Clone)]
struct Field {
    reference: EditorDocumentRef,
    handler: u32,
    options: wire::EditorOptions,
    placeholder: String,
    editable: bool,
}

struct Document {
    reference: EditorDocumentRef,
    text: Option<Arc<str>>,
    queue: VecDeque<Work>,
    queued_bytes: usize,
    phase: Phase,
}

// Boxing `Decision` would allocate on every structural edit inside the
// per-frame pump/drain loop below; the size difference is accepted instead.
#[allow(clippy::large_enum_variant)]
enum Phase {
    Ready,
    Decision {
        request: wire::EditorRequest,
        since: Instant,
    },
    Acknowledgment {
        revision: u64,
    },
    Fault,
}

struct Work {
    key: AuthoredPath,
    sequence: u64,
    at: u64,
    input: Input,
}

enum Input {
    Native(NativeEdit),
    Request(wire::EditorRequestInput),
}

/// Offsets relative to the previous caret let typing queued behind a structural
/// guest key follow that key's new caret, rather than overwrite its result.
struct NativeEdit {
    start: isize,
    end: isize,
    replacement: String,
    caret: isize,
    anchor: Option<isize>,
    kind: wire::EditorEditKind,
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

struct Incoming {
    id: EditorTransferId,
    target: EditorDocumentRef,
    handler: u32,
    receiver: EditorTransferReceiver,
}

struct Outgoing {
    handler: u32,
    sender: EditorTransferSender,
}

#[derive(Clone, PartialEq)]
struct Projection {
    reference: EditorDocumentRef,
    text: Option<Arc<str>>,
    options: wire::EditorOptions,
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
    pub fn validate(&self, root: &wire::Node) -> Result<(), String> {
        let mut fields = HashMap::new();
        collect(root, &mut fields)?;
        wire::editor_document::validate_editor_document_refs(
            fields.values().map(|field| &field.reference),
        )
        .map_err(|error| format!("invalid editor references: {error:?}"))?;
        self.lock().validate_budget(&fields)
    }

    /// A replacement may reuse immutable document bytes only when its restored
    /// reference names exactly the same projection. The old store is untouched.
    pub fn retain_restored_projections(&self, old: &Self, root: &wire::Node) -> Result<(), String> {
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

    pub fn replace(&self, root: &wire::Node) -> Result<(), String> {
        let mut fields = HashMap::new();
        collect(root, &mut fields)?;
        wire::editor_document::validate_editor_document_refs(fields.values().map(|f| &f.reference))
            .map_err(|error| format!("invalid editor references: {error:?}"))?;
        let mut store = self.lock();
        store.validate_budget(&fields)?;
        store.replace(fields);
        store.check()
    }

    /// Called after adopting the complete root, including unchanged-tree frames.
    pub fn frame(&self, frame: &wire::Frame) -> Result<(), String> {
        let mut store = self.lock();
        if !frame.busy {
            store.acknowledge();
        }
        for message in &frame.editor_documents {
            store.document_message(message);
        }
        for response in &frame.editor_decisions {
            store.decide(response);
        }
        store.pump();
        store.check()
    }

    pub fn drain(&self) -> Vec<wire::Event> {
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

    fn projection(&self, key: &[wire::ElementIdWire]) -> Option<Projection> {
        let store = self.lock();
        let field = store.fields.get(key)?;
        let document = store.documents.get(&field.reference.document)?;
        Some(Projection {
            reference: document.reference.clone(),
            text: document.text.clone(),
            options: field.options.clone(),
            placeholder: field.placeholder.clone(),
            editable: field.editable,
            pending: !document.queue.is_empty(),
            fault: store.fault.clone(),
        })
    }

    /// A toolbar press on an editor. It is not an edit: only the guest knows
    /// what its tag means, so it reaches the field's binding as an
    /// interaction and the decision comes back the way a key's does.
    pub(crate) fn act(&self, key: &[wire::ElementIdWire], tag: String) {
        self.request(
            key,
            wire::EditorRequestInput::Interaction {
                action: wire::editor_presentation::EditorInteraction::Action { tag },
            },
        );
    }

    fn request(&self, key: &[wire::ElementIdWire], input: wire::EditorRequestInput) {
        let mut store = self.lock();
        store.enqueue(key, Input::Request(input));
        store.pump();
    }

    // NativeModuleView applies the guest's event-interest mask on dispatch.
    // These observations never mutate the authoritative editor document.
    fn observe_ime(&self, events: Vec<wire::Event>) {
        self.lock().events.extend(events);
    }

    fn native(
        &self,
        key: &[wire::ElementIdWire],
        before: &str,
        previous: wire::EditorCursor,
        after: &str,
        next: wire::EditorCursor,
        kind: wire::EditorEditKind,
    ) {
        let result = native_edit(before, previous, after, next, kind);
        let mut store = self.lock();
        match result {
            Ok(edit) => store.enqueue(key, Input::Native(edit)),
            Err(error) => store.fault = Some(error),
        }
        store.pump();
    }
}

fn collect(node: &wire::Node, fields: &mut HashMap<AuthoredPath, Field>) -> Result<(), String> {
    fn walk(
        node: &wire::Node,
        path: &mut AuthoredPath,
        fields: &mut HashMap<AuthoredPath, Field>,
    ) -> Result<(), String> {
        let entered = match node.identity() {
            Some(wire::IdentityKeyRef::Element(id)) => {
                path.push(id.clone());
                true
            }
            _ => false,
        };
        if let wire::Node::Editor {
            document,
            on_document,
            options,
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
                        options: (**options).clone(),
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

mod store;
mod protocol;
mod native;
use native::*;

#[path = "text.rs"]
mod text;
pub use text::{GUEST_EDITOR_CONTEXT, TextEditor};
