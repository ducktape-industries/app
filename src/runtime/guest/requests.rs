use super::*;

/// The `methods::Call` envelope around an op: its target name and two
/// lengths, allowed on top of MAX_OP_BYTES.
const OP_ENVELOPE_BYTES: usize = 256;
/// Widget commands waiting for a frame that mounts their target: at most
/// one tick's worth of requests.
const MAX_PENDING_WIDGET_COMMANDS: usize = wire::MAX_REQUESTS;

/// Whether `target` is the whole authored path of a node mounted in
/// `root` (the ancestry `crate::render::enter_scope` walks). A view names a
/// command's target by the id it holds; the guest SDK, which lowered every
/// id above it, sends the whole path, so two scopes holding one id are two
/// targets and neither is taken for the other.
pub(crate) fn target_names_mounted_node(root: &wire::Node, target: &[wire::ElementIdWire]) -> bool {
    fn contains(
        node: &wire::Node,
        path: &mut Vec<wire::ElementIdWire>,
        target: &[wire::ElementIdWire],
    ) -> bool {
        let entered = crate::render::enter_scope(node, path);
        let found = entered && path == target
            || target.starts_with(path)
                && node
                    .children()
                    .iter()
                    .any(|child| contains(child, path, target));
        if entered {
            path.pop();
        }
        found
    }
    contains(root, &mut Vec::new(), target)
}

impl Guest {
    pub(crate) fn sync_theme(&mut self, dark: bool) {
        if self.theme_dark != Some(dark) {
            self.theme_dark = Some(dark);
            // Theme is driver state, shared by every view regardless of its
            // product-specific props stream. Latest appearance wins per tick.
            self.pending
                .retain(|event| !matches!(event, wire::Event::Theme { .. }));
            self.pending.push(wire::Event::Theme { dark });
        }
    }

    /// One redraw: tick if there is anything to deliver — or never was a
    /// first frame — answer the requests, and say whether the guest is due
    /// again at once. A guest with nothing to deliver is left alone: the
    /// tree the host has is the tree it would send.
    pub(crate) fn redraw(&mut self, props: &Option<Vec<u8>>) -> bool {
        if self.fault.is_some() {
            return false;
        }
        self.sync_props(props);
        self.pending.extend(self.inputs.drain());
        if let Err(error) = self.inputs.ready() {
            self.fault = Some(error);
            return false;
        }
        match self.replies.drain_into(&mut self.pending) {
            Ok(backlog) => crate::perf::record(self.perf_key(), "backlog", backlog as u64),
            Err(error) => {
                self.fault = Some(error);
                return false;
            }
        }
        self.pending
            .extend(kernel::ticked(&mut self.clocks, std::time::Instant::now()));
        // Queued results precede becoming visible, so a view can distinguish
        // arrivals while hidden from data received after it returns to screen.
        self.sync_visibility();
        if !self.offset_subscriptions.is_empty() {
            self.sync_offset(kernel::offset_minutes());
        }
        self.sync_route();
        if self.staged {
            // a replacement's first tree is already here; only its
            // requests and cancels are still to route
            self.staged = false;
        } else {
            let quiet = self.ticks > 0
                && !self.frame.busy
                && self.pending.is_empty()
                && !self.inputs.pending()
                && self.inputs.ready() != Ok(false);
            if quiet {
                return false;
            }
            self.tick();
            self.ticks += 1;
            crate::perf::count(self.perf_key(), "ticks", 1);
            if self.frame.busy {
                crate::perf::count(self.perf_key(), "busy_ticks", 1);
            }
        }
        for request in std::mem::take(&mut self.frame.requests) {
            self.answer(request, props);
        }
        // what those requests logged reaches the disk once, off this thread
        notify::center().lock().flush();
        for id in std::mem::take(&mut self.frame.cancels) {
            self.clipboard.cancel(id);
            self.widget_commands.retain(|(request, _)| *request != id);
            if self.props_subscription == Some(id) {
                self.props_subscription = None;
            }
            self.live_subscriptions.retain(|(live, _)| *live != id);
            self.visibility_subscriptions
                .retain(|subscription| *subscription != id);
            self.offset_subscriptions
                .retain(|subscription| *subscription != id);
            self.route_subscriptions
                .retain(|subscription| *subscription != id);
            // dropping the stream aborts it: the node socket goes with the
            // subscription the view abandoned
            self.tasks.retain(|(task, _)| *task != id);
            self.clocks.retain(|clock| clock.id != id);
        }
        self.fault.is_none()
            && (self.frame.busy
                || self.inputs.pending()
                || !self.pending.is_empty()
                || self.inputs.ready() == Ok(false))
    }

