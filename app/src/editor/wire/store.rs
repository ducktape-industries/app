use super::*;

impl Store {
    pub(super) fn check(&self) -> Result<(), String> {
        self.fault.clone().map_or(Ok(()), Err)
    }

    pub(super) fn next(&mut self) -> u64 {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("editor sequence exhausted");
        self.serial
    }

    pub(super) fn validate_budget(
        &self,
        fields: &HashMap<AuthoredPath, Field>,
    ) -> Result<(), String> {
        let mut logical = HashMap::new();
        let mut projections = 0usize;
        for field in fields.values() {
            let reference = &field.reference;
            let bytes = self
                .documents
                .get(&reference.document)
                .filter(|d| d.reference.reset == reference.reset)
                .and_then(|d| d.text.as_ref())
                .map_or(reference.byte_len as usize, |text| text.len());
            logical.insert(reference.document.clone(), bytes);
            projections = projections
                .checked_add(bytes)
                .ok_or("editor projection budget")?;
        }
        let total = logical
            .values()
            .try_fold(0usize, |sum, n| sum.checked_add(*n))
            .ok_or("editor document budget")?;
        let exceeds_budget =
            total > MAX_EDITOR_LIVE_BYTES || projections > MAX_EDITOR_PROJECTION_BYTES;
        if exceeds_budget {
            return Err("editor live document budget exceeded".into());
        }
        Ok(())
    }

    pub(super) fn replace(&mut self, fields: HashMap<AuthoredPath, Field>) {
        let removed: Vec<_> = self
            .documents
            .keys()
            .filter(|name| !fields.values().any(|f| &f.reference.document == *name))
            .cloned()
            .collect();
        for name in removed {
            self.cancel(&name);
            self.documents.remove(&name);
        }
        for field in fields.values() {
            let reference = &field.reference;
            let changed_reset = self
                .documents
                .get(&reference.document)
                .is_some_and(|d| d.reference.reset != reference.reset);
            if changed_reset {
                self.cancel(&reference.document);
                self.documents.remove(&reference.document);
            }
            self.documents
                .entry(reference.document.clone())
                .or_insert_with(|| Document {
                    reference: reference.clone(),
                    text: None,
                    queue: VecDeque::new(),
                    queued_bytes: 0,
                    phase: Phase::Ready,
                });
        }
        self.fields = fields;
        let stale_incoming = self.incoming.as_ref().is_some_and(|incoming| {
            self.documents
                .get(&incoming.id.document)
                .is_none_or(|d| d.reference.reset != incoming.id.reset)
        });
        if stale_incoming {
            self.incoming = None;
        }
        let stale_outgoing = self.outgoing.as_ref().is_some_and(|outgoing| {
            self.documents
                .get(&outgoing.sender.id().document)
                .is_none_or(|d| d.reference.reset != outgoing.sender.id().reset)
        });
        if stale_outgoing {
            self.outgoing = None;
        }
    }

    pub(super) fn cancel(&mut self, name: &str) {
        let Some(document) = self.documents.get_mut(name) else {
            return;
        };
        for work in document.queue.drain(..) {
            let Some(binding) = self
                .fields
                .get(&work.key)
                .and_then(|f| f.options.binding.as_ref())
            else {
                continue;
            };
            self.events.push(wire::Event::EditorTransaction {
                handler: binding.on_event,
                event: wire::EditorTransactionEvent::Cancelled {
                    id: transaction_id(self.instance, &document.reference, work.sequence),
                    state: document.reference.clone(),
                },
            });
        }
        document.queued_bytes = 0;
    }

    pub(super) fn enqueue(&mut self, key: &[wire::ElementIdWire], input: Input) {
        if self.fault.is_some() {
            return;
        }
        let Some(field) = self.fields.get(key).cloned() else {
            return;
        };
        let interaction = matches!(
            &input,
            Input::Request(wire::EditorRequestInput::Interaction { .. })
        );
        let allowed = field.editable || interaction;
        if !allowed {
            return;
        }
        let sequence = self.next();
        let at = self.epoch.elapsed().as_millis() as u64;
        let Some(document) = self.documents.get_mut(&field.reference.document) else {
            return;
        };
        // Typing into a field whose document has not arrived is kept: a
        // native edit and a key are relative to the caret, so the pump
        // replays them onto the document once it is here. A rich edit is a
        // snapshot of the whole document, and one taken before it arrived
        // would write over it.
        let relative = matches!(
            &input,
            Input::Native(_) | Input::Request(wire::EditorRequestInput::Key { .. })
        );
        if document.text.is_none() && !relative {
            return;
        }
        // A drag-selection is one caret move per pointer sample and the queue
        // drains one item per guest frame, so a drag through a paragraph could
        // fill it, fault the view and leave the field read-only for good. Two
        // caret moves in a row compose exactly, so the queue keeps one — never
        // the one already handed to the guest, which is the front whenever the
        // document is waiting on it.
        let sent = !matches!(document.phase, Phase::Ready);
        let behind_the_one_in_flight = document.queue.len() > usize::from(sent);
        if behind_the_one_in_flight
            && let Input::Native(edit) = &input
            && edit.only_the_caret_moved()
            && let Some(standing) = document.queue.back_mut()
            && standing.key == key
            && let Input::Native(held) = &mut standing.input
            && held.only_the_caret_moved()
        {
            held.then(edit);
            standing.at = at;
            return;
        }
        let bytes = match &input {
            Input::Native(edit) => edit.replacement.len(),
            Input::Request(request) => wire::encode(request).len(),
        };
        let budget = if field.options.rich.is_some() {
            wire::editor_rich::MAX_RICH_QUEUE_BYTES
        } else {
            wire::editor_transaction::MAX_EDITOR_INPUT_BYTES
        };
        let overflow =
            document.queue.len() >= 128 || document.queued_bytes.saturating_add(bytes) > budget;
        if overflow {
            self.fault = Some("editor input queue is full; document retained".into());
            self.events.push(wire::Event::EditorTransaction {
                handler: field.options.binding.as_ref().map_or(0, |b| b.on_event),
                event: wire::EditorTransactionEvent::Fault {
                    id: transaction_id(self.instance, &document.reference, sequence),
                    state: document.reference.clone(),
                    reason: wire::EditorFault::Overflow,
                },
            });
            return;
        }
        document.queued_bytes += bytes;
        document.queue.push_back(Work {
            key: key.to_vec(),
            sequence,
            at,
            input,
        });
    }

    pub(super) fn pump(&mut self) {
        if self.fault.is_some() {
            return;
        }
        self.send_mirror();
        self.request_document();
        let names: Vec<_> = self.documents.keys().cloned().collect();
        for name in names {
            self.pump_document(&name);
        }
    }
}
