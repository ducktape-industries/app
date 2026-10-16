use super::*;

/// The host's facts a drawn view holds for its replacement (`View::ambient`).
pub struct Ambient {
    pub theme_dark: Option<bool>,
    pub touch: Option<bool>,
    pub viewport: Option<(f32, f32)>,
}

impl View {
    /// Reusing identical code over a new connection: the node subscriptions
    /// go on, opened on it, and the rest of the work started on the old one
    /// is retired (`kernel::reconnected`).
    pub fn reconnect(&mut self, connection: &Connection) {
        if self.connection.rev == connection.rev {
            return;
        }
        self.connection = connection.clone();
        self.clipboard = Default::default();
        kernel::reconnected(self, connection);
    }

    /// A view comes from the roster's `code` for `program` on the connected
    /// node — nothing else, so with no node there is nothing to load yet,
    /// and no file is ever opened for it unless the developer's override
    /// ([`override_views_from`]) supplies one, or the app ships it
    /// (`OsServices::bundled_view`, read in place of a fetch). A seat no
    /// pane holds yet (instance 0) stops at the compiled code under
    /// `Preload::Compile`: `Compiled`, its manifest read, nothing
    /// instantiated or run; the pane that claims it starts that code
    /// ([`Self::start`]). Over an empty seat a pane holds the view is
    /// `init`ed. With a view of the program already drawn, the same code is
    /// `Unchanged`, and different code is prepared and parked on the seat,
    /// whose own thread swaps it in (`seat::swap_in`): instantiated without
    /// `init`, restored from the drawn view's snapshot, its first tree
    /// verified. Overridden, Missing, Compiled, Ready and Failed each log one
    /// `view_source` line here (the seat logs `Swapped`); every load that
    /// got past the roster fills `timing`, which the seat logs as one
    /// `view_load` line once the load is installed. Each stage is also a
    /// sample under the seat's perf key.
    pub fn load(
        program: &'static str,
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
            log_source(program, hash, state, generation, reason);
        };
        let instance = lock(mounted).instance;
        if let Some(path) = view_override(program) {
            let reason = format!("DUCKTAPE_VIEWS_DIR developer override: {}", path.display());
            logged(None, "Overridden", &reason);
            if instance == 0 && preload_compiles() {
                let shown = path.display().to_string();
                let bytes = std::fs::read(&path).map_err(|error| {
                    before_any_candidate(Failure::Refused(format!("{shown}: {error}")))
                })?;
                let runner = crate::contract::runner();
                let (hash, _) =
                    Self::prepare(&**runner, &bytes, &shown).map_err(before_any_candidate)?;
                let view = view_code(program, runner, None, hash, &bytes);
                return Ok(Loaded::Compiled(Box::new(view)));
            }
            return Self::load_from(program, &path)
                .map(|view| Loaded::Fresh(Box::new(view)))
                .map_err(|reason| before_any_candidate(Failure::Refused(reason)));
        }
        let Some(client) = asked_of.client.as_ref() else {
            let reason = "not connected to a node yet";
            logged(None, "Failed", reason);
            return Err(before_any_candidate(Failure::Unreachable(
                reason.to_owned(),
            )));
        };
        let Some((blob, bare)) = code else {
            let reason = format!("this network's roster does not list {program}");
            logged(None, "Failed", &reason);
            return Err(before_any_candidate(Failure::NotListed(reason)));
        };
        // compiled for this pane's seat out of this very blob: started as it
        // is. A seat no pane holds is never started, however often it loads
        let startable = instance != 0 && lock(mounted).slot.startable(code);
        if startable {
            return Self::start(program, code, generation, mounted, timing);
        }
        let runtime = handle();
        timing.started = Some(Instant::now());
        timing.path = "first";
        let key = crate::perf::Key::View { program, instance };
        let show = |stage: Slot| lock(mounted).show(generation, stage);
        let fetched = Instant::now();
        let bytes = match crate::contract::os().bundled_view(&blob) {
            // the app ships the view section this blob holds: no fetch
            Some(view_bytes) => Ok(Some((view_bytes, "bundle"))),
            None => {
                show(Slot::Fetching {
                    received: 0,
                    total: None,
                });
                runtime.block_on(crate::backend::views::view_of(client, &blob, bare))
            }
        };
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
        let Some((view_bytes, from)) = bytes else {
            let hash = code_digest(&blob);
            logged(Some(&hash), "Missing", "");
            timing.hash = Some(hash);
            timing.outcome = "Missing";
            return Ok(Loaded::Empty);
        };
        timing.blob = from;
        crate::perf::count(
            key,
            match from {
                "bundle" => "fetch.bundle",
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
        let shown = shown_as(program, &hash);
        // the bytes are here: the rail and Home list the program by its
        // manifest before any of its code is compiled
        let listing = manifest_of(&view_bytes);
        show(Slot::Listed(Box::new(listing.clone())));
        let outcome = (|| -> Result<Loaded, Failure> {
            // a view drawn of other code: this deployment replaces it, on
            // the seat's own thread once the code is prepared (`Mounted::swap`)
            let swap = {
                let locked = lock(mounted);
                match &locked.slot {
                    Slot::Ready(old) if old.hash == Some(hash) => return Ok(Loaded::Unchanged),
                    Slot::Ready(_) => true,
                    _ => false,
                }
            };
            if swap {
                timing.path = "swap";
            }
            show(Slot::Compiling(Box::new(listing)));
            let compiled = Instant::now();
            let runner = crate::contract::runner();
            let code = Self::prepare(&**runner, &view_bytes, &shown);
            timing.compile = compiled.elapsed();
            crate::perf::record(key, "compile", timing.compile.as_micros() as u64);
            let (_, source) = code?;
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
            let view = view_code(program, runner, Some(blob), hash, &view_bytes);
            // a seat no pane holds stops here where the host preloads by
            // compiling (`Preload::Compile`, the desktop): the rail reads its
            // name and its icon off the manifest, and the pane that claims
            // it starts it. `Preload::Seat` (the phone) seats and inits it
            if swap {
                return Ok(Loaded::Swap {
                    code: Box::new(view),
                    shown,
                });
            }
            if instance == 0 && preload_compiles() {
                return Ok(Loaded::Compiled(Box::new(view)));
            }
            let mut fresh = Self::instance_of(program, instance, &view, &shown, timing)?;
            fresh.timed_init(&shown, timing)?;
            Ok(Loaded::Fresh(Box::new(fresh)))
        })();
        timing.hash = Some(hash);
        timing.outcome = match &outcome {
            Ok(Loaded::Fresh(_)) => {
                logged(Some(&hash), "Ready", "");
                "Ready"
            }
            Ok(Loaded::Unchanged) => "Unchanged",
            Ok(Loaded::Compiled(_)) => {
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

    /// The view a seat holds compiled out of the blob `code` names, started
    /// for the pane that claimed it: an instance and its `init`, nothing
    /// fetched or compiled again, which the window thread runs as it runs a
    /// tick (`mounted`). A seat that moved on meanwhile (a newer load, the
    /// program gone) has nothing here to start: the load is stale, and its
    /// failure lands nowhere.
    pub fn start(
        program: &'static str,
        code: Option<(::abi::BlobId, bool)>,
        generation: u64,
        mounted: &Arc<Mutex<Mounted>>,
        timing: &mut LoadTiming,
    ) -> Result<Loaded, Unloaded> {
        let (view, instance) = {
            let locked = lock(mounted);
            match &locked.slot {
                Slot::Compiled(view) if locked.slot.startable(code) => {
                    ((**view).clone(), locked.instance)
                }
                _ => {
                    return Err(Unloaded {
                        hash: None,
                        failure: Failure::Refused(
                            "the seat moved on before its view started".into(),
                        ),
                    });
                }
            }
        };
        timing.started = Some(Instant::now());
        timing.path = "first";
        timing.blob = "seat";
        timing.code = "seat";
        timing.hash = Some(view.hash);
        let shown = shown_as(program, &view.hash);
        let started =
            Self::instance_of(program, instance, &view, &shown, timing).and_then(|mut fresh| {
                fresh.timed_init(&shown, timing)?;
                Ok(fresh)
            });
        let (state, reason) = match &started {
            Ok(_) => ("Ready", String::new()),
            Err(failure) => ("Failed", failure.to_string()),
        };
        log_source(program, Some(&view.hash), state, generation, &reason);
        timing.outcome = state;
        started
            .map(|fresh| Loaded::Fresh(Box::new(fresh)))
            .map_err(|failure| Unloaded {
                hash: Some(view.hash),
                failure,
            })
    }

    /// An instance of `view` for the pane `instance`, its manifest's facts
    /// on it, nothing run yet: `init`ed fresh, or `restore`d as a
    /// replacement.
    fn instance_of(
        program: &'static str,
        instance: u64,
        view: &ViewCode,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        now(async { Ok(Self::seated_from(program, instance, view, shown, timing).await) })
            .unwrap_or_else(|stuck| Err(Failure::Refused(stuck)))
    }

    /// [`Self::instance_of`], its seating awaited.
    pub async fn seated_from(
        program: &'static str,
        instance: u64,
        view: &ViewCode,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let instantiated = Instant::now();
        let mut fresh = Self::seated(program, &view.runner, view.hash, shown)
            .await
            .map_err(Failure::Refused)?;
        timing.instantiate = instantiated.elapsed();
        fresh.instance = instance;
        let key = fresh.perf_key();
        crate::perf::record(key, "instantiate", timing.instantiate.as_micros() as u64);
        crate::perf::record(key, "fuel.instantiate", fresh.fuel_used());
        fresh.name = view.name.clone();
        fresh.icon = view.icon.clone();
        fresh.description = view.description.clone();
        fresh.capabilities = view.capabilities.clone();
        fresh.targets = view.targets.clone();
        fresh.min_width = view.min_width;
        fresh.deployed(view.hash);
        Ok(fresh)
    }

    /// A once-valid view carries its state over to a new instance of
    /// `code` for the pane `instance`, on the seat's thread (SPEC §4.6): the
    /// drawn view `alive` names is asked for its snapshot
    /// ([`Self::handover`]), then the new instance seated, restored from it
    /// and its first tree verified ([`Self::prepared`]), each step begun
    /// once the one before landed. The seat's lock is held only to ask and
    /// to read an answer, never across a wait; the seat does not turn the
    /// drawn view meanwhile (`Mounted::owned`).
    pub async fn replacement(
        program: &'static str,
        instance: u64,
        code: &ViewCode,
        alive: &Arc<()>,
        mounted: &Arc<Mutex<Mounted>>,
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let (asked, ambient) = {
            let mut locked = lock(mounted);
            let old = drawn(&mut locked, alive)?;
            (old.handover(&code.hash)?, old.ambient())
        };
        let state = match asked {
            None => None,
            Some(landing) => {
                let taken = Instant::now();
                let landed = landing.recv().await.unwrap_or_else(|_| {
                    Landed::Fault(Fault::Runner(
                        "the instance stopped before it answered".into(),
                    ))
                });
                let mut locked = lock(mounted);
                let old = drawn(&mut locked, alive)?;
                old.handed_over(landed, instance, &code.hash, taken, timing)?
            }
        };
        let fresh = Self::seated_from(program, instance, code, shown, timing).await?;
        Self::prepared(fresh, (state, ambient), shown, timing).await
    }

    /// A tick whose instance died before it answered (`Fault::Runner`: the
    /// page it ran in is gone, SPEC §6.5), taken up again where the host
    /// keeps a replay (`View::kept`): a new instance of the same code is
    /// seated and restored from the kept snapshot (or inited), every kept
    /// batch but the last replayed with its frame discarded (those frames'
    /// requests, intents and trees were taken in the first time), and the
    /// last, the tick that died, begun again on it. The channel that tick's
    /// frame lands on, or why there is none: the fault then stands.
    pub async fn revive(
        mounted: &Arc<Mutex<Mounted>>,
        alive: &Arc<()>,
    ) -> Result<async_channel::Receiver<Landed>, String> {
        let (program, runner, hash, kept) = {
            let mut locked = lock(mounted);
            let view = drawn(&mut locked, alive).map_err(|failure| failure.to_string())?;
            let kept = view.kept.clone().ok_or("no replay is kept")?;
            (
                view.program,
                view.runner.clone(),
                view.running.code_hash(),
                kept,
            )
        };
        let (last, replayed) = kept.batches.split_last().ok_or("no tick to replay")?;
        let (land, landing) = Landing::channel();
        runner.instantiate(hash, SeatId(program, 0), land);
        let mut running = match next(&landing).await {
            Landed::Seated(running) => running,
            other => return Err(unexpected(other)),
        };
        match &kept.snapshot {
            Some(state) => running.begin_restore(state.clone()),
            None => running.begin_init(),
        }
        match next(&landing).await {
            Landed::Restored | Landed::Inited => {}
            other => return Err(unexpected(other)),
        }
        for batch in replayed {
            running.begin_tick(batch.clone());
            match next(&landing).await {
                Landed::Frame(_) => {}
                other => return Err(unexpected(other)),
            }
        }
        let mut locked = lock(mounted);
        let view = drawn(&mut locked, alive).map_err(|failure| failure.to_string())?;
        std::mem::replace(&mut view.running, running).stop();
        view.landing = landing.clone();
        view.running.begin_tick(last.clone());
        Ok(landing)
    }

    /// Whether the replay this view keeps is due a new snapshot: settled,
    /// nothing in flight, and the last kept one older than the host's
    /// `Limits::snapshot_every` (SPEC §6.5).
    pub fn keep_due(&self) -> bool {
        // no replay kept (the desktop): nothing read, every turn
        let Some(kept) = &self.kept else {
            return false;
        };
        self.ticks > 0
            && !self.ticking
            && self.settled()
            && crate::contract::os()
                .limits()
                .snapshot_every
                .is_some_and(|every| kept.at.elapsed() >= every)
    }

    /// Asks for the snapshot a kept replay starts from from now on: the
    /// channel it lands on.
    pub fn begin_keep(&mut self) -> async_channel::Receiver<Landed> {
        self.running.begin_snapshot();
        self.landing()
    }

    /// The snapshot [`Self::begin_keep`] asked for, landed: taken, it is
    /// the replay's start and the batches before it go; refused, the one
    /// kept stays (the view has work in hand). A dead instance is the next
    /// tick's to find (`revive`); a trap ends the view, as in a tick.
    pub fn kept_landed(&mut self, landed: Landed) {
        if matches!(landed, Landed::Fault(Fault::Runner(_))) {
            return;
        }
        if let Ok(Snapshot::Taken(state)) = self.snapshot_landed(landed)
            && let Some(kept) = &mut self.kept
        {
            *kept = Kept {
                snapshot: Some(state),
                at: Instant::now(),
                batches: Vec::new(),
            };
        }
    }

    /// The host's facts this drawn view holds, for its replacement to draw
    /// its first tree by: the theme, the pointer and the body it is laid
    /// out in, as the seat last told them. The offset is the host's to read at any time.
    fn ambient(&self) -> Ambient {
        Ambient {
            theme_dark: self.theme_dark,
            touch: self.touch_sent,
            viewport: self.viewport_sent,
        }
    }

    /// A replacement hears the host's facts before its first frame, so the
    /// tree it draws is themed, laid out to its pane and in the reader's
    /// zone, and the seat's first turn finds nothing to tell it.
    fn seed(&mut self, ambient: Ambient) {
        if let Some(dark) = ambient.theme_dark {
            self.sync_theme(dark);
        }
        if let Some(touch) = ambient.touch {
            self.sync_pointer(touch);
        }
        if let Some((width, height)) = ambient.viewport {
            self.sync_viewport(width, height);
        }
        self.sync_offset(kernel::offset_minutes());
    }

    /// This drawn view asked for what it hands `to`, its replacement: the
    /// channel its snapshot lands on, or `None` when it hands nothing. A
    /// view that never ticked has no state. One that stopped never will: `to`
    /// starts clean, with a warn saying why. One with work unfinished
    /// refuses the replacement until its next turns finish it, and the seat
    /// asks again: the host's own (an event, a widget command, an editor
    /// transfer), or the view's, in its own words (busy, a request in flight).
    fn handover(
        &mut self,
        to: &[u8; 32],
    ) -> Result<Option<async_channel::Receiver<Landed>>, Failure> {
        if self.ticks == 0 {
            return Ok(None);
        }
        if let Some(fault) = self.fault.clone() {
            state_dropped(self.program, Some(to), "view_stopped", &fault);
            return Ok(None);
        }
        if !self.settled() {
            return Err(Failure::Refused(
                "the view has pending work; its replacement waits".into(),
            ));
        }
        self.running.begin_snapshot();
        Ok(Some(self.landing()))
    }

    /// The handover [`Self::handover`] asked for, landed (`taken` when it
    /// was asked): the snapshot, or none when it traps (`to` starts clean).
    /// A view that does not hand its state over yet refuses the replacement.
    fn handed_over(
        &mut self,
        landed: Landed,
        instance: u64,
        to: &[u8; 32],
        taken: Instant,
        timing: &mut LoadTiming,
    ) -> Result<Option<Vec<u8>>, Failure> {
        timing.snapshot = taken.elapsed();
        let key = crate::perf::Key::View {
            program: self.program,
            instance,
        };
        crate::perf::record(key, "snapshot", timing.snapshot.as_micros() as u64);
        crate::perf::record(key, "fuel.snapshot", self.fuel_used());
        match self.snapshot_landed(landed) {
            Ok(Snapshot::Taken(snapshot)) => {
                timing.snapshot_bytes = snapshot.len();
                crate::perf::gauge(key, "snapshot_bytes", snapshot.len() as u64);
                Ok(Some(snapshot))
            }
            Ok(Snapshot::Refused(refusal)) => Err(Failure::Refused(format!(
                "the view does not hand its state over yet ({refusal}); its replacement waits"
            ))),
            Err(trap) => {
                state_dropped(self.program, Some(to), "snapshot_trapped", &trap);
                Ok(None)
            }
        }
    }

    /// `fresh` started from `state`, a drawn view's handover: restored, or
    /// `init`ed in its place when there is none, when the view refuses it
    /// as not its own, or when the restore traps (on a new instance of its
    /// code then: a trapped one is not entered again). Its first tree is
    /// verified. Every answer is awaited as it lands.
    async fn prepared(
        mut fresh: Self,
        (state, ambient): (Option<Vec<u8>>, Ambient),
        shown: &str,
        timing: &mut LoadTiming,
    ) -> Result<Self, Failure> {
        let carried = match state {
            Some(state) => {
                let restored = Instant::now();
                let answered = fresh.restore(&state, shown).await;
                timing.restore = restored.elapsed();
                let key = fresh.perf_key();
                crate::perf::record(key, "restore", timing.restore.as_micros() as u64);
                crate::perf::record(key, "fuel.restore", fresh.fuel_used());
                match answered {
                    Ok(Restored::Carried) => {
                        // a replay starts from the state it was restored from
                        if let Some(kept) = &mut fresh.kept {
                            kept.snapshot = Some(state);
                        }
                        true
                    }
                    Ok(Restored::Refused(refusal)) => {
                        state_dropped(
                            fresh.program,
                            fresh.hash.as_ref(),
                            "snapshot_refused",
                            &refusal,
                        );
                        false
                    }
                    Err(trap) => {
                        state_dropped(fresh.program, fresh.hash.as_ref(), "restore_trapped", &trap);
                        fresh = fresh.sibling(shown).await.map_err(Failure::Refused)?;
                        false
                    }
                }
            }
            None => false,
        };
        if !carried {
            fresh.timed_inited(shown, timing).await?;
        }
        fresh.seed(ambient);
        let framed = Instant::now();
        let frame = fresh.first_frame(shown).await;
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

    /// A new instance of this one's code, named and manifested as `load` made this
    /// one, nothing run in it yet. Every field is named here, so one added
    /// to `View` is a compile error until it says whether a sibling
    /// carries it: what `load` set (the seat, the manifest, the deployment)
    /// is carried, and what the instance ran into starts fresh.
    async fn sibling(&self, shown: &str) -> Result<Self, String> {
        let Self {
            program,
            instance,
            name,
            icon,
            description,
            capabilities,
            targets,
            min_width,
            hash,
            connection: _,
            activation: _,
            undeclared_logged: _,
            running,
            runner,
            landing: _,
            tick_call: _,
            ticking: _,
            kept: _,
            pending: _,
            theme_dark: _,
            touch_sent: _,
            viewport_sent: _,
            offset_sent: _,
            #[cfg(any(test, feature = "test-support"))]
                ticked_with: _,
            widget_commands: _,
            frame: _,
            styles: _,
            frame_reports: _,
            display_diagnostics: _,
            installed_generation: _,
            frame_rev: _,
            tooltip_responses: _,
            ticks: _,
            pictures: _,
            props_subscription: _,
            props_sent: _,
            visible: _,
            visibility_change: _,
            visibility_subscriptions: _,
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
        let mut sibling = Self::seated(program, runner, running.code_hash(), shown).await?;
        sibling.instance = *instance;
        sibling.name = name.clone();
        sibling.icon = icon.clone();
        sibling.description = description.clone();
        sibling.capabilities = capabilities.clone();
        sibling.targets = targets.clone();
        sibling.min_width = *min_width;
        sibling.hash = *hash;
        Ok(sibling)
    }

    /// `init`, timed into `timing` and the registry, as a load runs it.
    fn timed_init(&mut self, shown: &str, timing: &mut LoadTiming) -> Result<(), Failure> {
        now(async { Ok(self.timed_inited(shown, timing).await) })
            .unwrap_or_else(|stuck| Err(Failure::Trapped(stuck)))
    }

    /// [`Self::timed_init`], its answer awaited.
    async fn timed_inited(&mut self, shown: &str, timing: &mut LoadTiming) -> Result<(), Failure> {
        let started = Instant::now();
        let ran = self.inited(shown).await;
        timing.init = started.elapsed();
        crate::perf::record(self.perf_key(), "init", timing.init.as_micros() as u64);
        crate::perf::record(self.perf_key(), "fuel.init", self.fuel_used());
        ran.map_err(Failure::Trapped)
    }

    pub fn deployed(&mut self, hash: [u8; 32]) {
        self.hash = Some(hash);
    }

    /// The load generation that seated this instance — zero for one built
    /// directly by a test — so a candidate still loading retires nothing
    /// of the seated view.
    pub fn seated_generation(&self) -> u64 {
        self.installed_generation.unwrap_or_default()
    }

    /// Everything this instance was asked to do is done: nothing pending,
    /// no request the host has yet to route, no trap. Busy (out of budget,
    /// ticking again soon) is the view's to weigh: its snapshot says
    /// whether it can hand its state over mid-work. A replacement not
    /// yet redrawn is settled too: the only requests its first tree
    /// carries are the subscriptions its restore rebuilt, which its own
    /// replacement rebuilds again — a tab not shown between two
    /// deployments is not stuck on the first.
    pub fn settled(&self) -> bool {
        self.fault.is_none()
            && self.pending.is_empty()
            && self.widget_commands.is_empty()
            && (self.staged || self.frame.requests.is_empty())
    }

    /// The view's state, or why it does not hand it over now. A trap ends
    /// the instance as one in `tick` does: the fault keeps it from being
    /// entered again.
    #[cfg(test)]
    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        self.running.begin_snapshot();
        let landed = self.at_once();
        self.snapshot_landed(landed)
    }

    /// What a `snapshot` answered, as [`Self::snapshot`] reads it.
    fn snapshot_landed(&mut self, landed: Landed) -> Result<Snapshot, String> {
        match landed {
            Landed::Snapshot(state) => Ok(Snapshot::Taken(state)),
            Landed::SnapshotRefused(refusal) => Ok(Snapshot::Refused(refusal)),
            other => {
                let reason = unexpected(other);
                self.fault = Some(reason.clone());
                Err(reason)
            }
        }
    }

    pub async fn restore(&mut self, snapshot: &[u8], shown: &str) -> Result<Restored, String> {
        self.running.begin_restore(snapshot.to_vec());
        match self.landed().await {
            Landed::Restored => Ok(Restored::Carried),
            Landed::RestoreRefused(refusal) => Ok(Restored::Refused(refusal)),
            other => Err(format!("{shown}: restore trapped: {}", unexpected(other))),
        }
    }

    /// The view's `init` export: a fresh start, with no snapshot to restore.
    pub fn init(&mut self, shown: &str) -> Result<(), String> {
        now(self.inited(shown))
    }

    /// [`Self::init`], its answer awaited.
    pub async fn inited(&mut self, shown: &str) -> Result<(), String> {
        self.running.begin_init();
        match self.landed().await {
            Landed::Inited => Ok(()),
            // the view's own words, as its panic hook handed them over
            Landed::Fault(Fault::Panicked(words)) => Err(words),
            other => Err(format!("{shown}: init trapped: {}", unexpected(other))),
        }
    }

    /// A restored instance's first ticks, until one leaves nothing owed
    /// (the resync a picture it names by hash alone may cost it,
    /// `Pictures::adopt`): its whole tree, or it is no replacement. Its
    /// requests wait for the first redraw.
    pub async fn first_frame(&mut self, shown: &str) -> Result<(), String> {
        let mut requests = Vec::new();
        let mut cancels = Vec::new();
        loop {
            self.begin_tick();
            let landed = self.landed().await;
            self.tick_landed(landed);
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
            if self.pending.is_empty() {
                break;
            }
        }
        requests.retain(|request| !cancels.contains(&request.id));
        self.frame.requests = requests;
        self.frame.cancels = cancels;
        self.ticks += 1;
        self.staged = true;
        Ok(())
    }

    pub fn load_from(program: &'static str, path: &std::path::Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|error| format!("{shown}: {error}"))?;
        Self::from_bytes(program, &bytes, &shown)
    }

    /// The view instantiated and mounted; `shown` names it in errors.
    pub fn from_bytes(program: &'static str, bytes: &[u8], shown: &str) -> Result<Self, String> {
        let runner = crate::contract::runner();
        let (code, _) =
            Self::prepare(&**runner, bytes, shown).map_err(|failure| failure.to_string())?;
        let mut view = Self::instantiate(program, runner, code, shown)?;
        let listing = manifest_of(bytes);
        view.name = listing.name;
        view.icon = listing.icon;
        view.description = listing.description;
        view.capabilities = listing.capabilities;
        view.targets = listing.targets;
        view.min_width = listing.min_width;
        crate::render::web::seen(program, &view.capabilities);
        view.init(shown)?;
        Ok(view)
    }

    /// The view's bytes checked (its manifest read, its wire this app's)
    /// and prepared on `runner` — the compile stage of a load, measured on
    /// its own: their sha256, which instances are seated by, and where the
    /// code came from (`ViewRunner::prepare`).
    pub fn prepare(
        runner: &dyn ViewRunner,
        bytes: &[u8],
        shown: &str,
    ) -> Result<([u8; 32], &'static str), Failure> {
        // refused here, a manifest `manifest_of` would read as empty never
        // reaches a seat
        let manifest = view_wire::manifest::read_manifest(bytes).ok_or_else(|| {
            Failure::Refused(format!("{shown}: the view's manifest cannot be read"))
        })?;
        wire_id(&manifest.wire_id).map_err(|error| Failure::Wire(format!("{shown}: {error}")))?;
        let hash = {
            use sha2::Digest as _;
            sha2::Sha256::digest(bytes).into()
        };
        let view_bytes =
            usize::try_from(crate::backend::memory_bounds().view_bytes).unwrap_or(usize::MAX);
        let source = runner
            .prepare(hash, bytes, view_bytes)
            .map_err(|failure| Failure::Refused(format!("{shown}: {failure}")))?;
        Ok((hash, source))
    }

    /// [`Self::seated`], taken at once: a load on its own thread, a test.
    pub fn instantiate(
        program: &'static str,
        runner: &Arc<dyn ViewRunner>,
        code: [u8; 32],
        shown: &str,
    ) -> Result<Self, String> {
        now(Self::seated(program, runner, code, shown))
    }

    /// An instance of the prepared `code` seated by `runner`, its exports
    /// bound, nothing run yet: a fresh view is `init`ed, a replacement
    /// `restore`d. Its answers land on a channel of its own.
    pub async fn seated(
        program: &'static str,
        runner: &Arc<dyn ViewRunner>,
        code: [u8; 32],
        shown: &str,
    ) -> Result<Self, String> {
        let (land, landing) = Landing::channel();
        runner.instantiate(code, SeatId(program, 0), land);
        let running = match landing.recv().await {
            Ok(Landed::Seated(running)) => running,
            Ok(other) => return Err(format!("{shown}: {}", unexpected(other))),
            Err(_) => {
                return Err(format!(
                    "{shown}: the runner stopped before it seated the view"
                ));
            }
        };
        Ok(Self {
            connection: connection().lock().expect("views rpc").clone(),
            activation: None,
            program,
            instance: 0,
            name: String::new(),
            icon: String::new(),
            description: String::new(),
            capabilities: Vec::new(),
            targets: Vec::new(),
            min_width: 0,
            undeclared_logged: Vec::new(),
            running,
            runner: runner.clone(),
            landing,
            tick_call: None,
            ticking: false,
            kept: Kept::if_kept(),
            pending: Vec::new(),
            theme_dark: None,
            touch_sent: None,
            viewport_sent: None,
            offset_sent: None,
            #[cfg(any(test, feature = "test-support"))]
            ticked_with: Vec::new(),
            widget_commands: Vec::new(),
            frame: wire::Frame::default(),
            styles: wire::Styles::default(),
            frame_reports: Default::default(),
            display_diagnostics: Default::default(),
            installed_generation: None,
            frame_rev: 0,
            tooltip_responses: Vec::new(),
            ticks: 0,
            pictures: Pictures::default(),
            props_subscription: None,
            props_sent: None,
            visible: false,
            visibility_change: None,
            visibility_subscriptions: Vec::new(),
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

    /// Hands the view the props the app holds, if they moved since the
    /// view last saw them. Before the view has subscribed they wait here.
    pub fn sync_props(&mut self, props: &Option<Vec<u8>>) {
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

    pub fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.visibility_change = Some(visible);
    }

    /// A route a link left for this seat goes to its first route
    /// subscriber, once; with no subscriber it waits.
    pub fn sync_route(&mut self) {
        let Some(&id) = self.route_subscriptions.first() else {
            return;
        };
        if let Some(route) = crate::runtime::take_route(self.program, self.instance) {
            self.stream_item(id, wire::methods::encode(&route));
        }
    }

    pub fn sync_visibility(&mut self) {
        let Some(visible) = self.visibility_change.take() else {
            return;
        };
        for id in self.visibility_subscriptions.clone() {
            self.stream_item(id, wire::methods::encode(&visible));
        }
    }
}

/// How a view is named in logs and errors: its program and the first six
/// bytes of its hash.
fn shown_as(program: &str, hash: &[u8; 32]) -> String {
    format!(
        "{program} view @ {}",
        noded_client::hex::hex_encode(&hash[..6])
    )
}

/// The view `bytes`, prepared on `runner` as `hash` out of `blob`, with
/// what their manifest says: what a pane starts it from.
fn view_code(
    program: &'static str,
    runner: &Arc<dyn ViewRunner>,
    blob: Option<::abi::BlobId>,
    hash: [u8; 32],
    bytes: &[u8],
) -> ViewCode {
    let listing = manifest_of(bytes);
    crate::render::web::seen(program, &listing.capabilities);
    ViewCode {
        runner: runner.clone(),
        blob,
        hash,
        name: listing.name,
        icon: listing.icon,
        description: listing.description,
        min_width: listing.min_width,
        capabilities: listing.capabilities,
        targets: listing.targets,
    }
}

/// The warn a replacement of `program`'s view (code `hash`) logs when it
/// starts without the drawn view's state, `reason` naming why and
/// `refusal` in the words that said so.
fn state_dropped(program: &str, hash: Option<&[u8; 32]>, reason: &str, refusal: &str) {
    tracing::warn!(
        target: "ducktape::app",
        program,
        hash = %noded_client::hex::hex_encode(hash.map_or(&[][..], |hash| hash)),
        reason,
        refusal = %refusal,
        "view_state_dropped"
    );
}

/// The next answer on `landing`; a stopped instance answers a fault.
async fn next(landing: &async_channel::Receiver<Landed>) -> Landed {
    landing.recv().await.unwrap_or_else(|_| {
        Landed::Fault(Fault::Runner(
            "the instance stopped before it answered".into(),
        ))
    })
}

/// The drawn view in `locked` that `alive` names, or why it is gone: a
/// replacement is prepared against that very instance.
fn drawn<'a>(locked: &'a mut Mounted, alive: &Arc<()>) -> Result<&'a mut View, Failure> {
    match &mut locked.slot {
        Slot::Ready(old) if Arc::ptr_eq(alive, &old.alive) => Ok(old),
        Slot::Ready(_) => Err(Failure::Refused(
            "the view changed while its replacement was prepared".into(),
        )),
        _ => Err(Failure::Refused(
            "the view left while its replacement was prepared".into(),
        )),
    }
}

/// `call` taken at once, on a path that cannot wait for a later landing
/// (a load on its own thread, a test): the wasmtime runner lands every
/// answer before its call returns; a runner that answers later is refused
/// here, never waited for.
fn now<T>(call: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    use futures::FutureExt as _;
    call.now_or_never()
        .unwrap_or_else(|| Err("the runner did not answer at once".into()))
}

/// Whether a seat no pane holds stops at its compiled code (the
/// desktop's `Preload::Compile`) rather than being seated and inited.
fn preload_compiles() -> bool {
    crate::contract::os().limits().preload == crate::contract::Preload::Compile
}

/// What a call answered that is not its answer, as the sentence it fails
/// with: a fault's words, or the answer it got.
fn unexpected(landed: Landed) -> String {
    match landed {
        Landed::Fault(fault) => fault.to_string(),
        other => format!("the view answered {other:?}"),
    }
}
