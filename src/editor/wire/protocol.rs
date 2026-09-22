use super::*;

impl Store {
    pub(super) fn request_document(&mut self) {
        let transfer_active = self.incoming.is_some() || self.outgoing.is_some();
        if transfer_active {
            return;
        }
        let Some(field) = self
            .fields
            .values()
            .find(|f| {
                self.documents
                    .get(&f.reference.document)
                    .is_some_and(|d| d.text.is_none())
            })
            .cloned()
        else {
            return;
        };
        let id = EditorTransferId {
            instance: self.instance,
            document: field.reference.document.clone(),
            reset: field.reference.reset,
            serial: self.next(),
            attempt: 0,
        };
        let target = field.reference;
        let receiver = match EditorTransferReceiver::new(id.clone(), target.clone()) {
            Ok(receiver) => receiver,
            Err(error) => {
                self.fault = Some(format!("invalid editor transfer: {error:?}"));
                return;
            }
        };
        self.events.push(wire::Event::EditorDocument {
            handler: field.handler,
            message: DocumentMessage::Request {
                id: id.clone(),
                target: target.clone(),
            },
        });
        self.incoming = Some(Incoming {
            id,
            target,
            handler: field.handler,
            receiver,
        });
    }

    pub(super) fn pump_document(&mut self, name: &str) {
        let Some(document) = self.documents.get_mut(name) else {
            return;
        };
        if let Phase::Decision { since, .. } = &document.phase {
            if since.elapsed().as_secs() >= 5 {
                document.phase = Phase::Fault;
                self.fault = Some("editor key decision timed out; input retained".into());
            }
            return;
        }
        // Keys typed before the document arrived wait here for it.
        if !matches!(document.phase, Phase::Ready) || document.text.is_none() {
            return;
        }
        let Some(front) = document.queue.front() else {
            return;
        };
        let Some(field) = self.fields.get(&front.key) else {
            return;
        };
        let Some(binding) = field.options.binding.as_ref() else {
            self.fault = Some("editor has no guest transaction binding".into());
            return;
        };
        match &front.input {
            Input::Request(input) => {
                let request = wire::EditorRequest {
                    id: transaction_id(self.instance, &document.reference, front.sequence),
                    state: document.reference.clone(),
                    input: input.clone(),
                    input_time_ms: front.at,
                };
                self.events.push(wire::Event::EditorRequest {
                    handler: binding.on_request,
                    request: request.clone(),
                });
                document.phase = Phase::Decision {
                    request,
                    since: Instant::now(),
                };
            }
            Input::Native(edit) => {
                let Some(text) = document.text.as_ref() else {
                    return;
                };
                let change = apply_native(text, document.reference.cursor, edit);
                match change {
                    Ok((patches, cursor)) => self.commit(
                        name,
                        patches,
                        cursor,
                        wire::EditorHistoryEffect::Native,
                        None,
                    ),
                    Err(error) => self.fault = Some(error),
                }
            }
        }
    }

