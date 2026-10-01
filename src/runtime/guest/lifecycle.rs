use super::*;

/// Ticks a replacement gets to publish its tree and finish its document
/// transfers. The host fetches documents one at a time, and a view answers
/// the request with `Begin`, then one chunk a tick, then `Complete`, and
/// hears the acknowledgement on one more tick; the last tick is the quiet
/// one that shows the transfer done.
const FIRST_FRAME_TICK_LIMIT: usize = wire::editor_document::MAX_EDITOR_DOCUMENTS
    * (wire::editor_document::MAX_EDITOR_CHUNKS + 3)
    + 1;
/// Tables are allocated eagerly at their declared minimum, before any fuel
/// or memory limit is consulted; a view is one core instance and one memory.
const MAX_TABLES: usize = 4;
const MAX_TABLE_ELEMENTS: usize = 1 << 20;
/// How much of a panic message the host keeps: this many bytes read from
/// the guest, the first line, at most this many chars.
const MAX_PANIC_BYTES: u32 = 1024;

impl Guest {
    /// Reusing identical code still retires work started on the old connection.
    pub(crate) fn reconnect(&mut self, revision: u64) {
        if self.connection_rev == revision {
            return;
        }
        let ids: Vec<_> = self.tasks.iter().map(|(id, _)| *id).collect();
        self.tasks.clear();
        self.clipboard = Default::default();
        for id in ids {
            self.refuse(id, refusal::STALE_CONNECTION, "network connection changed");
        }
        self.connection_rev = revision;
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
                let (name, _, min_width) = manifest_of(&bytes);
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
            let (name, capabilities, min_width) = manifest_of(&view_bytes);
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
            fresh.min_width = min_width;
            fresh.deployed(hash);
            let fresh = match &mut against {
                Some((alive, ticks)) => {
                    Self::replacement(fresh, alive, ticks, mounted, &shown, timing)?
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
    /// replacement: its snapshot, taken under the seat's lock against the
    /// very instance the load saw and held to `MAX_SNAPSHOT_BYTES`, is
    /// restored — or, when the guest refuses it as not its own, `init` runs
    /// in its place — and the first tree is verified. `ticks` becomes the
    /// count the snapshot was taken at; a view that never ticked has no
    /// state to carry and only `init`s.
    pub(super) fn replacement(
        mut fresh: Self,
        alive: &Arc<()>,
        ticks: &mut u64,
        mounted: &Arc<Mutex<Mounted>>,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let snapshot = {
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
            let preserve = *ticks > 0;
            if preserve && !old.settled() {
                return Err(Failure::Refused(
                    "the view has pending work; its replacement waits".into(),
                ));
            }
            if preserve {
                let taken = Instant::now();
                let snapshot = old.snapshot();
                timing.snapshot = taken.elapsed();
                let key = fresh.perf_key();
                crate::perf::record(key, "snapshot", timing.snapshot.as_micros() as u64);
                crate::perf::record(key, "fuel.snapshot", old.fuel_used());
                let snapshot = snapshot
                    .map_err(Failure::Trapped)?
                    .map_err(Failure::Refused)?;
                timing.snapshot_bytes = snapshot.len();
                crate::perf::gauge(key, "snapshot_bytes", snapshot.len() as u64);
                Some(snapshot)
            } else {
                None
            }
        };
        match snapshot {
            Some(snapshot) => {
                let restored = Instant::now();
                let answered = fresh.restore(&snapshot, shown);
                timing.restore = restored.elapsed();
                let key = fresh.perf_key();
                crate::perf::record(key, "restore", timing.restore.as_micros() as u64);
                crate::perf::record(key, "fuel.restore", fresh.fuel_used());
                match answered.map_err(Failure::Trapped)? {
                    Restored::Carried => {}
                    Restored::Refused(refusal) => {
                        tracing::warn!(
                            target: "ducktape::app",
                            module = fresh.module,
                            hash = %crate::backend::hex_encode(fresh.hash.as_ref().map_or(&[][..], |hash| hash)),
                            reason = "snapshot_refused",
                            refusal = %refusal,
                            "view_state_dropped"
                        );
                        fresh.timed_init(shown, timing)?;
                    }
                }
            }
            None => fresh.timed_init(shown, timing)?,
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
    /// no request the host has yet to route, no trap. A replacement not
    /// yet redrawn is settled too: the only requests its first tree
    /// carries are the subscriptions its restore rebuilt, which its own
    /// replacement rebuilds again — a tab not shown between two
    /// deployments is not stuck on the first.
    pub(crate) fn settled(&self) -> bool {
        self.fault.is_none()
            && self.replies.fault().is_none()
            && self.pending.is_empty()
            && self.widget_commands.is_empty()
            && self.inputs.ready() == Ok(true)
            && !self.inputs.pending()
            && !self.frame.busy
            && (self.staged || self.frame.requests.is_empty())
    }

    /// The guest's state, or its own word that it cannot hand it over now.
    /// A trap ends the instance as one in `tick` does: the fault keeps it
    /// from being entered again.
    pub(crate) fn snapshot(&mut self) -> Result<Result<Vec<u8>, String>, String> {
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
            if requests.len() > MAX_REQUESTS_PER_TICK || cancels.len() > MAX_CANCELS_PER_TICK {
                return Err(format!(
                    "{shown}: replacement requests exceed the first-frame budget"
                ));
            }
            if self.inputs.ready()? && self.pending.is_empty() {
                break;
            }
        }
        if !self.inputs.ready()? || !self.pending.is_empty() {
            return Err(format!(
                "{shown}: replacement document transfer did not complete"
            ));
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
        (guest.name, guest.capabilities, guest.min_width) = manifest_of(bytes);
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
            user_activation: None,
            module,
            instance: 0,
            name: String::new(),
            capabilities: Vec::new(),
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
            inputs: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                EditorStore::new(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            },
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