    /// Whether this view's activation is fresh, taking it: the gated call
    /// that asks first is the one the person's input admits.
    pub(crate) fn take_activation(&mut self) -> bool {
        self.activation
            .take()
            .is_some_and(|at| at.elapsed() <= super::ACTIVATION_EXPIRY)
    }

    /// Routes one request: the size cap, the manifest's capability gate,
    /// then the kernel contract (`kernel::answer`), then the three methods
    /// only this side has — `host.widget`, `host.session` answered from the
    /// props the app holds, `host.log` — and anything else is refused.
    pub(crate) fn answer(&mut self, request: wire::Request, props: &Option<Vec<u8>>) {
        let wire::Request { id, kind, payload } = request;
        // `op.submit` and `module.describe` carry an op inside a `methods::Call`
        let payload_limit = match kind.as_str() {
            "op.submit" | "module.describe" => MAX_OP_BYTES + OP_ENVELOPE_BYTES,
            _ => MAX_PAYLOAD_BYTES,
        };
        if payload.len() > payload_limit {
            self.refuse(
                id,
                refusal::TOO_LARGE,
                format!("`{kind}` carries more than {payload_limit} bytes"),
            );
            return;
        }
        let Some((capability, operation)) = Capability::of_kind(&kind) else {
            self.refuse(
                id,
                refusal::UNKNOWN_REQUEST,
                format!("unknown request `{kind}`"),
            );
            return;
        };
        // A view is untrusted code: it reaches only the methods its manifest
        // declares. Consent (media, notifications) is asked on top of this.
        if !self.capabilities.contains(&capability) {
            let name = capability.as_str();
            if !self.undeclared_logged.contains(&capability) {
                self.undeclared_logged.push(capability);
                tracing::warn!(
                    target: "ducktape::app",
                    module = self.module,
                    capability = name,
                    "view asked for a capability its manifest does not declare"
                );
            }
            self.refuse(
                id,
                refusal::UNDECLARED_CAPABILITY,
                format!("`{kind}` needs the `{name}` capability, which this view does not declare"),
            );
            return;
        }
        // the kernel contract first: what every view may ask, module-free
        if kernel::answer(self, capability, operation, id, &payload, props) {
            return;
        }
        match (capability, operation) {
            (Capability::Host, "widget") => self.widget_request(id, &payload),
            (Capability::Host, "session") => {
                self.props_subscription = Some(id);
                self.props_sent = None;
                self.sync_props(props);
            }
            (Capability::Host, "log") => match wire::methods::decode::<String>(&payload) {
                Ok(line) => {
                    tracing::debug!(
                        target: "ducktape::app",
                        module = self.module,
                        line,
                        "module view log"
                    );
                    self.reply(id, Ok(Vec::new()));
                }
                Err(error) => self.refuse(id, refusal::MALFORMED_REQUEST, error),
            },
            _ => self.refuse(
                id,
                refusal::UNKNOWN_REQUEST,
                format!("unknown request `{kind}`"),
            ),
        }
    }

    /// The host's own refusal, in the shape a guest gets a node's: a stable
    /// snake_case token it may branch on, and the sentence it may show.
    pub(crate) fn refuse(&mut self, id: u64, reason: &'static str, message: impl Into<String>) {
        self.reply(id, Err(wire::Error::new(reason, message)));
    }

