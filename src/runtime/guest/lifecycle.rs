use super::*;

impl Guest {
    /// Reusing identical code still retires work started on the old connection.
    pub(crate) fn reconnect(&mut self, revision: u64) {
        if self.connection_rev == revision {
            return;
        }
        let ids: Vec<_> = self.tasks.iter().map(|(id, _)| *id).collect();
        self.tasks.clear();
        self.filesystem = Default::default();
        self.media = Default::default();
        for id in ids {
            self.refuse(id, "stale_connection", "network connection changed");
        }
        self.connection_rev = revision;
    }

    /// A view comes from its registry entry's active deployment on the
    /// connected node — nothing else, so with no node there is nothing to
    /// load yet, and no file is ever opened for it unless the developer's
    /// override ([`override_views_from`]) supplies one. With a view of the module already
    /// drawn, the deployment's view is prepared as its replacement:
    /// instantiated without `init`, restored from the drawn view's snapshot,
    /// and its first tree verified — and the active code is read again
    /// right before it is handed over, so a deployment that moved meanwhile
    /// is not installed. Every outcome for a module-owned view is one
    /// `view_source` log line with stable fields.
    pub(crate) fn load(
        module: &'static str,
        asked_of: &Connection,
        generation: u64,
        mounted: &Arc<Mutex<Mounted>>,
    ) -> Result<Loaded, Unloaded> {
        let before_any_candidate = |failure: Failure| Unloaded {
            hash: None,
            failure,
        };
        let logged = |hash: Option<&[u8; 32]>, state: &str, reason: &str| {
            log_source(module, hash, state, generation, reason);
        };
        if let Some(path) = view_override(module) {
            let reason = format!("DUCKTAPE_VIEWS_DIR developer override: {}", path.display());
            logged(None, "Overridden", &reason);
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
        let Some(code) = listed_code(module) else {
            let reason = format!("this network's roster does not list {module}");
            logged(None, "Failed", &reason);
            return Err(before_any_candidate(Failure::NotListed(reason)));
        };
        let runtime = handle();
        let started = Instant::now();
        let mut timing = LoadTiming {
            path: "first",
            ..LoadTiming::default()
        };
        let show = |stage: Slot| {
            mounted
                .lock()
                .expect("module view lock")
                .show(generation, stage)
        };
        show(Slot::Fetching {
            received: 0,
            total: None,
        });
        let fetched = Instant::now();
        let bytes = runtime.block_on(crate::backend::views::view_of(client, &code));
        timing.fetch = fetched.elapsed();
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                let failure = Failure::of(error);
                logged(None, "Failed", &failure.to_string());
                timing.log(module, None, started, "Failed");
                return Err(before_any_candidate(failure));
            }
        };
        let Some(view_bytes) = bytes else {
            let hash = code_digest(&code);
            logged(Some(&hash), "Missing", "");
            timing.log(module, Some(&hash), started, "Missing");
            return Ok(Loaded::Empty(hash));
        };
        let hash: [u8; 32] = {
            use sha2::Digest as _;
            sha2::Sha256::digest(&view_bytes).into()
        };
        let shown = format!("{module} view @ {}", hex_short(&hash));
        let outcome = (|| -> Result<Loaded, Failure> {
            // the instance in the slot, if the deployment is a new one for
            // it: the replacement is seated only against that very
            // instance at that very tick count
            let mut against = {
                let locked = mounted.lock().expect("module view lock");
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
            let code = Self::compile(&view_bytes, &shown);
            timing.compile = compiled.elapsed();
            let code = code?;
            let seated = Instant::now();
            let name = manifest_name(&view_bytes);
            let prepared = (|| -> Result<Self, Failure> {
                let mut fresh =
                    Self::instantiate(module, &code, &shown).map_err(Failure::Refused)?;
                fresh.name = name.clone();
                fresh.deployed(hash);
                match &mut against {
                    // A once-valid view carries its state over. Only an
                    // explicitly admitted never-valid recovery may initialize.
                    Some((alive, ticks)) => {
                        let snapshot = {
                            let mut locked = mounted.lock().expect("module view lock");
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
                                Some(old.snapshot().map_err(Failure::Trapped)?)
                            } else {
                                None
                            }
                        };
                        match snapshot {
                            Some(snapshot) => {
                                wire::Snapshot::decode(&snapshot).map_err(Failure::Refused)?;
                                match fresh.restore(&snapshot, &shown).map_err(Failure::Trapped)? {
                                    Restored::Carried => {}
                                    Restored::Refused(refusal) => {
                                        tracing::warn!(
                                            target: "ducktape::app",
                                            module,
                                            hash = %crate::backend::hex_encode(&hash),
                                            reason = "snapshot_refused",
                                            refusal = %refusal,
                                            "view_state_dropped"
                                        );
                                        fresh.init(&shown).map_err(Failure::Trapped)?;
                                    }
                                }
                            }
                            None => fresh.init(&shown).map_err(Failure::Trapped)?,
                        }
                        let framed = Instant::now();
                        let frame = fresh.first_frame(&shown);
                        timing.first_frame = Some(framed.elapsed());
                        frame.map_err(Failure::Trapped)?;
                    }
                    None => {
                        fresh.init(&shown).map_err(Failure::Trapped)?;
                    }
                }
                Ok(fresh)
            })();
            timing.init = seated
                .elapsed()
                .saturating_sub(timing.first_frame.unwrap_or_default());
            let fresh = prepared?;
            Ok(match against {
                Some((alive, ticks)) => Loaded::Swap {
                    fresh: Box::new(fresh),
                    alive,
                    ticks,
                },
                None => Loaded::Fresh(Box::new(fresh)),
            })
        })();
        let state = match &outcome {
            Ok(Loaded::Fresh(_)) => {
                logged(Some(&hash), "Ready", "");
                "Ready"
            }
            Ok(Loaded::Unchanged) => "Unchanged",
            Ok(_) => "Swap",
            Err(failure) => {
                logged(Some(&hash), "Failed", &failure.to_string());
                "Failed"
            }
        };
        timing.log(module, Some(&hash), started, state);
        outcome.map_err(|failure| Unloaded {
            hash: Some(hash),
            failure,
        })
    }

    pub(crate) fn deployed(&mut self, hash: [u8; 32]) {
        self.hash = Some(hash);
    }

    /// Candidate attempts do not retire a seated view or its input routes.
    /// Direct test fixtures have no loader-assigned generation and use zero.
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

    pub(crate) fn snapshot(&mut self) -> Result<Vec<u8>, String> {
        arm(&mut self.store);
        self.exports
            .snapshot(&mut self.store)
            .map_err(|error| first_line(&error))?
    }

    pub(crate) fn restore(&mut self, snapshot: &[u8], shown: &str) -> Result<Restored, String> {
        arm(&mut self.store);
        let answered = self
            .exports
            .restore(&mut self.store, snapshot, cfg!(target_os = "macos"))
            .map_err(|error| format!("{shown}: restore trapped: {}", first_line(&error)))?;
        Ok(match answered {
            Ok(()) => Restored::Carried,
            Err(refusal) => Restored::Refused(refusal),
        })
    }

    /// `on mount` runs in here, told which platform it keys for.
    pub(crate) fn init(&mut self, shown: &str) -> Result<(), String> {
        arm(&mut self.store);
        if let Err(error) = self
            .exports
            .init(&mut self.store, cfg!(target_os = "macos"))
        {
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
        let limit = wire::editor_document::MAX_EDITOR_DOCUMENTS
            * (wire::editor_document::MAX_EDITOR_CHUNKS + 3)
            + 1;
        for _ in 0..limit {
            self.tick();
            #[cfg(test)]
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
            if requests.len() > MAX_REQUESTS_PER_TICK || cancels.len() > 2 * MAX_REQUESTS_PER_TICK {
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
        let metadata = std::fs::metadata(path).map_err(|error| format!("{shown}: {error}"))?;
        if metadata.len() > MAX_MODULE_BYTES {
            return Err(format!(
                "{shown}: past the {MAX_MODULE_BYTES} byte module limit"
            ));
        }
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
        guest.name = manifest_name(bytes);
        guest.init(shown)?;
        Ok(guest)
    }

    /// The view's bytes checked and compiled — the cranelift stage of
    /// a load, measured on its own.
    pub(crate) fn compile(bytes: &[u8], shown: &str) -> Result<Arc<Module>, Failure> {
        if bytes.len() as u64 > MAX_MODULE_BYTES {
            return Err(Failure::Refused(format!(
                "{shown}: past the {MAX_MODULE_BYTES} byte module limit"
            )));
        }
        // its preferred size is for placing a new window; the tab embeds
        let manifest = view_wire::manifest::read_manifest(bytes).ok_or_else(|| {
            Failure::Refused(format!("{shown}: the view's manifest cannot be read"))
        })?;
        wire_epoch(manifest.wire_epoch)
            .map_err(|error| Failure::WireEpoch(format!("{shown}: {error}")))?;
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
        // Tables are allocated eagerly at their declared minimum, before any
        // fuel or memory limit is consulted; a view is one core instance
        // and one memory.
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .memories(1)
            .instances(1)
            .tables(4)
            .table_elements(1 << 20)
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
                            let bytes = memory
                                .data(&caller)
                                .get(start..start.saturating_add(len.min(1024) as usize))?;
                            Some(String::from_utf8_lossy(bytes).into_owned())
                        })
                        .unwrap_or_default();
                    let line = message.lines().next().unwrap_or_default();
                    caller.data_mut().panic = Some(line.chars().take(1024).collect());
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
            name: String::new(),
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
            intents: Vec::new(),
            replies: Arc::default(),
            live_subscriptions: Vec::new(),
            tasks: Vec::new(),
            filesystem: Default::default(),
            media: Default::default(),
            clocks: Vec::new(),
            chords: Vec::new(),
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
        self.pending.push(wire::Event::Response {
            id,
            result: Ok(props),
            done: false,
        });
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.visibility_change = Some(visible);
    }

    /// A chord this guest claimed was pressed: every subscription that named
    /// it gets one item. Says whether any did, which is how the shell knows
    /// the press was spent and must not also be a native key.
    pub(crate) fn chord_pressed(&mut self, chord: &str) -> bool {
        let claimed: Vec<u64> = self
            .chords
            .iter()
            .filter(|(_, named)| named == chord)
            .map(|(id, _)| *id)
            .collect();
        for id in &claimed {
            self.pending.push(wire::Event::Response {
                id: *id,
                result: Ok(Vec::new()),
                done: false,
            });
        }
        !claimed.is_empty()
    }

    pub(crate) fn sync_visibility(&mut self) {
        let Some(visible) = self.visibility_change.take() else {
            return;
        };
        for id in &self.visibility_subscriptions {
            self.pending.push(wire::Event::Response {
                id: *id,
                result: Ok(visible.to_string().into_bytes()),
                done: false,
            });
        }
    }
}
