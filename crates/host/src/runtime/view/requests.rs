use super::*;

/// Whether `target` is the whole authored path of a node mounted in
/// `root` (the ancestry `crate::render::enter_scope` walks). A view names a
/// command's target by the id it holds; the view SDK, which lowered every
/// id above it, sends the whole path, so two scopes holding one id are two
/// targets and neither is taken for the other.
pub fn target_names_mounted_node(root: &wire::Node, target: &[wire::ElementIdWire]) -> bool {
    fn contains(
        node: &wire::Node,
        row: Option<usize>,
        path: &mut Vec<wire::ElementIdWire>,
        target: &[wire::ElementIdWire],
    ) -> bool {
        let entered = crate::render::enter_scope(node, row, path);
        let found = entered && path == target
            || target.starts_with(path)
                && node.children().iter().enumerate().any(|(at, child)| {
                    contains(child, wire::identity::row(node, at), path, target)
                });
        if entered {
            path.pop();
        }
        found
    }
    contains(root, None, &mut Vec::new(), target)
}

impl View {
    pub fn sync_theme(&mut self, dark: bool) {
        if self.theme_dark != Some(dark) {
            self.theme_dark = Some(dark);
            // Theme is driver state, shared by every view regardless of its
            // product-specific props stream. Latest appearance wins per tick.
            self.pending
                .retain(|event| !matches!(event, wire::Event::Theme { .. }));
            self.pending.push(wire::Event::Theme { dark });
        }
    }

    /// Whether the device's primary pointer is a finger (`OsFacts::
    /// touch_first`, a fact of the device, never of the last input): told
    /// ahead of the first tick, on change, and to a replacement before its
    /// first tree, so the view's first frame is at the right ladder.
    pub fn sync_pointer(&mut self, touch: bool) {
        if self.touch_sent != Some(touch) {
            self.touch_sent = Some(touch);
            self.pending
                .retain(|event| !matches!(event, wire::Event::Pointer { .. }));
            self.pending.push(wire::Event::Pointer { touch });
        }
    }

    /// The body the view is laid out in, as the pane has it before the
    /// draw: the view's `Window::viewport_size()`. Sent when it moved;
    /// the latest size wins the tick.
    pub fn sync_viewport(&mut self, width: f32, height: f32) {
        if self.viewport_sent != Some((width, height)) {
            self.viewport_sent = Some((width, height));
            self.pending
                .retain(|event| !matches!(event, wire::Event::Viewport { .. }));
            self.pending.push(wire::Event::Viewport { width, height });
        }
    }

    /// The reader's UTC offset in minutes, sent when it moved (a DST
    /// change): every time the view writes is in the reader's zone from
    /// its first frame on.
    pub fn sync_offset(&mut self, minutes: i32) {
        if self.offset_sent != Some(minutes) {
            self.offset_sent = Some(minutes);
            self.pending
                .retain(|event| !matches!(event, wire::Event::Offset { .. }));
            self.pending.push(wire::Event::Offset { minutes });
        }
    }

    /// One redraw, its tick landed at once: [`Self::begin_redraw`], the
    /// frame taken in, [`Self::finish_redraw`], as a seat's turn runs it on
    /// the wasmtime runner. Whether the view is due again at once.
    #[cfg(test)]
    pub fn redraw(&mut self, props: &Option<Vec<u8>>) -> bool {
        match self.begin_redraw(props) {
            Redraw::Quiet => false,
            Redraw::Ready => self.finish_redraw(props),
            Redraw::Ticking => {
                let landed = self.at_once();
                self.redraw_landed(landed);
                self.finish_redraw(props)
            }
        }
    }