    /// The key a command acts on, or `None` for the two that act on focus
    /// order rather than a node. Exhaustive by design: adding a command
    /// requires reviewing its scope.
    pub(crate) fn command_target(command: &wire::WidgetCommand) -> Option<&[wire::ElementIdWire]> {
        use wire::WidgetCommand as C;
        match command {
            C::FocusPrevious | C::FocusNext | C::FocusHandle { .. } => None,
            C::EditorAction { target, .. }
            | C::Focus { target }
            | C::CursorFront { target }
            | C::CursorEnd { target }
            | C::Cursor { target, .. }
            | C::SelectAll { target }
            | C::Select { target, .. }
            | C::Snap { target, .. }
            | C::SnapEnd { target }
            | C::ScrollTo { target, .. }
            | C::ScrollBy { target, .. } => Some(target),
        }
    }

    /// Whether a command moves the keyboard: the ones the seat runs only
    /// while its keys are free (`Seat::keys_free`). Exhaustive for the same
    /// reason as `command_target`.
    pub(crate) fn moves_keys(command: &wire::WidgetCommand) -> bool {
        use wire::WidgetCommand as C;
        match command {
            C::Focus { .. } | C::FocusHandle { .. } | C::FocusNext | C::FocusPrevious => true,
            C::EditorAction { .. }
            | C::CursorFront { .. }
            | C::CursorEnd { .. }
            | C::Cursor { .. }
            | C::SelectAll { .. }
            | C::Select { .. }
            | C::Snap { .. }
            | C::SnapEnd { .. }
            | C::ScrollTo { .. }
            | C::ScrollBy { .. } => false,
        }
    }

    /// Whether the typed path this command names is in the tree the guest is
    /// showing RIGHT NOW. A press is answered a frame or more after it was
    /// made, and a live view replaces its frame between the two — so the
    /// question a queued command has to pass is whether its target is still
    /// there, never whether the frame it was made against is still the
    /// current one.
    pub(crate) fn target_is_mounted(&self, command: &wire::WidgetCommand) -> bool {
        let Some(target) = Self::command_target(command) else {
            return true;
        };
        self.frame
            .root
            .as_ref()
            .is_some_and(|root| target_names_mounted_node(root, target))
    }

    pub(crate) fn widget_request(&mut self, id: u64, payload: &[u8]) {
        let admitted = (|| -> Result<wire::WidgetCommand, String> {
            let mut command: wire::WidgetCommand = wire::decode(payload)?;
            let exact_payload = wire::encoded_size(&command) == payload.len() as u64;
            if !exact_payload {
                return Err("widget request has trailing bytes".into());
            }
            command.validate()?;
            let queue_full = self.widget_commands.len() >= MAX_PENDING_WIDGET_COMMANDS;
            if queue_full {
                return Err("too many pending widget requests".into());
            }
            if !self.target_is_mounted(&command) {
                return Err("widget target is outside this guest tree".into());
            }
            Ok(command)
        })();
        match admitted {
            Ok(command) => self.widget_commands.push((id, command)),
            Err(error) => self.refuse(id, refusal::INVALID_WIDGET_COMMAND, error),
        }
    }

    /// How many of the queued widget commands can run now. They wait for
    /// native editor work to drain, since a caret or a selection is placed in
    /// the document as it stands. A Focus does not wait: a field the view
    /// just opened has to hold the keys typed into it before its document
    /// arrives.
    pub(crate) fn runnable_widget_commands(&self) -> usize {
        if !self.inputs.pending() {
            return self.widget_commands.len();
        }
        self.widget_commands
            .iter()
            .take_while(|(_, command)| matches!(command, wire::WidgetCommand::Focus { .. }))
            .count()
    }

    /// Called on this guest's mounted tree with the commands that can run
    /// now. A command whose target left the tree in the meantime is answered
    /// with that, never performed.
    pub(crate) fn execute_widget_commands(
        &mut self,
        mut execute: impl FnMut(wire::WidgetCommand) -> Result<Vec<u8>, String>,
    ) {
        let runnable = self.runnable_widget_commands();
        let commands: Vec<_> = self.widget_commands.drain(..runnable).collect();
        for (id, command) in commands {
            let result = match self.target_is_mounted(&command) {
                true => execute(command)
                    .map_err(|error| wire::Error::new(refusal::WIDGET_COMMAND_FAILED, error)),
                false => Err(wire::Error::new(
                    refusal::WIDGET_UNMOUNTED,
                    "widget target left the tree",
                )),
            };
            self.reply(id, result);
        }
    }