    pub(super) fn commit(
        &mut self,
        name: &str,
        patches: Vec<wire::EditorPatch>,
        cursor: wire::EditorCursor,
        history: wire::EditorHistoryEffect,
        origin: Option<wire::EditorRequestInput>,
    ) {
        let Some(document) = self.documents.get(name) else {
            return;
        };
        let Some(text) = document.text.as_ref() else {
            return;
        };
        let next = match wire::patched_editor_text(text, &patches, cursor) {
            Ok(text) => text,
            Err(error) => {
                self.fault = Some(format!("invalid editor decision: {error:?}"));
                return;
            }
        };
        let new_len = next.len();
        let mut candidate = self.fields.clone();
        for field in candidate
            .values_mut()
            .filter(|f| f.reference.document == name)
        {
            field.reference.byte_len = new_len as u32;
        }
        let others = self
            .documents
            .iter()
            .filter(|(key, _)| key.as_str() != name)
            .map(|(_, d)| {
                d.text
                    .as_ref()
                    .map_or(d.reference.byte_len as usize, |t| t.len())
            })
            .sum::<usize>();
        let projected = candidate
            .values()
            .map(|f| {
                if f.reference.document == name {
                    new_len
                } else {
                    self.documents
                        .get(&f.reference.document)
                        .and_then(|d| d.text.as_ref())
                        .map_or(f.reference.byte_len as usize, |t| t.len())
                }
            })
            .sum::<usize>();
        let exceeds =
            others + new_len > MAX_EDITOR_LIVE_BYTES || projected > MAX_EDITOR_PROJECTION_BYTES;
        if exceeds {
            self.fault = Some("editor edit exceeds live document budget".into());
            return;
        }
        let document = self.documents.get_mut(name).expect("document checked");
        let Some(front) = document.queue.front() else {
            return;
        };
        let Some(binding) = self
            .fields
            .get(&front.key)
            .and_then(|f| f.options.binding.as_ref())
        else {
            return;
        };
        let before = document.reference.clone();
        let changed = document.text.as_ref().is_some_and(|t| t.as_ref() != next);
        let mut after = before.clone();
        after.cursor = cursor;
        after.byte_len = next.len() as u32;
        after.revision = after.revision.saturating_add(1);
        if changed {
            after.text_revision = after.text_revision.saturating_add(1);
        }
        let kind = match &front.input {
            Input::Native(edit) => edit.kind,
            Input::Request(_) => match history {
                wire::EditorHistoryEffect::Undo => wire::EditorEditKind::Undo,
                wire::EditorHistoryEffect::Redo => wire::EditorEditKind::Redo,
                _ => wire::EditorEditKind::GuestPatch,
            },
        };
        self.events.push(wire::Event::EditorTransaction {
            handler: binding.on_event,
            event: wire::EditorTransactionEvent::Commit {
                id: transaction_id(self.instance, &before, front.sequence),
                origin,
                before,
                after: after.clone(),
                patches,
                kind,
                history,
                input_time_ms: front.at,
            },
        });
        document.text = Some(Arc::from(next));
        document.reference = after.clone();
        document.phase = Phase::Acknowledgment {
            revision: after.revision,
        };
    }

    pub(super) fn acknowledge(&mut self) {
        for document in self.documents.values_mut() {
            let Phase::Acknowledgment { revision } = document.phase else {
                continue;
            };
            let observed = self.fields.values().any(|f| {
                f.reference.document == document.reference.document
                    && f.reference.reset == document.reference.reset
                    && f.reference.revision >= revision
            });
            if !observed {
                continue;
            }
            if let Some(work) = document.queue.pop_front() {
                let bytes = match work.input {
                    Input::Native(edit) => edit.replacement.len(),
                    Input::Request(request) => wire::encode(&request).len(),
                };
                document.queued_bytes = document.queued_bytes.saturating_sub(bytes);
            }
            document.phase = Phase::Ready;
        }
    }

    pub(super) fn decide(&mut self, response: &wire::EditorResponse) {
        let name = &response.id.document;
        let Some(document) = self.documents.get(name) else {
            return;
        };
        let Phase::Decision { request, .. } = &document.phase else {
            return;
        };
        if response.id != request.id {
            return;
        }
        let request = request.clone();
        match &response.decision {
            wire::EditorDecision::Apply {
                patches,
                cursor,
                history,
            } => self.commit(
                name,
                patches.clone(),
                *cursor,
                *history,
                Some(request.input),
            ),
            wire::EditorDecision::Noop => self.noop(name, request),
            wire::EditorDecision::DefaultEditorAction => self.default_action(name, request),
        }
    }

    pub(super) fn noop(&mut self, name: &str, request: wire::EditorRequest) {
        let wire::EditorRequestInput::Interaction { action } = &request.input else {
            self.commit(
                name,
                vec![],
                request.state.cursor,
                wire::EditorHistoryEffect::Native,
                Some(request.input),
            );
            return;
        };
        let Some(document) = self.documents.get_mut(name) else {
            return;
        };
        let Some(front) = document.queue.front() else {
            return;
        };
        let Some(binding) = self
            .fields
            .get(&front.key)
            .and_then(|f| f.options.binding.as_ref())
        else {
            return;
        };
        self.events.push(wire::Event::EditorTransaction {
            handler: binding.on_event,
            event: wire::EditorTransactionEvent::Interaction {
                id: request.id,
                state: request.state,
                action: action.clone(),
                input_time_ms: request.input_time_ms,
            },
        });
        document.phase = Phase::Acknowledgment {
            revision: document.reference.revision,
        };
    }