    /// A redraw's first half: everything owed to the view gathered, and a
    /// tick begun if there is anything to deliver — or never was a first
    /// frame. A view with nothing to deliver is left alone (`Quiet`): the
    /// tree the host has is the tree it would send. While `Ticking`, the
    /// frame lands on the seat's `Landing`, at once or later; the seat
    /// hands it to [`Self::redraw_landed`] and then calls
    /// [`Self::finish_redraw`].
    pub fn begin_redraw(&mut self, props: &Option<Vec<u8>>) -> Redraw {
        if self.fault.is_some() {
            return Redraw::Quiet;
        }
        self.sync_props(props);
        let backlog = self.replies.drain_into(&mut self.pending);
        crate::perf::record(self.perf_key(), "backlog", backlog as u64);
        self.pending
            .extend(kernel::ticked(&mut self.clocks, std::time::Instant::now()));
        // Queued results precede becoming visible, so a view can distinguish
        // arrivals while hidden from data received after it returns to screen.
        self.sync_visibility();
        self.sync_offset(kernel::offset_minutes());
        self.sync_route();
        if self.staged {
            // a replacement's first tree is already here; only its
            // requests and cancels are still to route
            self.staged = false;
            return Redraw::Ready;
        }
        let quiet = self.ticks > 0 && !self.frame.busy && self.pending.is_empty();
        if quiet {
            return Redraw::Quiet;
        }
        self.begin_tick();
        Redraw::Ticking
    }

    /// A redraw's tick landed: its frame taken in, and counted.
    pub fn redraw_landed(&mut self, landed: Landed) {
        self.tick_landed(landed);
        self.ticks += 1;
        crate::perf::count(self.perf_key(), "ticks", 1);
        if self.frame.busy {
            crate::perf::count(self.perf_key(), "busy_ticks", 1);
        }
    }

    /// The answer to the call begun last, if it has landed; the seat takes
    /// it after `begin_redraw`, and waits on [`Self::landing`] when not.
    pub fn landed_now(&self) -> Option<Landed> {
        self.landing.try_recv().ok()
    }

    /// Whether a tick is in flight: begun, its frame not landed.
    pub fn ticking(&self) -> bool {
        self.ticking
    }

    /// The instance's answers, for the seat to wait on a tick in flight.
    pub fn landing(&self) -> async_channel::Receiver<Landed> {
        self.landing.clone()
    }

