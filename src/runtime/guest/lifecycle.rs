use super::*;

/// Ticks a replacement gets to publish its tree: the tree itself, the
/// resync a picture it names by hash alone may cost it (`Pictures::adopt`),
/// and the quiet tick that shows nothing more is owed.
const FIRST_FRAME_TICK_LIMIT: usize = 3;
/// Tables are allocated eagerly at their declared minimum, before any fuel
/// or memory limit is consulted; a view is one core instance and one memory.
const MAX_TABLES: usize = 4;
const MAX_TABLE_ELEMENTS: usize = 1 << 20;
/// How much of a panic message the host keeps: this many bytes read from
/// the guest, the first line, at most this many chars.
const MAX_PANIC_BYTES: u32 = 1024;

impl Guest {
    /// Reusing identical code over a new connection: the node subscriptions
    /// go on, opened on it, and the rest of the work started on the old one
    /// is retired (`kernel::reconnected`).
    pub(crate) fn reconnect(&mut self, connection: &Connection) {
        if self.connection_rev == connection.rev {
            return;
        }
        self.connection_rev = connection.rev;
        self.clipboard = Default::default();
        kernel::reconnected(self, connection);
    }

    /// A view comes from the roster's `code` for `module` on the connected
    /// node — nothing else, so with no node there is nothing to load yet,
    /// and no file is ever opened for it unless the developer's override
    /// ([`override_views_from`]) supplies one. A seat no pane holds yet
    /// (instance 0) stops at the compiled code: `Compiled`, its manifest
    /// read, nothing instantiated or run; the pane that claims it loads it
    /// again, past that. Over an empty seat a pane holds the view
    /// is `init`ed. With a view of the module already drawn, the same code
    /// is `Unchanged`, and different code is prepared as its replacement
    /// (`replacement`): instantiated without `init`, restored from the drawn
    /// view's snapshot, its first tree verified, and handed to the seat to
    /// swap in. Overridden, Missing, Compiled, Ready and Failed each log one
    /// `view_source` line here (the seat logs `Swapped`); every load that
    /// got past the roster fills `timing`, which the seat logs as one
    /// `view_load` line once the load is installed. Each stage is also a
    /// sample under the seat's perf key.
    pub(crate) fn load(
        module: &'static str,
        code: Option<(::abi::BlobId, bool)>,
        asked_of: &Connection,
        generation: u64,
        mounted: &Arc<Mutex<Mounted>>,
        timing: &mut LoadTiming,
    ) -> Result<Loaded, Unloaded> {
        let before_any_candidate = |failure: Failure| Unloaded {
            hash: None,
            failure,
        };
        let logged = |hash: Option<&[u8; 32]>, state: &str, reason: &str| {
            log_source(module, hash, state, generation, reason);
        };
        let instance = lock(mounted).instance;
        if let Some(path) = view_override(module) {
            let reason = format!("DUCKTAPE_VIEWS_DIR developer override: {}", path.display());
            logged(None, "Overridden", &reason);
            if instance == 0 {
                let shown = path.display().to_string();
                let bytes = std::fs::read(&path).map_err(|error| {
                    before_any_candidate(Failure::Refused(format!("{shown}: {error}")))
                })?;
                Self::compile(&bytes, &shown).map_err(before_any_candidate)?;
                let (name, _, min_width, _) = manifest_of(&bytes);
                return Ok(Loaded::Compiled { name, min_width });
            }
            return Self::load_from(module, &path)
                .map(|guest| Loaded::Fresh(Box::new(guest)))
                .map_err(|reason| before_any_candidate(Failure::Refused(reason)));
        }
        let Some(client) = asked_of.client.as_ref() else {
            let reason = "not connected to a node yet";
            logged(None, "Failed", reason);
            return Err(before_any_candidate(Failure::Unreachable(
                reason.to_owned(),
            )));
        };
        let Some((code, bare)) = code else {
            let reason = format!("this network's roster does not list {module}");
            logged(None, "Failed", &reason);
            return Err(before_any_candidate(Failure::NotListed(reason)));
        };
        let runtime = handle();
        timing.started = Some(Instant::now());
        timing.path = "first";
        let key = crate::perf::Key::View { module, instance };
        let show = |stage: Slot| lock(mounted).show(generation, stage);
        show(Slot::Fetching {
            received: 0,
            total: None,
        });
        let fetched = Instant::now();
        let bytes = runtime.block_on(crate::backend::views::view_of(client, &code, bare));
        timing.fetch = fetched.elapsed();
        crate::perf::record(key, "fetch", timing.fetch.as_micros() as u64);
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                let failure = Failure::of(error);
                logged(None, "Failed", &failure.to_string());
                timing.outcome = "Failed";
                return Err(before_any_candidate(failure));
            }
        };
        let Some((view_bytes, blob)) = bytes else {
            let hash = code_digest(&code);
            logged(Some(&hash), "Missing", "");
            timing.hash = Some(hash);
            timing.outcome = "Missing";
            return Ok(Loaded::Empty);
        };
        timing.blob = blob;
        crate::perf::count(
            key,
            match blob {
                "disk" => "fetch.disk",
                _ => "fetch.node",
            },
            1,
        );
        crate::perf::gauge(key, "view_bytes", view_bytes.len() as u64);
        let hash: [u8; 32] = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&view_bytes).into()
        };
        // the first six bytes: how a view is named in logs and errors
        let shown = format!("{module} view @ {}", crate::backend::hex_encode(&hash[..6]));
        let outcome = (|| -> Result<Loaded, Failure> {
            // the instance in the slot, if the deployment is a new one for
            // it: the replacement is seated only against that very
            // instance at that very tick count
            let mut against = {
                let locked = lock(mounted);
                match &locked.slot {
                    Slot::Ready(old) if old.hash == Some(hash) => return Ok(Loaded::Unchanged),
                    Slot::Ready(old) => Some((old.alive.clone(), old.ticks)),
                    _ => None,
                }
            };
            if against.is_some() {
                timing.path = "swap";
            }
            show(Slot::Compiling);
            let compiled = Instant::now();
            let code = Self::compile_sourced(&view_bytes, &shown);
            timing.compile = compiled.elapsed();
            crate::perf::record(key, "compile", timing.compile.as_micros() as u64);
            let (code, source) = code?;
            timing.code = source;
            crate::perf::count(
                key,
                match source {
                    "memory" => "compile.memory",
                    "disk" => "compile.disk",
                    _ => "compile.cold",
                },
                1,
            );
            let (name, capabilities, min_width, targets) = manifest_of(&view_bytes);
            // a seat no pane holds stops here: the rail reads its name off
            // the manifest, and the pane that claims it starts it
            if instance == 0 && against.is_none() {
                return Ok(Loaded::Compiled { name, min_width });
            }
            let instantiated = Instant::now();
            let mut fresh = Self::instantiate(module, &code, &shown).map_err(Failure::Refused)?;
            timing.instantiate = instantiated.elapsed();
            fresh.instance = instance;
            crate::perf::record(key, "instantiate", timing.instantiate.as_micros() as u64);
            crate::perf::record(key, "fuel.instantiate", fresh.fuel_used());
            fresh.name = name;
            fresh.capabilities = capabilities;
            fresh.targets = targets;
            fresh.min_width = min_width;
            fresh.deployed(hash);
            let fresh = match &mut against {
                Some((alive, ticks)) => {
                    Self::replacement(fresh, alive, ticks, mounted, &code, &shown, timing)?
                }
                None => {
                    fresh.timed_init(&shown, timing)?;
                    fresh
                }
            };
            Ok(match against {
                Some((alive, ticks)) => Loaded::Swap {
                    fresh: Box::new(fresh),
                    alive,
                    ticks,
                    code,
                    shown,
                },
                None => Loaded::Fresh(Box::new(fresh)),
            })
        })();
        timing.hash = Some(hash);
        timing.outcome = match &outcome {
            Ok(Loaded::Fresh(_)) => {
                logged(Some(&hash), "Ready", "");
                "Ready"
            }
            Ok(Loaded::Unchanged) => "Unchanged",
            Ok(Loaded::Compiled { .. }) => {
                logged(Some(&hash), "Compiled", "");
                "Compiled"
            }
            Ok(_) => "Swap",
            Err(failure) => {
                logged(Some(&hash), "Failed", &failure.to_string());
                "Failed"
            }
        };
        outcome.map_err(|failure| Unloaded {
            hash: Some(hash),
            failure,
        })
    }

    /// A once-valid view carries its state over to `fresh`, prepared as its
    /// replacement: what it hands over ([`Self::handover`]), under the
    /// seat's lock against the very instance the load saw, is restored, and
    /// the first tree is verified ([`Self::prepared`]). `ticks` becomes the
    /// count the handover was taken at.
    pub(crate) fn replacement(
        fresh: Self,
        alive: &Arc<()>,
        ticks: &mut u64,
        mounted: &Arc<Mutex<Mounted>>,
        code: &Module,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let state = {
            let mut locked = lock(mounted);
            let Slot::Ready(old) = &mut locked.slot else {
                return Err(Failure::Refused(
                    "the view left while its replacement was prepared".into(),
                ));
            };
            if !Arc::ptr_eq(alive, &old.alive) {
                return Err(Failure::Refused(
                    "the view changed while its replacement was prepared".into(),
                ));
            }
            *ticks = old.ticks;
            old.handover(&fresh, timing)?
        };
        Self::prepared(fresh, state, code, shown, timing)
    }

    /// [`Self::replacement`] once more, on a new instance of `like`'s
    /// `code`, against `old` as it is now: the seat calls it under its lock
    /// when the view ticked or took work while `like` was prepared.
    pub(crate) fn prepared_again(
        old: &mut Self,
        like: &Self,
        code: &Module,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let fresh = like.sibling(code, shown).map_err(Failure::Refused)?;
        let state = old.handover(&fresh, timing)?;
        Self::prepared(fresh, state, code, shown, timing)
    }

    /// What this drawn view hands `to`, its replacement: its snapshot, held
    /// to `MAX_SNAPSHOT_BYTES`, or none. A view that never ticked has no
    /// state. One that cannot hand its state over — it stopped, its state
    /// is past the budget, or its snapshot traps — never will: `to` starts
    /// clean, with a warn saying why. One with work unfinished refuses the
    /// replacement until its next turns finish it, and the seat asks again:
    /// the host's own (an event, a widget command, an editor transfer), or
    /// the guest's, in its own words (busy, a request in flight).
    fn handover(&mut self, to: &Self, timing: &mut LoadTiming) -> Result<Option<Vec<u8>>, Failure> {
        if self.ticks == 0 {
            return Ok(None);
        }
        if let Some(fault) = self.fault.clone().or_else(|| self.replies.fault()) {
            to.state_dropped("view_stopped", &fault);
            return Ok(None);
        }
        if !self.settled() {
            return Err(Failure::Refused(
                "the view has pending work; its replacement waits".into(),
            ));
        }
        let taken = Instant::now();
        let snapshot = self.snapshot();
        timing.snapshot = taken.elapsed();
        let key = to.perf_key();
        crate::perf::record(key, "snapshot", timing.snapshot.as_micros() as u64);
        crate::perf::record(key, "fuel.snapshot", self.fuel_used());
        match snapshot {
            Ok(Snapshot::Taken(snapshot)) => {
                timing.snapshot_bytes = snapshot.len();
                crate::perf::gauge(key, "snapshot_bytes", snapshot.len() as u64);
                Ok(Some(snapshot))
            }
            Ok(Snapshot::TooLarge) => {
                to.state_dropped(
                    "snapshot_too_large",
                    "the view's state is past the snapshot byte budget",
                );
                Ok(None)
            }
            Ok(Snapshot::Refused(refusal)) => Err(Failure::Refused(format!(
                "the view does not hand its state over yet ({refusal}); its replacement waits"
            ))),
            Err(trap) => {
                to.state_dropped("snapshot_trapped", &trap);
                Ok(None)
            }
        }
    }

    /// `fresh` started from `state`, a drawn view's handover: restored, or
    /// `init`ed in its place when there is none, when the guest refuses it
    /// as not its own, or when the restore traps (on a new instance of
    /// `code` then: a trapped one is not entered again). Its first tree is
    /// verified.
    fn prepared(
        mut fresh: Self,
        state: Option<Vec<u8>>,
        code: &Module,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let carried = match state {
            Some(state) => {
                let restored = Instant::now();
                let answered = fresh.restore(&state, shown);
                timing.restore = restored.elapsed();
                let key = fresh.perf_key();
                crate::perf::record(key, "restore", timing.restore.as_micros() as u64);
                crate::perf::record(key, "fuel.restore", fresh.fuel_used());
                match answered {
                    Ok(Restored::Carried) => true,
                    Ok(Restored::Refused(refusal)) => {
                        fresh.state_dropped("snapshot_refused", &refusal);
                        false
                    }
                    Err(trap) => {
                        fresh.state_dropped("restore_trapped", &trap);
                        fresh = fresh.sibling(code, shown).map_err(Failure::Refused)?;
                        false
                    }
                }
            }
            None => false,
        };
        if !carried {
            fresh.timed_init(shown, timing)?;
        }
        let framed = Instant::now();
        let frame = fresh.first_frame(shown);
        let first_frame = framed.elapsed();
        timing.first_frame = Some(first_frame);
        crate::perf::record(
            fresh.perf_key(),
            "first_frame",
            first_frame.as_micros() as u64,
        );
        frame.map_err(Failure::Trapped)?;
        Ok(fresh)
    }

    /// A new instance of `code`, named and manifested as `load` made this
    /// one, nothing run in it yet. Every field is named here, so one added
    /// to `Guest` is a compile error until it says whether a sibling
    /// carries it: what `load` set (the seat, the manifest, the deployment)
    /// is carried, and what the instance ran into starts fresh.
    fn sibling(&self, code: &Module, shown: &str) -> Result<Self, String> {
        let Self {
            module,
            instance,
            name,
            capabilities,
            targets,
            min_width,
            hash,
            connection_rev: _,
            activation: _,
            links: _,
            undeclared_logged: _,
            store: _,
            exports: _,
            pending: _,
            theme_dark: _,
            widget_commands: _,
            frame: _,
            frame_reports: _,
            display_diagnostics: _,
            installed_generation: _,
            frame_rev: _,
            ticks: _,
            pictures: _,
            props_subscription: _,
            props_sent: _,
            visible: _,
            visibility_change: _,
            visibility_subscriptions: _,
            offset_subscriptions: _,
            offset_sent: _,
            route_subscriptions: _,
            intents: _,
            replies: _,
            live_subscriptions: _,
            tasks: _,
            clipboard: _,
            clocks: _,
            fault: _,
            alive: _,
            staged: _,
        } = self;
        let mut sibling = Self::instantiate(module, code, shown)?;
        sibling.instance = *instance;
        sibling.name = name.clone();
        sibling.capabilities = capabilities.clone();
        sibling.targets = targets.clone();
        sibling.min_width = *min_width;
        sibling.hash = *hash;
        Ok(sibling)
    }

    /// The warn a replacement logs when it starts without the drawn view's
    /// state, `reason` naming why and `refusal` in the words that said so.
    fn state_dropped(&self, reason: &str, refusal: &str) {
        tracing::warn!(
            target: "ducktape::app",
            module = self.module,
            hash = %crate::backend::hex_encode(self.hash.as_ref().map_or(&[][..], |hash| hash)),
            reason,
            refusal = %refusal,
            "view_state_dropped"
        );
    }

    /// `init`, timed into `timing` and the registry, as a load runs it.
    fn timed_init(&mut self, shown: &str, timing: &mut LoadTiming) -> Result<(), Failure> {
        let started = Instant::now();
        let ran = self.init(shown);
        timing.init = started.elapsed();
        crate::perf::record(self.perf_key(), "init", timing.init.as_micros() as u64);
        crate::perf::record(self.perf_key(), "fuel.init", self.fuel_used());
        ran.map_err(Failure::Trapped)
    }

    pub(crate) fn deployed(&mut self, hash: [u8; 32]) {
        self.hash = Some(hash);
    }

    /// The load generation that seated this instance — zero for one built
    /// directly by a test — so a candidate still loading retires nothing
    /// of the seated view.
    pub(crate) fn seated_generation(&self) -> u64 {
        self.installed_generation.unwrap_or_default()
    }

    /// Everything this instance was asked to do is done: nothing pending,
    /// no request the host has yet to route, no trap. Busy (out of budget,
    /// ticking again soon) is the guest's to weigh: its snapshot says
    /// whether it can hand its state over mid-work. A replacement not
    /// yet redrawn is settled too: the only requests its first tree
    /// carries are the subscriptions its restore rebuilt, which its own
    /// replacement rebuilds again — a tab not shown between two
    /// deployments is not stuck on the first.
    pub(crate) fn settled(&self) -> bool {
        self.fault.is_none()
            && self.replies.fault().is_none()
            && self.pending.is_empty()
            && self.widget_commands.is_empty()
            && (self.staged || self.frame.requests.is_empty())
    }

    /// The guest's state, or why it does not hand it over now. A trap ends
    /// the instance as one in `tick` does: the fault keeps it from being
    /// entered again.
    pub(crate) fn snapshot(&mut self) -> Result<Snapshot, String> {
        arm(&mut self.store);
        self.exports.snapshot(&mut self.store).map_err(|error| {
            let reason = panic_message(&mut self.store).unwrap_or_else(|| first_line(&error));
            self.fault = Some(reason.clone());
            reason
        })
    }

    pub(crate) fn restore(&mut self, snapshot: &[u8], shown: &str) -> Result<Restored, String> {
        arm(&mut self.store);
        let answered = self
            .exports
            .restore(&mut self.store, snapshot)
            .map_err(|error| {
                let trap = panic_message(&mut self.store).unwrap_or_else(|| first_line(&error));
                format!("{shown}: restore trapped: {trap}")
            })?;
        Ok(match answered {
            Ok(()) => Restored::Carried,
            Err(refusal) => Restored::Refused(refusal),
        })
    }

    /// The view's `init` export: a fresh start, with no snapshot to restore.
    pub(crate) fn init(&mut self, shown: &str) -> Result<(), String> {
        arm(&mut self.store);
        if let Err(error) = self.exports.init(&mut self.store) {
            let trap = format!("{shown}: init trapped: {}", first_line(&error));
            return Err(panic_message(&mut self.store).unwrap_or(trap));
        }
        Ok(())
    }

    /// A restored instance's first tick: its whole tree, or it is no
    /// replacement. Its requests wait for the first redraw.
    pub(crate) fn first_frame(&mut self, shown: &str) -> Result<(), String> {
        let mut requests = Vec::new();
        let mut cancels = Vec::new();
        for _ in 0..FIRST_FRAME_TICK_LIMIT {
            self.tick();
            if let Some(fault) = &self.fault {
                return Err(format!("{shown}: {fault}"));
            }
            if self.frame.root.is_none() {
                return Err(format!(
                    "{shown}: the replacement did not publish a complete tree"
                ));
            }
            requests.append(&mut self.frame.requests);
            cancels.append(&mut self.frame.cancels);
            if requests.len() > wire::MAX_REQUESTS || cancels.len() > wire::MAX_CANCELS {
                return Err(format!(
                    "{shown}: replacement requests exceed the first-frame budget"
                ));
            }
            if self.pending.is_empty() {
                break;
            }
        }
        if !self.pending.is_empty() {
            return Err(format!("{shown}: the replacement did not settle"));
        }
        requests.retain(|request| !cancels.contains(&request.id));
        self.frame.requests = requests;
        self.frame.cancels = cancels;
        self.ticks += 1;
        self.staged = true;
        Ok(())
    }

    pub(crate) fn load_from(module: &'static str, path: &std::path::Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|error| format!("{shown}: {error}"))?;
        Self::from_bytes(module, &bytes, &shown)
    }

    /// The view instantiated and mounted; `shown` names it in errors.
    pub(crate) fn from_bytes(
        module: &'static str,
        bytes: &[u8],
        shown: &str,
    ) -> Result<Self, String> {
        let code = Self::compile(bytes, shown).map_err(|failure| failure.to_string())?;
        let mut guest = Self::instantiate(module, &code, shown)?;
        (
            guest.name,
            guest.capabilities,
            guest.min_width,
            guest.targets,
        ) = manifest_of(bytes);
        guest.init(shown)?;
        Ok(guest)
    }

    /// The view's bytes checked and compiled — the cranelift stage of
    /// a load, measured on its own.
    pub(crate) fn compile(bytes: &[u8], shown: &str) -> Result<Arc<Module>, Failure> {
        Self::compile_sourced(bytes, shown).map(|(code, _)| code)
    }

    /// [`Self::compile`], with where the code came from (`compiled_view`).
    fn compile_sourced(bytes: &[u8], shown: &str) -> Result<(Arc<Module>, &'static str), Failure> {
        // refused here, a manifest `manifest_of` would read as empty never
        // reaches a seat
        let manifest = view_wire::manifest::read_manifest(bytes).ok_or_else(|| {
            Failure::Refused(format!("{shown}: the view's manifest cannot be read"))
        })?;
        wire_id(&manifest.wire_id).map_err(|error| Failure::Wire(format!("{shown}: {error}")))?;
        compiled_view(bytes).map_err(|error| Failure::Refused(format!("{shown}: {error}")))
    }

    /// The module instantiated, its exports bound, nothing run yet: a
    /// fresh view is `init`ed, a replacement `restore`d.
    pub(crate) fn instantiate(
        module: &'static str,
        code: &Module,
        shown: &str,
    ) -> Result<Self, String> {
        let engine = engine();
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .memories(1)
            .instances(1)
            .tables(MAX_TABLES)
            .table_elements(MAX_TABLE_ELEMENTS)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(
            engine,
            HostState {
                limits,
                panic: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        // A view's one import is the panic hook's (`wire::abi`); anything
        // else the module asks for traps if it is ever called.
        let mut linker = Linker::<HostState>::new(engine);
        linker
            .func_wrap(
                wire::abi::IMPORT_MODULE,
                "panicked",
                |mut caller: Caller<'_, HostState>, ptr: u32, len: u32| {
                    let message = caller
                        .get_export("memory")
                        .and_then(|export| export.into_memory())
                        .and_then(|memory| {
                            let start = ptr as usize;
                            let bytes = memory.data(&caller).get(
                                start..start.saturating_add(len.min(MAX_PANIC_BYTES) as usize),
                            )?;
                            Some(String::from_utf8_lossy(bytes).into_owned())
                        })
                        .unwrap_or_default();
                    let line = message.lines().next().unwrap_or_default();
                    caller.data_mut().panic =
                        Some(line.chars().take(MAX_PANIC_BYTES as usize).collect());
                },
            )
            .map_err(|error| error.to_string())?;
        linker
            .define_unknown_imports_as_traps(code)
            .map_err(|error| error.to_string())?;
        arm(&mut store);
        let instance = linker
            .instantiate(&mut store, code)
            .map_err(|error| format!("{shown}: {}", first_line(&error)))?;
        let exports =
            Exports::bind(&mut store, &instance).map_err(|error| format!("{shown}: {error}"))?;
        Ok(Self {
            connection_rev: connection().lock().expect("views rpc").rev,
            activation: None,
            links: Vec::new(),
            module,
            instance: 0,
            name: String::new(),
            capabilities: Vec::new(),
            targets: Vec::new(),
            min_width: 0,
            undeclared_logged: Vec::new(),
            store,
            exports,
            pending: Vec::new(),
            theme_dark: None,
            widget_commands: Vec::new(),
            frame: wire::Frame::default(),
            frame_reports: Default::default(),
            display_diagnostics: Default::default(),
            installed_generation: None,
            frame_rev: 0,
            ticks: 0,
            pictures: Pictures::default(),
            props_subscription: None,
            props_sent: None,
            visible: false,
            visibility_change: None,
            visibility_subscriptions: Vec::new(),
            offset_subscriptions: Vec::new(),
            offset_sent: None,
            route_subscriptions: Vec::new(),
            intents: Vec::new(),
            replies: Arc::default(),
            live_subscriptions: Vec::new(),
            tasks: Vec::new(),
            clipboard: Default::default(),
            clocks: Vec::new(),
            fault: None,
            hash: None,
            alive: Arc::new(()),
            staged: false,
        })
    }

    /// Hands the guest the props the app holds, if they moved since the
    /// guest last saw them. Before the guest has subscribed they wait here.
    pub(crate) fn sync_props(&mut self, props: &Option<Vec<u8>>) {
        let Some(id) = self.props_subscription else {
            return;
        };
        if props.is_none() || *props == self.props_sent {
            return;
        }
        self.props_sent = props.clone();
        let props = props.clone().unwrap_or_default();
        self.stream_item(id, props);
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.visibility_change = Some(visible);
    }

    /// A route a link left for this module goes to its first route
    /// subscriber, once; with no subscriber it waits.
    pub(crate) fn sync_route(&mut self) {
        let Some(&id) = self.route_subscriptions.first() else {
            return;
        };
        if let Some(route) = crate::runtime::take_route(self.module) {
            self.stream_item(id, wire::methods::encode(&route));
        }
    }

    /// Hands every `host.offset` subscriber the reader's offset, if it
    /// moved since they last heard it.
    pub(crate) fn sync_offset(&mut self, minutes: i32) {
        if self.offset_sent == Some(minutes) {
            return;
        }
        self.offset_sent = Some(minutes);
        for id in self.offset_subscriptions.clone() {
            self.stream_item(id, wire::methods::encode(&minutes));
        }
    }

    pub(crate) fn sync_visibility(&mut self) {
        let Some(visible) = self.visibility_change.take() else {
            return;
        };
        for id in self.visibility_subscriptions.clone() {
            self.stream_item(id, wire::methods::encode(&visible));
        }
    }
}