    pub(crate) fn reply(&mut self, id: u64, result: kernel::Answer) {
        self.pending.push(wire::Event::Response {
            id,
            result,
            done: true,
        });
    }

    /// One item on the subscription `id`: the stream stays open.
    pub(crate) fn stream_item(&mut self, id: u64, item: Vec<u8>) {
        self.pending.push(wire::Event::Response {
            id,
            result: Ok(item),
            done: false,
        });
    }

    pub(crate) fn report_display_truncation(&mut self) {
        let Some(generation) = self.installed_generation else {
            return;
        };
        for origin in self
            .display_diagnostics
            .observe(self.frame_reports)
            .into_iter()
            .flatten()
        {
            crate::perf::count(self.perf_key(), "truncations", 1);
            tracing::warn!(target: "ducktape::app", module = self.module, generation,
                reason = "display_text_truncated", origin, "module view display text truncated");
        }
    }

    /// One call into the module with the pending events, inside the budget.
    /// A trap ends the view; the widget shows the message in its place.
    /// Each stage is a perf sample under the seat's key: the call, the
    /// decode, the merge, the fuel and bytes each way, the frame's kind.
    pub(crate) fn tick(&mut self) {
        use crate::perf;
        let key = self.perf_key();
        let events = std::mem::take(&mut self.pending);
        let bytes = wire::encode(&events);
        perf::record(key, "events_bytes", bytes.len() as u64);
        self.editor_moved = false;
        arm(&mut self.store);
        let called = perf::time(key, "tick.call");
        let answer = self
            .exports
            .tick(&mut self.store, &bytes)
            .map_err(|error| first_line(&error));
        drop(called);
        if perf::on() {
            perf::record(key, "fuel.tick", self.fuel_used());
        }
        // `RUST_LOG=ducktape::perf=debug`: how much of the budget a tick took
        tracing::debug!(target: "ducktape::perf", module = self.module,
            used = self.fuel_used(), limit = FUEL_PER_TICK);
        let outcome = answer.and_then(|frame| {
            perf::record(key, "frame_bytes", frame.len() as u64);
            let _decoded = perf::time(key, "tick.decode");
            shape(&frame)
        });
        match outcome {
            Ok((mut frame, mut reports)) => {
                let kind = match (frame.root.is_some(), frame.unchanged) {
                    (true, _) => "frame.full",
                    (false, true) => "frame.unchanged",
                    (false, false) => "frame.patch",
                };
                perf::count(key, kind, 1);
                perf::record(key, "requests", frame.requests.len() as u64);
                perf::record(key, "cancels", frame.cancels.len() as u64);
                let inherits = frame.root.is_none();
                let mut previous = self.frame.root.take();
                let mut accepted = true;
                let merging = perf::time(key, "merge");
                let merged = merge(&mut previous, &mut frame)
                    .map_err(str::to_owned)
                    .and_then(|changed| {
                        if changed.0
                            && let Some(root) = &frame.root
                        {
                            self.inputs.validate(root)?;
                        }
                        Ok(changed)
                    });
                drop(merging);
                match merged {
                    Ok((false, _)) => {}
                    Ok((true, report)) => {
                        reports.local.merge(report);
                        self.frame_rev += 1;
                        if let Some(root) = &mut frame.root {
                            if let Err(error) = self.inputs.replace(root) {
                                self.fault = Some(error);
                            }
                            // The guest remembers its tree without the
                            // picture bytes; the tree its patches build on
                            // has to be that one.
                            if self.pictures.adopt(root, self.module) {
                                self.pending.push(wire::Event::Resync);
                            }
                            // O(n) over the tree: only with perf on, and
                            // only on a tick that changed it
                            if perf::on() {
                                let mut nodes = 0;
                                root.for_each_mut(&mut |_| nodes += 1);
                                perf::gauge(key, "nodes", nodes);
                                perf::gauge(key, "picture_bytes", self.pictures.bytes());
                            }
                        }
                    }
                    // Preserve accepted document state while requesting a full tree.
                    Err(refused) => {
                        accepted = false;
                        frame.root = previous;
                        tracing::warn!(
                            target: "ducktape::app",
                            module = self.module,
                            reason = "module_view_patch_refused",
                            error = refused,
                            "module view patch refused"
                        );
                        self.frame_rev += 1;
                        self.pending.push(wire::Event::Resync);
                    }
                }
                if accepted {
                    if inherits {
                        reports.inherit(self.frame_reports);
                    }
                    self.frame_reports = reports;
                    self.report_display_truncation();
                    match self.inputs.frame(&frame) {
                        Ok(moved) => self.editor_moved = moved,
                        Err(error) => self.fault = Some(error),
                    }
                    self.pending.extend(self.inputs.drain());
                }
                self.frame = frame;
                if perf::on() {
                    perf::gauge(
                        key,
                        "memory",
                        self.exports.memory.data_size(&self.store) as u64,
                    );
                }
            }
            Err(trap) => {
                perf::count(key, "faults", 1);
                let reason = panic_message(&mut self.store).unwrap_or(trap);
                tracing::warn!(
                    target: "ducktape::app",
                    module = self.module,
                    reason = "module_view_trapped",
                    error = %reason,
                    "module view ended"
                );
                self.fault = Some(reason);
                self.frame_rev += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(id: &str, children: Vec<wire::Node>) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: Some(wire::ElementIdWire::Name(id.into())),
            style: Default::default(),
            interactivity: Default::default(),
            children,
        })
    }