    /// A redraw's second half, after its tick landed (or none was due):
    /// the frame's requests answered and its cancels retired. Whether the
    /// view is due again at once.
    pub fn finish_redraw(&mut self, props: &Option<Vec<u8>>) -> bool {
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
            self.route_subscriptions
                .retain(|subscription| *subscription != id);
            // dropping the stream aborts it: the node socket goes with the
            // subscription the view abandoned
            self.tasks.retain(|(task, _)| *task != id);
            self.clocks.retain(|clock| clock.id != id);
        }
        self.fault.is_none() && (self.frame.busy || !self.pending.is_empty())
    }

    /// Whether this view's activation is fresh, taking it: the gated call
    /// that asks first is the one the person's input admits.
    pub fn take_activation(&mut self) -> bool {
        self.activation
            .take()
            .is_some_and(|at| at.elapsed() <= super::ACTIVATION_EXPIRY)
    }

    /// Routes one request: the manifest's capability gate,
    /// then the kernel contract (`kernel::answer`), then the three methods
    /// only this side has — `host.widget`, `host.session` answered from the
    /// props the app holds, `host.log` — and anything else is refused.
    pub fn answer(&mut self, request: wire::Request, props: &Option<Vec<u8>>) {
        let wire::Request { id, kind, payload } = request;
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
                    program = self.program,
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
        // the kernel contract first: what every view may ask, program-free
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
                        program = self.program,
                        line,
                        "program view log"
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

    /// The host's own refusal, in the shape a view gets a node's: a stable
    /// snake_case token it may branch on, and the sentence it may show.
    pub fn refuse(&mut self, id: u64, reason: &'static str, message: impl Into<String>) {
        self.reply(id, Err(wire::Error::new(reason, message)));
    }

    /// The key a command acts on, or `None` for the two that act on focus
    /// order rather than a node. Exhaustive by design: adding a command
    /// requires reviewing its scope.
    pub fn command_target(command: &wire::WidgetCommand) -> Option<&[wire::ElementIdWire]> {
        use wire::WidgetCommand as C;
        match command {
            C::FocusPrevious | C::FocusNext | C::FocusHandle { .. } => None,
            C::Replace { target, .. }
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
    pub fn moves_keys(command: &wire::WidgetCommand) -> bool {
        use wire::WidgetCommand as C;
        match command {
            C::Focus { .. } | C::FocusHandle { .. } | C::FocusNext | C::FocusPrevious => true,
            C::Replace { .. }
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

    /// Whether the typed path this command names is in the tree the view is
    /// showing RIGHT NOW. A press is answered a frame or more after it was
    /// made, and a live view replaces its frame between the two — so the
    /// question a queued command has to pass is whether its target is still
    /// there, never whether the frame it was made against is still the
    /// current one.
    pub fn target_is_mounted(&self, command: &wire::WidgetCommand) -> bool {
        let Some(target) = Self::command_target(command) else {
            return true;
        };
        self.frame
            .root
            .as_ref()
            .is_some_and(|root| target_names_mounted_node(root, target))
    }

    pub fn widget_request(&mut self, id: u64, payload: &[u8]) {
        let admitted = (|| -> Result<wire::WidgetCommand, String> {
            let mut command: wire::WidgetCommand = wire::decode(payload)?;
            let exact_payload = wire::encode(&command).len() == payload.len();
            if !exact_payload {
                return Err("widget request has trailing bytes".into());
            }
            command.validate()?;
            if !self.target_is_mounted(&command) {
                return Err("widget target is outside this view tree".into());
            }
            Ok(command)
        })();
        match admitted {
            Ok(command) => self.widget_commands.push((id, command)),
            Err(error) => self.refuse(id, refusal::INVALID_WIDGET_COMMAND, error),
        }
    }

    /// Called on this view's mounted tree with the queued commands. A
    /// command whose target left the tree in the meantime is answered with
    /// that, never performed.
    pub fn execute_widget_commands(
        &mut self,
        mut execute: impl FnMut(wire::WidgetCommand) -> Result<Vec<u8>, String>,
    ) {
        let commands: Vec<_> = self.widget_commands.drain(..).collect();
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

    pub fn reply(&mut self, id: u64, result: kernel::Answer) {
        self.pending.push(wire::Event::Response {
            id,
            result,
            done: true,
        });
    }

    /// One item on the subscription `id`: the stream stays open.
    pub fn stream_item(&mut self, id: u64, item: Vec<u8>) {
        self.pending.push(wire::Event::Response {
            id,
            result: Ok(item),
            done: false,
        });
    }

    pub fn report_display_truncation(&mut self) {
        let Some(generation) = self.installed_generation else {
            return;
        };
        if self.display_diagnostics.first_cut(&self.frame_reports) {
            let cuts = &self.frame_reports;
            crate::perf::count(self.perf_key(), "truncations", 1);
            tracing::warn!(target: "ducktape::app", program = self.program, generation,
                reason = "frame_cut", nodes = cuts.nodes, depth = cuts.depth,
                first = ?cuts.first, "program view frame cut by the host's sanitizer");
        }
    }

    /// One call into the view with the pending events, inside the budget.
    /// A trap ends the view; the widget shows the message in its place.
    /// Each stage is a perf sample under the seat's key: the call, the
    /// decode, the merge, the fuel and bytes each way, the frame's kind.
    #[cfg(test)]
    pub fn tick(&mut self) {
        self.begin_tick();
        let landed = self.at_once();
        self.tick_landed(landed);
    }

    /// A tick's first half: the events owed to the view handed to its
    /// instance. The frame lands on the seat's `Landing`, at once or later,
    /// and [`Self::tick_landed`] takes it.
    pub fn begin_tick(&mut self) {
        let key = self.perf_key();
        let events = std::mem::take(&mut self.pending);
        #[cfg(any(test, feature = "test-support"))]
        self.ticked_with.push(events.clone());
        let bytes = wire::encode(&events);
        crate::perf::record(key, "events_bytes", bytes.len() as u64);
        self.tick_call = crate::perf::time(key, "tick.call");
        self.ticking = true;
        if let Some(kept) = &mut self.kept {
            kept.batches.push(bytes.clone());
        }
        self.running.begin_tick(bytes);
    }

    /// A tick's second half: its frame landed, shaped and merged into the
    /// tree the host holds, or its fault ending the view.
    pub fn tick_landed(&mut self, landed: Landed) {
        use crate::perf;
        let key = self.perf_key();
        drop(self.tick_call.take());
        self.ticking = false;
        let answer = match landed {
            Landed::Frame(frame) => Ok(frame),
            Landed::Fault(fault) => Err(fault.to_string()),
            other => Err(format!("the view answered a tick with {other:?}")),
        };
        if perf::on() {
            perf::record(key, "fuel.tick", self.fuel_used());
        }
        // `RUST_LOG=ducktape::perf=debug`: how much of the budget a tick took
        tracing::debug!(target: "ducktape::perf", program = self.program,
            used = self.fuel_used(), limit = FUEL_PER_TICK);
        let outcome = answer.and_then(|frame| {
            perf::record(key, "frame_bytes", frame.len() as u64);
            let _decoded = perf::time(key, "tick.decode");
            shape(&frame, &mut self.styles)
        });
        let outcome = outcome.and_then(|shaped| {
            crate::render::web::declared(&shaped.0, &self.capabilities).map(|()| shaped)
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
                let merged = merge(&mut previous, &mut frame, &self.styles)
                    .map_err(|refused| refused.to_string());
                drop(merging);
                match merged {
                    Ok((false, _)) => {}
                    Ok((true, report)) => {
                        reports.merge(report);
                        self.frame_rev += 1;
                        if let Some(root) = &mut frame.root {
                            // The view remembers its tree without the
                            // picture bytes; the tree its patches build on
                            // has to be that one.
                            if self.pictures.adopt(root, self.program) {
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
                    // the tree shown stays while a full one is asked for
                    Err(refused) => {
                        accepted = false;
                        frame.root = previous;
                        tracing::warn!(
                            target: "ducktape::app",
                            program = self.program,
                            reason = "program_view_patch_refused",
                            error = refused,
                            "program view patch refused"
                        );
                        self.frame_rev += 1;
                        self.pending.push(wire::Event::Resync);
                    }
                }
                if accepted {
                    if inherits {
                        reports.merge(std::mem::take(&mut self.frame_reports));
                    }
                    self.frame_reports = reports;
                    self.report_display_truncation();
                    // the tooltip content built this tick goes to the tree
                    // beside its root; its pictures join the store like the
                    // tree's own, the budget held against the tree
                    for mut response in frame.tooltip_responses.drain(..) {
                        if let (Some(content), Some(root)) =
                            (&mut response.content, &mut frame.root)
                            && self.pictures.take(content, root, self.program)
                        {
                            self.pending.push(wire::Event::Resync);
                        }
                        self.tooltip_responses.push((response, self.styles.clone()));
                    }
                }
                self.frame = frame;
                if perf::on() {
                    perf::gauge(key, "memory", self.running.memory_bytes() as u64);
                }
            }
            Err(trap) => {
                perf::count(key, "faults", 1);
                let reason = trap;
                tracing::warn!(
                    target: "ducktape::app",
                    program = self.program,
                    reason = "program_view_trapped",
                    error = %reason,
                    "program view ended"
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
            style: crate::render::plain_style(),
            interactivity: Default::default(),
            children,
        })
    }

    fn field(id: &str) -> wire::Node {
        wire::Node::Field {
            id: wire::ElementIdWire::Name(id.into()),
            multiline: true,
            value: String::new(),
            cursor: Default::default(),
            generation: 1,
            revision: 0,
            tokens: Default::default(),
            claims: Default::default(),
            options: Default::default(),
            placeholder: String::new(),
            secure: false,
            on_change: Some(1),
            on_key: None,
            on_submit: None,
            style: crate::render::plain_style(),
        }
    }

    /// A field five named ancestors deep, the way a composer mounts in a
    /// real view; the field is targeted by its own key alone.
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
                            vec![field("draft-general/editor")],
                        )],
                    )],
                )],
            )],
        )
    }

    /// The view SDK names a nested field by its whole path, which the
    /// host finds; the field's own key alone names no node, since a second
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
