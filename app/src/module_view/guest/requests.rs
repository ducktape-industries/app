use super::*;

impl Guest {
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
        if let Err(error) = self.replies.drain_into(&mut self.pending) {
            self.fault = Some(error);
            return false;
        }
        self.pending
            .extend(kernel::ticked(&mut self.clocks, std::time::Instant::now()));
        // Queued results precede becoming visible, so a view can distinguish
        // arrivals while hidden from data received after it returns to screen.
        self.sync_visibility();
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
        }
        for (nth, request) in std::mem::take(&mut self.frame.requests)
            .into_iter()
            .enumerate()
        {
            match nth < MAX_REQUESTS_PER_TICK {
                true => self.answer(request, props),
                false => self.refuse(request.id, "tick_limit", "too many requests this tick"),
            }
        }
        self.user_activation = None;
        for id in std::mem::take(&mut self.frame.cancels) {
            self.filesystem.cancel(id);
            self.media.cancel(id);
            self.widget_commands.retain(|(request, _)| *request != id);
            if self.props_subscription == Some(id) {
                self.props_subscription = None;
            }
            self.live_subscriptions.retain(|(live, _)| *live != id);
            self.visibility_subscriptions
                .retain(|subscription| *subscription != id);
            // dropping the stream aborts it: the node socket goes with the
            // subscription the view abandoned
            self.tasks.retain(|(task, _)| *task != id);
            self.clocks.retain(|clock| clock.id != id);
            // a chord is given back with the subscription its presses were
            // arriving on, or the next view could never claim it
            let dropped: Vec<String> = self
                .chords
                .iter()
                .filter(|(subscription, _)| *subscription == id)
                .map(|(_, chord)| chord.clone())
                .collect();
            self.chords.retain(|(subscription, _)| *subscription != id);
            for chord in dropped {
                release_chord(&chord, self.module);
            }
        }
        self.fault.is_none()
            && (self.frame.busy
                || self.inputs.pending()
                || !self.pending.is_empty()
                || self.inputs.ready() == Ok(false))
    }

    /// Routes one request: the props subscription is answered from what the
    /// app holds, an intent the module declares goes to the app, the log
    /// goes to the log, and anything else is refused.
    pub(crate) fn answer(&mut self, request: wire::Request, props: &Option<Vec<u8>>) {
        let wire::Request { id, kind, payload } = request;
        let payload_limit = match kind.as_str() {
            "op.submit_bytes" => MAX_OP_BYTES.div_ceil(3) * 4 + 256,
            _ => MAX_PAYLOAD_BYTES,
        };
        if payload.len() > payload_limit {
            self.refuse(
                id,
                "too_large",
                format!("`{kind}` carries more than {payload_limit} bytes"),
            );
            return;
        }
        let (capability, operation) = kind.split_once('.').unwrap_or((kind.as_str(), ""));
        // the kernel contract first: what every view may ask, module-free
        if kernel::answer(self, capability, operation, id, &payload) {
            return;
        }
        let own = capability == self.module;
        match (capability, operation) {
            ("host", "widget") => self.widget_request(id, &payload),
            _ if (own || capability == "host") && operation == "props" => {
                self.props_subscription = Some(id);
                self.props_sent = None;
                self.sync_props(props);
            }
            ("host", "log") => {
                tracing::debug!(
                    target: "ducktape::app",
                    module = self.module,
                    line = %String::from_utf8_lossy(&payload),
                    "module view log"
                );
                self.reply(id, Ok(Vec::new()));
            }
            _ => self.refuse(id, "unknown_request", format!("unknown request `{kind}`")),
        }
    }

    /// The host's own refusal, in the shape a guest gets a node's: a stable
    /// snake_case token it may branch on, and the sentence it may show.
    pub(crate) fn refuse(&mut self, id: u64, reason: &'static str, message: impl Into<String>) {
        self.reply(id, Err(wire::Refusal::new(reason, message)));
    }

    /// The key a command acts on, or `None` for the two that act on focus
    /// order rather than a node. Exhaustive by design: adding a command
    /// requires reviewing its scope.
    pub(crate) fn command_target(command: &wire::WidgetCommand) -> Option<&str> {
        use wire::WidgetCommand as C;
        match command {
            C::FocusPrevious | C::FocusNext => None,
            C::EditorAction { target, .. }
            | C::Focus { target }
            | C::Focused { target }
            | C::CursorFront { target }
            | C::CursorEnd { target }
            | C::Cursor { target, .. }
            | C::SelectAll { target }
            | C::Select { target, .. }
            | C::Snap { target, .. }
            | C::SnapEnd { target }
            | C::ScrollTo { target, .. }
            | C::ScrollToKey { target, .. }
            | C::ScrollBy { target, .. } => Some(target),
        }
    }

    /// Whether the key this command names is in the tree the guest is
    /// showing RIGHT NOW. A press is answered a frame or more after it was
    /// made, and a live view replaces its frame between the two — so the
    /// question a queued command has to pass is whether its target is still
    /// there, never whether the frame it was made against is still the
    /// current one.
    pub(crate) fn target_is_mounted(&self, command: &wire::WidgetCommand) -> bool {
        fn contains(node: &wire::Node, target: &str) -> bool {
            node.key() == Some(target)
                || node.children().iter().any(|child| contains(child, target))
        }
        let Some(target) = Self::command_target(command) else {
            return true;
        };
        self.frame
            .root
            .as_ref()
            .is_some_and(|root| contains(root, target))
    }

    pub(crate) fn widget_request(&mut self, id: u64, payload: &[u8]) {
        let admitted = (|| -> Result<wire::WidgetCommand, String> {
            let mut command: wire::WidgetCommand = wire::decode(payload)?;
            let exact_payload = wire::encoded_size(&command) == payload.len() as u64;
            if !exact_payload {
                return Err("widget request has trailing bytes".into());
            }
            command.validate()?;
            let queue_full = self.widget_commands.len() >= MAX_REQUESTS_PER_TICK;
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
            Err(error) => self.refuse(id, "invalid_widget_command", error),
        }
    }

    /// How many of the queued widget commands can run now. They wait for
    /// native editor work to drain, since a caret or a selection is placed in
    /// the document as it stands. A Focus does not wait: a field the view
    /// just opened has to hold the keys typed into it before its document
    /// arrives. A rich editor's Focus still waits — it puts the caret in a
    /// block of its document, and has none until the document is here.
    pub(crate) fn runnable_widget_commands(&self) -> usize {
        if !self.inputs.pending() {
            return self.widget_commands.len();
        }
        fn rich(node: &wire::Node, target: &str) -> bool {
            match node {
                wire::Node::Editor { key, options, .. } if key == target => options.rich.is_some(),
                node => node.children().iter().any(|child| rich(child, target)),
            }
        }
        self.widget_commands
            .iter()
            .take_while(|(_, command)| {
                matches!(command, wire::WidgetCommand::Focus { target }
                    if !self.frame.root.as_ref().is_some_and(|root| rich(root, target)))
            })
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
                    .map_err(|error| wire::Refusal::new("widget_command_failed", error)),
                false => Err(wire::Refusal::new(
                    "widget_unmounted",
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
            tracing::warn!(target: "ducktape::app", module = self.module, generation,
                reason = "display_text_truncated", origin, "module view display text truncated");
        }
    }

    /// One call into the module with the pending events, inside the budget.
    /// A trap ends the view; the widget shows the message in its place.
    pub(crate) fn tick(&mut self) {
        let events = std::mem::take(&mut self.pending);
        let bytes = wire::encode(&events);
        arm(&mut self.store);
        let outcome = self
            .exports
            .tick(&mut self.store, &bytes)
            .map_err(|error| first_line(&error))
            .and_then(|frame| shape(&frame));
        match outcome {
            Ok((mut frame, mut reports)) => {
                let inherits = frame.root.is_none();
                let mut previous = self.frame.root.take();
                let mut accepted = true;
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
                match merged {
                    Ok((false, _)) => {}
                    Ok((true, report)) => {
                        reports.local.merge(report);
                        self.frame_rev += 1;
                        if let Some(root) = &mut frame.root {
                            if let Err(error) = self.inputs.replace(root) {
                                self.fault = Some(error);
                            }
                            self.pictures.adopt(root);
                            // The guest remembers its tree without the
                            // picture bytes; the tree its patches build on
                            // has to be that one.
                            root.for_each_mut(&mut |node| match node {
                                wire::Node::Svg { bytes, .. } => *bytes = None,
                                wire::Node::Image { data, .. }
                                | wire::Node::ImageViewer { data, .. } => *data = None,
                                _ => {}
                            });
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
                    if let Err(error) = self.inputs.frame(&frame) {
                        self.fault = Some(error);
                    }
                    self.pending.extend(self.inputs.drain());
                }
                self.frame = frame;
            }
            Err(trap) => {
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