    pub(super) fn default_action(&mut self, name: &str, request: wire::EditorRequest) {
        let Some(document) = self.documents.get(name) else {
            return;
        };
        let Some(text) = document.text.as_ref() else {
            return;
        };
        let wire::EditorRequestInput::Key { key, .. } = &request.input else {
            self.fault = Some("editor interaction cannot request a native key action".into());
            return;
        };
        let (patches, cursor) = native_key(text, document.reference.cursor, key);
        self.commit(
            name,
            patches,
            cursor,
            wire::EditorHistoryEffect::Native,
            Some(request.input),
        );
    }

    pub(super) fn document_message(&mut self, message: &DocumentMessage) {
        match message {
            DocumentMessage::Request { id, target } => self.mirror_requested(id, target),
            DocumentMessage::Transfer(transfer) => self.transferred(transfer),
            DocumentMessage::Acknowledged { id } => self.mirror_acknowledged(id),
            DocumentMessage::Failed { id, reason } => self.transfer_failed(id, *reason),
        }
    }

    pub(super) fn mirror_requested(&mut self, id: &EditorTransferId, target: &EditorDocumentRef) {
        if self.outgoing.is_some() || self.incoming.is_some() {
            return;
        }
        let Some(document) = self.documents.get(&id.document) else {
            return;
        };
        let Phase::Decision { request, .. } = &document.phase else {
            return;
        };
        let accepted = id.instance == self.instance
            && id.serial == request.id.sequence
            && id.attempt == request.id.attempt
            && id.reset == document.reference.reset
            && target == &document.reference;
        if !accepted {
            return;
        }
        let Some(front) = document.queue.front() else {
            return;
        };
        let Some(field) = self.fields.get(&front.key) else {
            return;
        };
        match EditorTransferSender::new(id.clone(), target.clone()) {
            Ok(sender) => {
                self.outgoing = Some(Outgoing {
                    handler: field.handler,
                    sender,
                })
            }
            Err(error) => self.fault = Some(format!("invalid editor mirror request: {error:?}")),
        }
    }

    pub(super) fn send_mirror(&mut self) {
        let Some(outgoing) = &mut self.outgoing else {
            return;
        };
        let Some(document) = self.documents.get(&outgoing.sender.id().document) else {
            self.outgoing = None;
            return;
        };
        let Some(text) = document.text.as_ref() else {
            return;
        };
        match outgoing.sender.next_frame(&document.reference, text) {
            Ok(Some(transfer)) => self.events.push(wire::Event::EditorDocument {
                handler: outgoing.handler,
                message: DocumentMessage::Transfer(transfer),
            }),
            Ok(None) => {}
            Err(error) => self.fault = Some(format!("editor mirror failed: {error:?}")),
        }
    }

    pub(super) fn transferred(&mut self, transfer: &wire::editor_document::EditorTransfer) {
        let Some(incoming) = &mut self.incoming else {
            return;
        };
        if transfer.id() != &incoming.id {
            return;
        }
        match incoming.receiver.receive(transfer) {
            Ok(Some(text)) => {
                let incoming = self.incoming.take().expect("matching incoming transfer");
                let Some(document) = self.documents.get_mut(&incoming.id.document) else {
                    return;
                };
                document.text = Some(Arc::from(text));
                document.reference = incoming.target;
                self.events.push(wire::Event::EditorDocument {
                    handler: incoming.handler,
                    message: DocumentMessage::Acknowledged { id: incoming.id },
                });
            }
            Ok(None) => {}
            Err(error) => self.fault = Some(format!("editor document transfer failed: {error:?}")),
        }
    }

    pub(super) fn mirror_acknowledged(&mut self, id: &EditorTransferId) {
        let matches = self
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.sender.id() == id);
        if matches {
            self.outgoing = None;
        }
    }

    pub(super) fn transfer_failed(
        &mut self,
        id: &EditorTransferId,
        reason: wire::editor_document::EditorTransferError,
    ) {
        let current = self
            .incoming
            .as_ref()
            .is_some_and(|incoming| &incoming.id == id)
            || self
                .outgoing
                .as_ref()
                .is_some_and(|outgoing| outgoing.sender.id() == id);
        if current {
            self.fault = Some(format!("editor document transfer refused: {reason:?}"));
        }
    }
}