    fn editor(id: &str) -> wire::Node {
        wire::Node::Editor {
            binding: None,
            id: wire::ElementIdWire::Name(id.into()),
            style: Default::default(),
            placeholder: String::new(),
            label: None,
            document: wire::editor_document::EditorDocumentRef {
                document: "doc".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: wire::EditorCursor::default(),
                byte_len: 0,
            },
            on_document: 0,
            editable: true,
        }
    }

    /// An editor five named ancestors deep, the way a composer mounts in a
    /// real view; the editor is targeted by its own key alone.
    fn chat_shaped_tree() -> wire::Node {
        container(
            "chat-viewport",
            vec![container(
                "chat-root",
                vec![container(
                    "chat-panes",
                    vec![container(
                        "chat-room",
                        vec![container(
                            "draft-general",
                            vec![editor("draft-general/editor")],
                        )],
                    )],
                )],
            )],
        )
    }

    /// The guest SDK names a nested editor by its whole path, which the
    /// host finds; the editor's own key alone names no node, since a second
    /// composer could hold it too.
    #[test]
    fn a_target_names_its_editor_by_the_whole_path() {
        let tree = chat_shaped_tree();
        let name = |name: &str| wire::ElementIdWire::Name(name.to_owned().into());
        let whole = [
            "chat-viewport",
            "chat-root",
            "chat-panes",
            "chat-room",
            "draft-general",
            "draft-general/editor",
        ]
        .map(name);
        assert!(target_names_mounted_node(&tree, &whole));
        assert!(!target_names_mounted_node(
            &tree,
            &[name("draft-general/editor")]
        ));
        assert!(!target_names_mounted_node(&tree, &whole[1..]));
    }

    #[test]
    fn an_unrelated_key_is_still_refused() {
        let tree = chat_shaped_tree();
        let target = [wire::ElementIdWire::Name("some-other-editor".into())];
        assert!(!target_names_mounted_node(&tree, &target));
    }

    #[test]
    fn an_empty_root_mounts_nothing() {
        let target = [wire::ElementIdWire::Name("draft-general/editor".into())];
        assert!(!target_names_mounted_node(&wire::Node::empty(), &target));
    }
}
