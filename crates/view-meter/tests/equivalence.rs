//! The runner equivalence gate, Linux half (wasm-mobile SPEC §13.3 (a)): a
//! metered module run in wasmtime WITHOUT fuel answers exactly as the
//! original module does in wasmtime WITH fuel and the app's `ViewLimits`.
//!
//! - `chat_session`, `forge_session`: the recorded desk sessions of the S0
//!   probes (`tests/sessions`, gzipped; programs `afb9e33e` builds of the chat
//!   and forge views, lane 02's recordings, sha256s asserted below). Every
//!   tick's frame equals the recording under both, every tick's fuel equals
//!   wasmtime's, the totals are the S0-native ones, and the frames after a
//!   snapshot + restore at mid-session hash to the S0-webkit reference.
//! - the rest: hand-written modules for each edge the pass must answer the
//!   way wasmtime does (grows past the budget, size-charged operators, where
//!   fuel runs out, refused features, hostile input).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails loudly on the first surprise"
)]

use std::io::Read;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use view_meter::{FEATURES, GAS, GROW_REFUSED, Refusal, Stop, accepts, meter, stop};
use wasmtime::{
    Config, Engine, Global, Instance, Linker, Module, OptLevel, ResourceLimiter, Store, Trap, Val,
    WasmFeatures,
};

/// The app's budget per call (`crates/host/src/runtime.rs`).
const FUEL_PER_TICK: u64 = 250_000_000;
/// The `memory.view_bytes` default (`crates/host/src/backend/prefs.rs`).
const VIEW_BYTES: u64 = 64 << 20;

/// A wasmtime engine the way a view runner configures one: Cranelift at
/// speed, every feature outside [`FEATURES`] refused (the recipe the wasmtime
/// runner applies), fuel on for the original module, off for the metered.
fn engine(fuel: bool) -> &'static Engine {
    static ENGINES: [OnceLock<Engine>; 2] = [OnceLock::new(), OnceLock::new()];
    ENGINES[usize::from(fuel)].get_or_init(|| {
        let mut config = Config::new();
        config.cranelift_opt_level(OptLevel::Speed);
        config.consume_fuel(fuel);
        config.wasm_features(WasmFeatures::all().difference(FEATURES), false);
        Engine::new(&config).unwrap()
    })
}

/// The desktop's `ViewLimits` (`crates/host/src/runtime/view/abi.rs`): linear
/// memory plus a pointer per table element, against one budget; past it the
/// grow is an error, so the call traps. Asked before the module's own
/// maximum, in wasmtime's order; a grow that maximum then refuses holds
/// nothing, so it records nothing. The desktop's still records it until
/// app#545 (lane H) lands that fix; from then the two agree.
struct ViewLimits {
    budget: usize,
    memory: usize,
    tables: usize,
}

#[derive(Debug)]
struct PastBudget;

impl std::fmt::Display for PastBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the view is past the device's view memory setting")
    }
}

impl std::error::Error for PastBudget {}

impl ViewLimits {
    fn fits(&self, memory: usize, tables: usize) -> wasmtime::Result<()> {
        match memory.saturating_add(tables) <= self.budget {
            true => Ok(()),
            false => Err(wasmtime::Error::new(PastBudget)),
        }
    }
}

impl ResourceLimiter for ViewLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.fits(desired, self.tables)?;
        let lands = maximum.is_none_or(|maximum| desired <= maximum);
        if lands {
            self.memory = desired;
        }
        Ok(lands)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let added = desired.saturating_sub(current);
        let tables = self
            .tables
            .saturating_add(added.saturating_mul(std::mem::size_of::<usize>()));
        self.fits(self.memory, tables)?;
        let lands = maximum.is_none_or(|maximum| desired <= maximum);
        if lands {
            self.tables = tables;
        }
        Ok(lands)
    }
}

/// One instance, original (fuel + `ViewLimits`) or metered (no fuel, no
/// limiter: the module bounds itself).
struct Run {
    store: Store<ViewLimits>,
    instance: Instance,
    gas: Option<Global>,
}

/// How a call ended.
#[derive(Debug, Clone, PartialEq)]
enum End {
    Returned(Vec<i64>),
    Stopped(Stop),
}

/// A module compiled once, original (run with fuel and `ViewLimits`) or
/// metered (run with neither: it bounds itself), seated as often as needed.
struct Compiled {
    module: Module,
    budget: u64,
    metered: bool,
}

impl Compiled {
    fn original(code: &[u8], budget: u64) -> Self {
        Self {
            module: Module::new(engine(true), code).unwrap(),
            budget,
            metered: false,
        }
    }

    /// `metered` is `meter(code, budget)`.
    fn metered(metered: &[u8], budget: u64) -> Self {
        Self {
            module: Module::new(engine(false), metered).unwrap(),
            budget,
            metered: true,
        }
    }

    fn run(&self) -> Run {
        let engine = self.module.engine();
        let limits = ViewLimits {
            // the metered run's limiter never refuses: the module does
            budget: if self.metered {
                usize::MAX
            } else {
                usize::try_from(self.budget).unwrap()
            },
            memory: 0,
            tables: 0,
        };
        let mut store = Store::new(engine, limits);
        store.limiter(|limits| limits);
        if !self.metered {
            store.set_fuel(FUEL_PER_TICK).unwrap();
        }
        let mut linker = Linker::new(engine);
        linker
            .func_wrap("ducktape_view", "panicked", |_: u32, _: u32| {})
            .unwrap();
        let instance = linker.instantiate(&mut store, &self.module).unwrap();
        let gas = self
            .metered
            .then(|| instance.get_global(&mut store, GAS).unwrap());
        Run {
            store,
            instance,
            gas,
        }
    }
}

/// `meter(code, budget)` for a module the test runs: [`accepts`] takes it,
/// and then `meter` must meter it. Every module the suite meters comes
/// through here (the sessions, the probes, the random bodies), so the rule
/// and the pass cannot drift apart unseen.
fn meter_accepted(code: &[u8], budget: u64) -> Vec<u8> {
    assert_eq!(accepts(code), Ok(()), "accepts takes it");
    match meter(code, budget) {
        Ok(metered) => metered,
        Err(refusal) => panic!("accepts takes it, and meter refuses: {refusal}"),
    }
}

impl Run {
    fn arm(&mut self, fuel: u64) {
        match self.gas {
            Some(gas) => gas
                .set(&mut self.store, Val::I64(i64::try_from(fuel).unwrap()))
                .unwrap(),
            None => self.store.set_fuel(fuel).unwrap(),
        }
    }

    /// What is left of the fuel (metered: [`GAS`], which may be below 0).
    fn left(&mut self) -> i64 {
        match self.gas {
            Some(gas) => gas.get(&mut self.store).unwrap_i64(),
            None => i64::try_from(self.store.get_fuel().unwrap()).unwrap(),
        }
    }

    /// Calls an export with i32 arguments, armed with `fuel`.
    fn call(&mut self, name: &str, args: &[i32], fuel: u64) -> End {
        self.arm(fuel);
        let func = self.instance.get_func(&mut self.store, name).unwrap();
        let args: Vec<Val> = args.iter().map(|&arg| Val::I32(arg)).collect();
        let mut results = vec![Val::I32(0); func.ty(&self.store).results().len()];
        match func.call(&mut self.store, &args, &mut results) {
            Ok(()) => End::Returned(
                results
                    .iter()
                    .map(|value| value.i32().map_or_else(|| value.unwrap_i64(), i64::from))
                    .collect(),
            ),
            Err(error) => End::Stopped(self.stopped(&error)),
        }
    }

    /// The original's trap read the way the host reads it; the metered one's
    /// read from [`GAS`] the way the iOS runner will.
    fn stopped(&mut self, error: &wasmtime::Error) -> Stop {
        match self.gas {
            Some(gas) => {
                assert!(
                    error.downcast_ref::<Trap>().is_some(),
                    "a metered call stops by a trap: {error:?}"
                );
                stop(gas.get(&mut self.store).unwrap_i64())
            }
            None if error.downcast_ref::<PastBudget>().is_some() => Stop::Memory,
            None if error.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) => Stop::OutOfFuel,
            None => Stop::Trap,
        }
    }
}

/// Both runs of one module: every call must end alike and use the same fuel.
struct Both {
    original: Run,
    metered: Run,
}

impl Both {
    fn new(wat: &str, budget: u64) -> Self {
        let code = wat::parse_str(wat).unwrap();
        Self {
            original: Compiled::original(&code, budget).run(),
            metered: Compiled::metered(&meter_accepted(&code, budget), budget).run(),
        }
    }

    /// Calls both; asserts the same end and, when the call returned or ran
    /// out of fuel, the same fuel used as the host reports it: wasmtime's
    /// `get_fuel` stops at 0, so a metered count past the budget is capped
    /// at it (after any other trap neither runner has a count). Returns the
    /// end and the original's fuel used.
    fn call(&mut self, name: &str, args: &[i32], fuel: u64) -> (End, i64) {
        let original = self.original.call(name, args, fuel);
        let metered = self.metered.call(name, args, fuel);
        assert_eq!(original, metered, "{name}{args:?} with {fuel}");
        let fuel = i64::try_from(fuel).unwrap();
        let used = fuel - self.original.left();
        if matches!(original, End::Returned(_) | End::Stopped(Stop::OutOfFuel)) {
            assert_eq!(
                used,
                fuel.saturating_sub(self.metered.left()).min(fuel),
                "fuel used by {name}{args:?} with {fuel}"
            );
        }
        (original, used)
    }

    /// Calls `name` at every budget from 1 to 2 past what it uses: every
    /// point the fuel can run out at must end alike.
    fn every_budget(&mut self, name: &str, args: &[i32]) {
        let (_, total) = self.call(name, args, FUEL_PER_TICK);
        for fuel in 1..=total.unsigned_abs() + 2 {
            self.call(name, args, fuel);
        }
    }

    /// How far a call at `fuel` got: its end and the module's `trace`
    /// counter, the same under both (the counter is reset first).
    fn reach(&mut self, name: &str, args: &[i32], fuel: u64) -> (End, i32) {
        for run in [&mut self.original, &mut self.metered] {
            let trace = run.instance.get_global(&mut run.store, "trace").unwrap();
            trace.set(&mut run.store, Val::I32(0)).unwrap();
        }
        let (end, _) = self.call(name, args, fuel);
        let (original, metered) = self.global("trace");
        assert_eq!(original, metered, "trace of {name}{args:?} with {fuel}");
        (end, original)
    }

    /// [`Self::every_budget`] by bisection, for a module that bumps a
    /// `trace` counter after every size-charged operator: how a call ends
    /// and how far it gets only move forward as the budget grows, so where
    /// both runs agree at the two ends of a range of budgets, they agree all
    /// through it; a range whose ends differ is split until it is one
    /// budget wide. A check placed differently shows as a different trace.
    fn bisect_budgets(&mut self, name: &str, args: &[i32]) {
        let (_, total) = self.call(name, args, FUEL_PER_TICK);
        let high = total.unsigned_abs() + 2;
        let (at_low, at_high) = (self.reach(name, args, 1), self.reach(name, args, high));
        let mut ranges = vec![(1, at_low, high, at_high)];
        while let Some((low, at_low, high, at_high)) = ranges.pop() {
            if at_low == at_high || high - low <= 1 {
                continue;
            }
            let middle = low + (high - low) / 2;
            let at_middle = self.reach(name, args, middle);
            ranges.push((low, at_low, middle, at_middle.clone()));
            ranges.push((middle, at_middle, high, at_high));
        }
    }

    fn global(&mut self, name: &str) -> (i32, i32) {
        let read = |run: &mut Run| {
            run.instance
                .get_global(&mut run.store, name)
                .unwrap()
                .get(&mut run.store)
                .unwrap_i32()
        };
        (read(&mut self.original), read(&mut self.metered))
    }
}

// --- the recorded sessions -------------------------------------------------

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/sessions/{name}.gz", env!("CARGO_MANIFEST_DIR"));
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Tick {
    events: Vec<u8>,
    frame: Vec<u8>,
}

/// Lane 02's recording: per tick, u8 rendered, u32le len + the event batch,
/// u32le len + the frame the native view answered.
fn recording(bytes: &[u8]) -> Vec<Tick> {
    let mut at = 0;
    let chunk = |at: &mut usize| {
        let len = u32::from_le_bytes(bytes[*at..*at + 4].try_into().unwrap()) as usize;
        *at += 4 + len;
        bytes[*at - len..*at].to_vec()
    };
    let mut ticks = Vec::new();
    while at < bytes.len() {
        at += 1;
        let events = chunk(&mut at);
        let frame = chunk(&mut at);
        ticks.push(Tick { events, frame });
    }
    ticks
}

/// A seated view driven the way the app drives one.
struct Seat(Run);

impl Seat {
    fn func<P: wasmtime::WasmParams, R: wasmtime::WasmResults>(
        &mut self,
        name: &str,
    ) -> wasmtime::TypedFunc<P, R> {
        self.0
            .instance
            .get_typed_func(&mut self.0.store, name)
            .unwrap()
    }

    fn answer(&mut self, packed: u64) -> Vec<u8> {
        let memory = self
            .0
            .instance
            .get_memory(&mut self.0.store, "memory")
            .unwrap();
        let (at, len) = ((packed >> 32) as usize, (packed & 0xffff_ffff) as usize);
        memory.data(&self.0.store)[at..at + len].to_vec()
    }

    fn give(&mut self, bytes: &[u8]) -> (u32, u32) {
        let ptr = self
            .func::<u32, u32>("alloc")
            .call(&mut self.0.store, bytes.len() as u32)
            .unwrap();
        let memory = self
            .0
            .instance
            .get_memory(&mut self.0.store, "memory")
            .unwrap();
        memory
            .write(&mut self.0.store, ptr as usize, bytes)
            .unwrap();
        (ptr, bytes.len() as u32)
    }

    fn init(&mut self) {
        self.0.arm(FUEL_PER_TICK);
        self.func::<(), ()>("init")
            .call(&mut self.0.store, ())
            .unwrap();
    }

    /// One tick as the app runs it (armed once for alloc + write + tick):
    /// the frame and the fuel it used.
    fn tick(&mut self, events: &[u8]) -> (Vec<u8>, u64) {
        self.0.arm(FUEL_PER_TICK);
        let args = self.give(events);
        let packed = self
            .func::<(u32, u32), u64>("tick")
            .call(&mut self.0.store, args)
            .unwrap();
        let used = FUEL_PER_TICK - u64::try_from(self.0.left()).unwrap();
        (self.answer(packed), used)
    }

    fn snapshot(&mut self) -> Vec<u8> {
        self.0.arm(FUEL_PER_TICK);
        let packed = self
            .func::<(), u64>("snapshot")
            .call(&mut self.0.store, ())
            .unwrap();
        self.answer(packed)
    }

    fn restore(&mut self, body: &[u8]) -> u8 {
        self.0.arm(FUEL_PER_TICK);
        let args = self.give(body);
        let packed = self
            .func::<(u32, u32), u64>("restore")
            .call(&mut self.0.store, args)
            .unwrap();
        self.answer(packed)[0]
    }
}

/// S0-webkit's restore crosscheck (`replay.js restore`): run to tick `k`,
/// snapshot, a fresh instance restores it, the rest of the session is
/// replayed and its frames hashed (FNV-1a 64 over u32le length + bytes).
fn post_restore_hash(seat: impl Fn() -> Seat, ticks: &[Tick], k: usize) -> String {
    let mut first = seat();
    first.init();
    for tick in &ticks[..=k] {
        first.tick(&tick.events);
    }
    let state = first.snapshot();
    assert_eq!(state[0], 0, "the snapshot at tick {k} is accepted");
    let mut fresh = seat();
    assert_eq!(fresh.restore(&state[1..]), 0, "the restore is accepted");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for tick in &ticks[k + 1..] {
        let (frame, _) = fresh.tick(&tick.events);
        for byte in (frame.len() as u32).to_le_bytes().iter().chain(&frame) {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// The custom sections of a module (name, bytes) the host may read.
fn custom_sections(code: &[u8]) -> Vec<(String, Vec<u8>)> {
    wasmparser::Parser::new(0)
        .parse_all(code)
        .filter_map(|payload| match payload.unwrap() {
            // the pass rewrites `name` and drops the toolchain's `producers`
            wasmparser::Payload::CustomSection(section)
                if !["name", "producers"].contains(&section.name()) =>
            {
                Some((section.name().to_owned(), section.data().to_vec()))
            }
            _ => None,
        })
        .collect()
}

fn session(view: &str, wasm_sha256: &str, total: u64, k: usize, reference: &str) {
    let code = fixture(&format!("{view}_view.wasm"));
    assert_eq!(
        sha256(&code),
        wasm_sha256,
        "the {view} module the numbers are for"
    );
    let ticks = recording(&fixture(&format!("{view}-desk.rec")));
    let metered = meter_accepted(&code, VIEW_BYTES);
    assert_eq!(metered, meter_accepted(&code, VIEW_BYTES), "deterministic");
    assert_eq!(
        custom_sections(&code),
        custom_sections(&metered),
        "the view's custom sections (its manifest) are kept"
    );

    let (original, metered) = (
        Compiled::original(&code, VIEW_BYTES),
        Compiled::metered(&metered, VIEW_BYTES),
    );
    let (original, metered) = (|| Seat(original.run()), || Seat(metered.run()));
    let (mut a, mut b) = (original(), metered());
    a.init();
    b.init();
    let mut fuel = (0, 0);
    for (i, tick) in ticks.iter().enumerate() {
        let (frame_a, used_a) = a.tick(&tick.events);
        let (frame_b, used_b) = b.tick(&tick.events);
        assert!(
            frame_a == tick.frame,
            "{view} tick {i}: the original's frame"
        );
        assert!(frame_b == tick.frame, "{view} tick {i}: the metered frame");
        assert_eq!(used_a, used_b, "{view} tick {i}: fuel");
        fuel = (fuel.0 + used_a, fuel.1 + used_b);
    }
    assert_eq!(fuel, (total, total), "{view}: fuel over the session");
    assert_eq!(post_restore_hash(original, &ticks, k), reference);
    assert_eq!(post_restore_hash(metered, &ticks, k), reference);
}

#[test]
fn chat_session() {
    session(
        "chat",
        "1f18207dd8104e35c56f6588967e62f58536d51b727410b0eea01baef43f23e1",
        1_906_273_787,
        119,
        "bf0bc34cfbd3be03",
    );
}

#[test]
fn forge_session() {
    session(
        "forge",
        "7e88335107f087810fa28506a361cf2b5fbcb0a0c55151b38dc70b112e7399ee",
        1_325_536_710,
        92,
        "13577549968c5b26",
    );
}

// --- the edges -------------------------------------------------------------

const PAGE: u64 = 65_536;

#[test]
fn a_grow_past_the_budget_traps_as_memory_under_both() {
    // one budget for memory and tables, as ViewLimits keeps it: 4 pages and
    // 8 table elements, the table's own 4 already in it
    let budget = 4 * PAGE + 8 * 8;
    let mut both = Both::new(
        r#"(module
            (memory (export "memory") 1)
            (table 4 funcref)
            (func (export "grow") (param i32) (result i32)
                local.get 0 memory.grow)
            (func (export "grow_table") (param i32) (result i32)
                ref.null func local.get 0 table.grow 0))"#,
        budget,
    );
    let fuel = FUEL_PER_TICK;
    assert_eq!(both.call("grow", &[0], fuel).0, End::Returned(vec![1]));
    assert_eq!(both.call("grow", &[2], fuel).0, End::Returned(vec![1]));
    // the budget is asked before any size limit, even past 4 GiB
    assert_eq!(
        both.call("grow", &[70_000], fuel).0,
        End::Stopped(Stop::Memory)
    );
    assert_eq!(both.call("grow", &[2], fuel).0, End::Stopped(Stop::Memory));
    assert_eq!(both.call("grow", &[1], fuel).0, End::Returned(vec![3]));
    // memory is at its 4 pages: the tables' 4 more elements fit, 1 more not
    assert_eq!(
        both.call("grow_table", &[4], fuel).0,
        End::Returned(vec![4])
    );
    assert_eq!(
        both.call("grow_table", &[0], fuel).0,
        End::Returned(vec![8])
    );
    assert_eq!(
        both.call("grow_table", &[1], fuel).0,
        End::Stopped(Stop::Memory)
    );
}

#[test]
fn a_grow_past_the_modules_own_maximum_answers_minus_one_under_both() {
    let mut both = Both::new(
        r#"(module
            (memory 1 2)
            (table 1 2 funcref)
            (func (export "grow") (param i32) (result i32)
                local.get 0 memory.grow)
            (func (export "grow_table") (param i32) (result i32)
                ref.null func local.get 0 table.grow 0))"#,
        VIEW_BYTES,
    );
    assert_eq!(
        both.call("grow", &[2], FUEL_PER_TICK).0,
        End::Returned(vec![-1])
    );
    assert_eq!(
        both.call("grow", &[1], FUEL_PER_TICK).0,
        End::Returned(vec![1])
    );
    assert_eq!(
        both.call("grow_table", &[2], FUEL_PER_TICK).0,
        End::Returned(vec![-1])
    );
}

#[test]
fn a_grow_the_modules_maximum_refuses_holds_nothing_of_the_budget() {
    // room for 3 pages and 1 table element: the refused grows below each
    // fit the budget, so only what they would hold could fill it
    let mut both = Both::new(
        r#"(module
            (memory 1 2)
            (table 1 2 funcref)
            (func (export "grow") (param i32) (result i32)
                local.get 0 memory.grow)
            (func (export "grow_table") (param i32) (result i32)
                ref.null func local.get 0 table.grow 0))"#,
        3 * PAGE + 8,
    );
    let call = |both: &mut Both, name, delta| both.call(name, &[delta], FUEL_PER_TICK).0;
    // 3 pages asked, 2 allowed: memory stays at 1, so a second element fits
    assert_eq!(call(&mut both, "grow", 2), End::Returned(vec![-1]));
    assert_eq!(call(&mut both, "grow_table", 1), End::Returned(vec![1]));
    // 4 more elements asked, again and again, past the table's 2: the
    // memory's second page still fits after all of them
    for _ in 0..5_000 {
        assert_eq!(call(&mut both, "grow_table", 4), End::Returned(vec![-1]));
    }
    assert_eq!(call(&mut both, "grow", 1), End::Returned(vec![1]));
}

#[test]
fn size_charged_operators_cost_what_wasmtime_charges() {
    let mut both = Both::new(
        r#"(module
            (memory 1)
            (data $d "0123456789abcdef0123456789abcdef")
            (table 64 funcref)
            (elem $e func $f $f $f $f $f $f $f $f)
            (func $f)
            (func (export "memory_copy") (param i32)
                i32.const 0 i32.const 100 local.get 0 memory.copy)
            (func (export "memory_fill") (param i32)
                i32.const 0 i32.const 7 local.get 0 memory.fill)
            (func (export "memory_init") (param i32)
                i32.const 0 i32.const 0 local.get 0 memory.init $d)
            (func (export "table_init") (param i32)
                i32.const 0 i32.const 0 local.get 0 table.init $e)
            (func (export "table_copy") (param i32)
                i32.const 0 i32.const 10 local.get 0 table.copy)
            (func (export "table_fill") (param i32)
                i32.const 0 ref.null func local.get 0 table.fill 0)
            (func (export "table_grow") (param i32) (result i32)
                ref.null func local.get 0 table.grow 0)
            (func (export "copy_16")
                i32.const 0 i32.const 100 i32.const 16 memory.copy)
            (func (export "copy_1000")
                i32.const 0 i32.const 100 i32.const 1000 memory.copy))"#,
        VIEW_BYTES,
    );
    // a constant length is folded into the cost (wasmtime checks only past
    // 128); the count is the same either way
    let small = both.call("copy_16", &[], FUEL_PER_TICK).1;
    let large = both.call("copy_1000", &[], FUEL_PER_TICK).1;
    assert_eq!(large - small, 1000 - 16);
    for (name, sizes) in [
        ("memory_copy", [0, 1, 5000]),
        ("memory_fill", [0, 1, 5000]),
        ("memory_init", [0, 1, 32]),
        ("table_init", [0, 1, 8]),
        ("table_copy", [0, 1, 50]),
        ("table_fill", [0, 1, 64]),
        ("table_grow", [0, 1, 1000]),
    ] {
        let used: Vec<i64> = sizes
            .iter()
            .map(|&size| both.call(name, &[size], FUEL_PER_TICK).1)
            .collect();
        // each element or byte is one unit of fuel, under both
        assert_eq!(used[1] - used[0], 1, "{name}");
        assert_eq!(used[2] - used[0], i64::from(sizes[2]), "{name}");
    }
}

#[test]
fn fuel_runs_out_at_the_same_point_under_both() {
    let mut both = Both::new(
        r#"(module
            (global $n (export "n") (mut i32) (i32.const 0))
            (func $step
                global.get $n i32.const 1 i32.add global.set $n)
            (func (export "spin")
                (loop
                    global.get $n i32.const 1 i32.add global.set $n
                    br 0))
            (func (export "spin_calls")
                (loop call $step br 0)))"#,
        VIEW_BYTES,
    );
    for (name, fuel) in [
        ("spin", 10_000),
        ("spin", 10_003),
        ("spin_calls", 10_000),
        ("spin_calls", 10_001),
        ("spin_calls", FUEL_PER_TICK),
    ] {
        let start = both.global("n");
        assert_eq!(both.call(name, &[], fuel).0, End::Stopped(Stop::OutOfFuel));
        let end = both.global("n");
        assert_eq!(
            end.0 - start.0,
            end.1 - start.1,
            "{name} with {fuel}: iterations before the trap"
        );
    }
}

#[test]
fn an_import_in_a_table_is_still_reached_under_both() {
    let mut both = Both::new(
        r#"(module
            (import "ducktape_view" "panicked" (func $p (param i32 i32)))
            (type $t (func (param i32 i32)))
            (table 2 funcref)
            (elem (i32.const 0) $p $own)
            (func $own (param i32 i32))
            (func (export "through") (param i32)
                i32.const 1 i32.const 2 local.get 0 call_indirect (type $t))
            (func (export "direct")
                i32.const 1 i32.const 2 call $p))"#,
        VIEW_BYTES,
    );
    for index in [0, 1] {
        assert_eq!(
            both.call("through", &[index], FUEL_PER_TICK).0,
            End::Returned(vec![])
        );
    }
    assert_eq!(
        both.call("through", &[2], FUEL_PER_TICK).0,
        End::Stopped(Stop::Trap)
    );
    assert_eq!(
        both.call("direct", &[], FUEL_PER_TICK).0,
        End::Returned(vec![])
    );
}

#[test]
fn a_view_trap_stays_the_views_under_both() {
    let mut both = Both::new(
        r#"(module
            (memory 1)
            (func (export "oob") (result i32) i32.const 70000 i32.load)
            (func (export "panic") unreachable))"#,
        VIEW_BYTES,
    );
    assert_eq!(
        both.call("oob", &[], FUEL_PER_TICK).0,
        End::Stopped(Stop::Trap)
    );
    assert_eq!(
        both.call("panic", &[], FUEL_PER_TICK).0,
        End::Stopped(Stop::Trap)
    );
}

#[test]
fn tail_calls_threads_and_simd_are_refused_under_both() {
    for (what, wat) in [
        ("tail calls", r#"(module (func $f return_call $f))"#),
        ("threads", r#"(module (memory 1 1 shared))"#),
        (
            "simd",
            r#"(module (func (result v128) v128.const i64x2 0 0))"#,
        ),
    ] {
        let code = wat::parse_str(wat).unwrap();
        assert!(
            matches!(accepts(&code), Err(Refusal::Unsupported(_))),
            "{what}: the rule refuses it"
        );
        assert!(
            matches!(meter(&code, VIEW_BYTES), Err(Refusal::Unsupported(_))),
            "{what}: the pass refuses it"
        );
        for fuel in [true, false] {
            assert!(
                Module::new(engine(fuel), &code).is_err(),
                "{what}: wasmtime with FEATURES refuses it"
            );
        }
    }
}

#[test]
fn the_shapes_no_runner_takes_are_refused_wherever_asked() {
    let params = format!("(module (func (param {})))", "i32 ".repeat(1_000));
    let results = format!(
        "(module (func (result {}) {}))",
        "i32 ".repeat(1_000),
        "i32.const 0 ".repeat(1_000)
    );
    // the limits, refused with the margin of what the pass could add (the
    // other side of each: `the_limits_hold_once_metered`)
    let locals = format!("(module (func (local {})))", "i32 ".repeat(49_999));
    let segments = format!(
        "(module (func $f) {})",
        "(elem declare func $f) ".repeat(100_000)
    );
    let body = format!("(module (func {}))", "(loop) ".repeat(116_000));
    for (what, wat) in [
        (
            "an imported memory",
            r#"(module (import "x" "m" (memory 1)))"#,
        ),
        (
            "an imported table",
            r#"(module (import "x" "t" (table 1 funcref)))"#,
        ),
        (
            "an imported externref global",
            r#"(module (import "x" "g" (global externref)))"#,
        ),
        ("an externref table", r#"(module (table 1 externref))"#),
        (
            "an externref global",
            r#"(module (global (mut externref) (ref.null extern)))"#,
        ),
        ("a start function", r#"(module (func $s) (start $s))"#),
        (
            "an export named gas_left",
            r#"(module (global (export "gas_left") i32 (i32.const 0)))"#,
        ),
        ("1000 parameters", &params),
        ("1000 results", &results),
        (
            "49,999 locals (the pass adds 2; wasm allows 50,000)",
            &locals,
        ),
        ("100,000 element segments (the pass may add 1)", &segments),
        (
            "a body of 116,000 loops (metered, past 7,654,321 bytes)",
            &body,
        ),
    ] {
        let code = wat::parse_str(wat).unwrap();
        // valid wasm within FEATURES: refused by the rule, not by wasmtime
        assert!(Module::validate(engine(true), &code).is_ok(), "{what}");
        assert!(
            matches!(accepts(&code), Err(Refusal::Unsupported(_))),
            "{what}: not accepted"
        );
        assert!(
            matches!(meter(&code, VIEW_BYTES), Err(Refusal::Unsupported(_))),
            "{what}: not metered"
        );
    }
    for view in ["chat", "forge"] {
        assert_eq!(accepts(&fixture(&format!("{view}_view.wasm"))), Ok(()));
    }
}

#[test]
fn the_limits_hold_once_metered() {
    // one under each margin `the_shapes_no_runner_takes...` refuses at:
    // accepted, so metered into valid wasm (`meter` validates its output)
    // a parameter and 49,997 locals, all read, and a length from the
    // parameter: the count and the scratch local make exactly 50,000
    let gets: String = (0..49_998)
        .map(|i| format!("local.get {i} drop "))
        .collect();
    let locals = format!(
        "(module (memory 1) (func (export \"f\") (param i32) (local {}) {gets} \
         i32.const 0 i32.const 0 local.get 0 memory.fill))",
        "i32 ".repeat(49_997)
    );
    let segments = format!(
        r#"(module (import "m" "p" (func $p)) {}
            (func (export "f") (result i32) ref.func $p ref.is_null))"#,
        "(elem declare func $p) ".repeat(99_999)
    );
    let body = format!("(module (func {}))", "(loop) ".repeat(115_000));
    for wat in [&locals, &segments, &body] {
        meter_accepted(&wat::parse_str(wat).unwrap(), VIEW_BYTES);
    }
    // custom sections the validator does not read, garbage inside
    for name in ["name", "producers"] {
        let mut code = wat::parse_str(r#"(module (func (export "f")))"#).unwrap();
        let mut section = vec![u8::try_from(name.len()).unwrap()];
        section.extend_from_slice(name.as_bytes());
        section.extend_from_slice(&[0xff, 0xff, 0xff, 0x07, 0x01]);
        code.extend([0, u8::try_from(section.len()).unwrap()]);
        code.extend(section);
        meter_accepted(&code, VIEW_BYTES);
    }
}

#[test]
fn the_replay_is_of_the_locked_wasmtime() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    for (name, version) in [
        ("wasmtime-internal-cranelift", "46.0.3"),
        ("cranelift-frontend", "0.133.3"),
    ] {
        assert!(
            lock.contains(&format!("name = \"{name}\"\nversion = \"{version}\"\n")),
            "{name} is no longer {version}: re-audit constants.rs (the replay of its \
             translation) before changing this"
        );
    }
}

#[test]
fn hostile_or_oversized_input_is_a_typed_refusal() {
    let unsupported = |code: &[u8]| {
        matches!(accepts(code), Err(Refusal::Unsupported(_)))
            && matches!(meter(code, VIEW_BYTES), Err(Refusal::Unsupported(_)))
    };
    assert!(unsupported(b"\0asm\x01\0\0\0\x01\xff"));
    assert!(unsupported(b"not wasm at all"));
    assert!(unsupported(&[]));
    let big = wat::parse_str(r#"(module (memory 2) (table 8 funcref))"#).unwrap();
    assert_eq!(
        meter(&big, PAGE),
        Err(Refusal::Memory {
            needs: 2 * PAGE + 64,
            view_bytes: PAGE
        })
    );
    // no memory, no table: metered all the same
    let bare = wat::parse_str(r#"(module (func (export "f")))"#).unwrap();
    assert!(meter(&bare, VIEW_BYTES).is_ok());
}

#[test]
fn stop_reads_the_gas_left() {
    assert_eq!(stop(GROW_REFUSED), Stop::Memory);
    assert_eq!(stop(0), Stop::OutOfFuel);
    assert_eq!(stop(-41), Stop::OutOfFuel);
    assert_eq!(stop(1), Stop::Trap);
}

// --- where the fuel runs out: every budget ---------------------------------

/// `(drop (i32.const 1))` `n` times: `n` fuel of straight-line code.
fn burn(n: usize) -> String {
    "(drop (i32.const 1)) ".repeat(n)
}

#[test]
fn a_constant_length_is_folded_wherever_wasmtime_sees_one() {
    // a run of straight-line code around each size-charged operator: the
    // fuel can run out inside it, where only a check would stop the call
    let b = burn(8);
    let fill = |length: &str| format!("i32.const 0 i32.const 0 {length} memory.fill");
    let wat = format!(
        r#"(module
            (memory 1 1)
            (table $t 0 100 funcref)
            (global $g16 i32 (i32.const 16))
            (global $negative i32 (i32.const -1))
            (func (export "local") (local $n i32)
                i32.const 16 local.set $n {b} {fill_n} {b})
            (func (export "tee") (local $n i32)
                {b} i32.const 0 i32.const 0 i32.const 16 local.tee $n memory.fill {b})
            (func (export "table_grow")
                {b} ref.null func i32.const 1 table.grow $t drop {b})
            (func (export "table_grow_large")
                {b} ref.null func i32.const 200 table.grow $t drop {b})
            (func (export "across_a_block") (local $n i32)
                i32.const 16 local.set $n {b} (block {b}) {fill_n} {b})
            (func (export "across_br_if") (param $p i32) (local $n i32)
                i32.const 16 local.set $n
                (block local.get $p br_if 0 {b})
                {fill_n} {b})
            (func (export "if_arms_keep_it") (param $p i32) (local $n i32)
                i32.const 16 local.set $n
                local.get $p (if (then {b}) (else {b}))
                {fill_n} {b})
            (func (export "if_arms_set_it_apart") (param $p i32) (local $n i32)
                local.get $p
                (if (then i32.const 16 local.set $n) (else i32.const 16 local.set $n))
                {b} {fill_n} {b})
            (func (export "inside_a_loop") (local $n i32) (local $k i32)
                i32.const 16 local.set $n i32.const 2 local.set $k
                (loop {b} {fill_n} {b}
                    local.get $k i32.const 1 i32.sub local.tee $k br_if 0))
            (func (export "after_a_loop") (local $n i32) (local $k i32)
                i32.const 16 local.set $n i32.const 2 local.set $k
                (loop local.get $k i32.const 1 i32.sub local.tee $k br_if 0)
                {b} {fill_n} {b})
            (func (export "immutable_global") {b} {fill_g} {b})
            (func (export "zeroed_local") (local $n i32) {b} {fill_n} {b})
            (func (export "copied_local") (local $a i32) (local $n i32)
                i32.const 16 local.set $a local.get $a local.set $n {b} {fill_n} {b})
            (func (export "past_128") (local $n i32)
                i32.const 200 local.set $n {b} {fill_n} {b})
            (func (export "a_parameter") (param $n i32) {b} {fill_n} {b})
            (func (export "dead_branch") (param $p i32) (local $n i32)
                i32.const 16 local.set $n
                (block
                    local.get $p
                    (if (then
                        ;; past the memory's maximum: wasmtime stops
                        ;; translating, so the branch below adds no edge
                        i32.const 0 i32.load offset=70000 drop
                        i32.const 5 local.set $n br 1))
                    {b})
                {fill_n} {b})
            (func (export "negative_global") {b} {fill_neg} {b}))"#,
        fill_n = fill("local.get $n"),
        fill_g = fill("global.get $g16"),
        fill_neg = fill("global.get $negative"),
    );
    let mut both = Both::new(&wat, VIEW_BYTES);
    for name in [
        "local",
        "tee",
        "table_grow",
        "table_grow_large",
        "across_a_block",
        "inside_a_loop",
        "after_a_loop",
        "immutable_global",
        "zeroed_local",
        "copied_local",
        "past_128",
    ] {
        both.every_budget(name, &[]);
    }
    for name in [
        "across_br_if",
        "if_arms_keep_it",
        "if_arms_set_it_apart",
        "dead_branch",
        "a_parameter",
    ] {
        for arg in [0, 1, 16] {
            both.every_budget(name, &[arg]);
        }
    }
    // an immutable i32 global of -1 is sign-extended, as wasmtime's constant
    // is: a length of 2^64 - 1 that costs i64::MAX, so its run has no end to
    // sweep to; the low budgets cover it
    for fuel in 1..=64 {
        both.call("negative_global", &[], fuel);
    }
}

#[test]
fn every_operator_class_stops_where_wasmtime_stops() {
    let mut both = Both::new(
        r#"(module
            (type $ii (func (param i32) (result i32)))
            (memory 1)
            (data $d "abcdefghijklmnop")
            (table $t0 4 funcref)
            (table $t1 4 funcref)
            (elem (table $t0) (i32.const 0) func $inc $dbl)
            (elem (table $t1) (i32.const 0) func $dbl $inc)
            (elem $e func $inc $dbl)
            (global $n (mut i32) (i32.const 0))
            (func $inc (type $ii) local.get 0 i32.const 1 i32.add)
            (func $dbl (type $ii) local.get 0 i32.const 2 i32.mul)
            (func (export "br_table") (param $p i32) (result i32)
                (block (block (block local.get $p br_table 0 1 2)
                    i32.const 10 return)
                    i32.const 20 return)
                i32.const 30)
            (func (export "if_else") (param $p i32) (result i32)
                local.get $p (if (result i32) (then i32.const 1) (else i32.const 2)))
            (func (export "multi_value") (param $p i32) (result i32 i32)
                local.get $p i32.const 1
                (block (param i32 i32) (result i32 i32) i32.add i32.const 3))
            (func (export "nested_loops") (param $p i32) (result i32)
                (local $i i32) (local $j i32) (local $s i32)
                (loop $outer
                    i32.const 0 local.set $j
                    (loop $inner
                        local.get $s i32.const 1 i32.add local.set $s
                        local.get $j i32.const 1 i32.add local.tee $j
                        i32.const 3 i32.lt_u br_if $inner)
                    local.get $i i32.const 1 i32.add local.tee $i
                    local.get $p i32.lt_u br_if $outer)
                local.get $s)
            (func (export "indirect_0") (param $p i32) (result i32)
                i32.const 5 local.get $p call_indirect $t0 (type $ii))
            (func (export "indirect_1") (param $p i32) (result i32)
                i32.const 5 local.get $p call_indirect $t1 (type $ii))
            (func (export "table_get_set") (param $p i32)
                local.get $p (table.get $t0 (local.get $p)) table.set $t1)
            (func (export "typed_select") (param $p i32) (result i32)
                i32.const 1 i32.const 2 local.get $p select (result i32))
            (func (export "early_return") (param $p i32) (result i32)
                local.get $p (if (then i32.const 7 return)) i32.const 8)
            (func (export "br_to_the_function") (param $p i32) (result i32)
                i32.const 9 local.get $p br_if 0 drop i32.const 10)
            (func (export "segments") (param $p i32)
                i32.const 0 i32.const 0 local.get $p memory.init $d
                data.drop $d
                i32.const 0 i32.const 0 local.get $p table.init $t0 $e
                elem.drop $e)
            (func $recursion (export "recursion") (param $p i32) (result i32)
                local.get $p
                (if (result i32)
                    (then local.get $p i32.const 1 i32.sub call $recursion
                        i32.const 1 i32.add)
                    (else i32.const 0)))
            (func (export "dead_code") (param $p i32) (result i32)
                (block local.get $p br_if 0 i32.const 1 drop br 0 i32.const 2 drop)
                i32.const 3)
            (func (export "fill_copy") (param $p i32)
                i32.const 0 i32.const 7 local.get $p memory.fill
                i32.const 100 i32.const 0 local.get $p memory.copy
                i32.const 0 i32.const 1 local.get $p table.copy $t0 $t0)
            (func (export "divide") (param $p i32) (result i32)
                i32.const 10 local.get $p i32.div_u)
            (func (export "grows") (param $p i32) (result i32)
                local.get $p memory.grow
                ref.null func local.get $p table.grow $t1 i32.add)
            (func (export "counter") (param $p i32)
                (loop global.get $n i32.const 1 i32.add global.set $n
                    local.get $p i32.const 1 i32.sub local.tee $p br_if 0)))"#,
        VIEW_BYTES,
    );
    for (name, args) in [
        ("br_table", &[0, 1, 2, 7][..]),
        ("if_else", &[0, 1]),
        ("multi_value", &[4]),
        ("nested_loops", &[1, 3]),
        ("indirect_0", &[0, 1, 2, 9]),
        ("indirect_1", &[0, 1, 3]),
        ("table_get_set", &[0, 1]),
        ("typed_select", &[0, 1]),
        ("early_return", &[0, 1]),
        ("br_to_the_function", &[0, 1]),
        ("recursion", &[0, 5]),
        ("dead_code", &[0, 1]),
        ("fill_copy", &[0, 3, 200]),
        ("divide", &[2, 0]),
        ("grows", &[0, 1]),
        ("counter", &[1, 6]),
        // last: the first call drops the segments, the next ones trap
        ("segments", &[0, 2]),
    ] {
        for &arg in args {
            both.every_budget(name, &[arg]);
        }
    }
}

/// A seeded xorshift: the random modules are the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len() as u64) as usize]
    }
}

/// An enclosing label, as the generator sees it.
#[derive(Clone, Copy)]
enum Label {
    /// a block or an `if` without values: a branch target
    Block,
    /// a loop counted down on `$k{depth}`: reached again only through it
    Loop(usize),
    /// a label with values: never branched to
    Typed,
}

/// Bumps the `trace` global after a size-charged operator: a check placed
/// differently there shows in how far the call got.
const TRACE: &str = "global.get $trace i32.const 1 i32.add global.set $trace ";

/// A random statement list: constants moved through locals (spread over
/// several declarations), blocks, `if`s, loops and blocks with parameters,
/// counted loops and continues into them from nested blocks, branches,
/// `br_table` with and without values, `select`, `call_indirect`,
/// `table.get`, every size-charged operator with a length from all of that,
/// and a load past the memory's maximum in an arm never taken. `labels`
/// holds the enclosing labels. (The review of #547 wrote it; it replaced a
/// narrower one.)
fn statements(rng: &mut Rng, depth: usize, labels: &mut Vec<Label>, out: &mut String) {
    const LOCALS: &[&str] = &["$a", "$b", "$c"];
    const VALUES: &[&str] = &["0", "3", "16", "128", "129", "200"];
    for _ in 0..1 + rng.below(5) {
        let l = rng.pick(LOCALS);
        let l2 = rng.pick(LOCALS);
        let v = rng.pick(VALUES);
        let v2 = rng.pick(VALUES);
        let cond = rng.pick(&["$p", l2]);
        let n = labels.len();
        let blocks: Vec<usize> = (0..n)
            .filter(|&d| matches!(labels[n - 1 - d], Label::Block))
            .collect();
        let loops: Vec<(usize, usize)> = (0..n)
            .filter_map(|d| match labels[n - 1 - d] {
                Label::Loop(k) => Some((d, k)),
                _ => None,
            })
            .collect();
        match rng.below(if depth < 3 { 33 } else { 23 }) {
            0 => *out += &format!("i32.const {v} local.set {l} "),
            1 => *out += &format!("local.get {l2} local.set {l} "),
            2 => *out += &format!("local.get $p local.set {l} "),
            3 => *out += &format!("i32.const {v} local.tee {l} drop "),
            4 => *out += &format!("i32.const 0 i32.const 0 local.get {l} memory.fill {TRACE}"),
            5 => *out += &format!("ref.null func local.get {l} table.grow $g drop {TRACE}"),
            6 => {
                let global = rng.pick(&["$g16", "$g200"]);
                *out += &format!("global.get {global} local.set {l} ");
            }
            7 => *out += &burn(1 + rng.below(4) as usize),
            8 => *out += &format!("i32.const 100 i32.const 0 local.get {l} memory.copy {TRACE}"),
            9 if !blocks.is_empty() => {
                let target = blocks[rng.below(blocks.len() as u64) as usize];
                *out += &format!("local.get {cond} br_if {target} ");
            }
            10 if !blocks.is_empty() && rng.below(4) == 0 => {
                let target = blocks[rng.below(blocks.len() as u64) as usize];
                *out += &format!("br {target} ");
                return;
            }
            11 if !loops.is_empty() => {
                let (target, k) = loops[rng.below(loops.len() as u64) as usize];
                *out += &format!(
                    "local.get $k{k} i32.const 1 i32.sub local.tee $k{k} i32.const 0 i32.gt_s \
                     br_if {target} "
                );
            }
            12 => *out += "i32.const 0 call_indirect $w (type $v) ",
            13 => {
                *out += &format!(
                    "local.get {l2} i32.const 1 call_indirect $w (type $ii) local.set {l} "
                )
            }
            14 => *out += "i32.const 1 table.get $w drop ",
            15 => *out += &format!("i32.const 0 ref.null func local.get {l} table.fill $u {TRACE}"),
            16 => {
                *out += &format!("i32.const 0 i32.const 0 local.get {l} table.copy $u $u {TRACE}")
            }
            17 => {
                *out += &format!("i32.const 0 i32.const 0 local.get {l} table.init $u $e {TRACE}")
            }
            18 => *out += &format!("i32.const 0 i32.const 0 local.get {l} memory.init $d {TRACE}"),
            19 => {
                *out += &format!("i32.const {v} local.get {l2} local.get $p select local.set {l} ")
            }
            20 => {
                *out += &format!(
                    "i32.const 0 i32.const 0 i32.const {v} local.tee {l} memory.fill {TRACE}"
                )
            }
            21 => {
                *out += &format!(
                    "(block (result i32) i32.const {v} local.get $p br_if 0 drop i32.const {v2}) \
                     local.set {l} "
                )
            }
            22 => {
                *out += &format!(
                    "(block (result i32) (block (result i32) i32.const {v} local.get $p \
                     br_table 0 1 0) drop i32.const {v2}) local.set {l} "
                )
            }
            23 | 24 => {
                *out += "(block ";
                labels.push(Label::Block);
                statements(rng, depth + 1, labels, out);
                labels.pop();
                *out += ") ";
            }
            25 => {
                *out += &format!("local.get {cond} (if (then ");
                labels.push(Label::Block);
                statements(rng, depth + 1, labels, out);
                if rng.below(2) == 0 {
                    *out += ") (else ";
                    statements(rng, depth + 1, labels, out);
                }
                labels.pop();
                *out += ")) ";
            }
            26 => {
                *out += &format!("i32.const {} local.set $k{depth} (loop ", 1 + rng.below(3));
                labels.push(Label::Loop(depth));
                statements(rng, depth + 1, labels, out);
                labels.pop();
                *out += &format!(
                    "local.get $k{depth} i32.const 1 i32.sub local.tee $k{depth} i32.const 0 \
                     i32.gt_s br_if 0) "
                );
            }
            27 => {
                *out += "(block (block ";
                labels.extend([Label::Block, Label::Block]);
                statements(rng, depth + 1, labels, out);
                // the two new blocks, or an outer block two labels further
                let pick = |rng: &mut Rng| match rng.below(2 + blocks.len() as u64) as usize {
                    new @ 0..=1 => new,
                    outer => blocks[outer - 2] + 2,
                };
                let (first, second, default) = (pick(rng), pick(rng), pick(rng));
                *out += &format!("local.get $p br_table {first} {second} {default}) ");
                labels.pop();
                statements(rng, depth + 1, labels, out);
                *out += ") ";
                labels.pop();
            }
            28 => {
                *out += "i32.const 0 (if (then i32.const 0 i32.load offset=70000 drop ";
                labels.push(Label::Block);
                statements(rng, depth + 1, labels, out);
                labels.pop();
                *out += ")) ";
            }
            29 => {
                *out += &format!(
                    "local.get {l2} local.get {cond} (if (param i32) (then local.set {l} "
                );
                labels.push(Label::Block);
                statements(rng, depth + 1, labels, out);
                *out += &format!(") (else local.set {l} ");
                statements(rng, depth + 1, labels, out);
                labels.pop();
                *out += ")) ";
            }
            30 => {
                let l3 = rng.pick(LOCALS);
                *out += &format!(
                    "local.get {l2} local.get {cond} (if (param i32) (result i32) (then \
                     local.tee {l} "
                );
                labels.push(Label::Typed);
                statements(rng, depth + 1, labels, out);
                if rng.below(2) == 0 {
                    *out += &format!(") (else local.tee {l3} ");
                    statements(rng, depth + 1, labels, out);
                }
                labels.pop();
                *out += &format!(")) local.set {l} ");
            }
            31 => {
                *out += &format!("local.get {l2} (loop (param i32) local.set {l} ");
                labels.push(Label::Typed);
                statements(rng, depth + 1, labels, out);
                labels.pop();
                *out += ") ";
            }
            32 => {
                *out += &format!("local.get {l2} (block (param i32) local.set {l} ");
                labels.push(Label::Block);
                statements(rng, depth + 1, labels, out);
                labels.pop();
                *out += ") ";
            }
            _ => *out += "nop ",
        }
    }
}

/// The module the random bodies run in: two call targets in a table, a
/// table to grow, one to fill, copy and init, a data and an element segment.
fn random_module(seed: u64) -> String {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let mut functions = String::new();
    for index in 0..RANDOM_FUNCTIONS {
        let mut body = String::new();
        statements(&mut rng, 0, &mut Vec::new(), &mut body);
        let locals = match index % 2 {
            0 => "(local $a i32) (local $x i64) (local $b i32) (local $y f32) (local $c i32)",
            _ => "(local $a i32) (local $b i32) (local $c i32)",
        };
        functions += &format!(
            "(func (export \"f{index}\") (param $p i32) {locals} (local $k0 i32) (local $k1 i32) \
             (local $k2 i32) {body})\n"
        );
    }
    format!(
        r#"(module
            (type $v (func))
            (type $ii (func (param i32) (result i32)))
            (import "ducktape_view" "panicked" (func $panicked (param i32 i32)))
            (memory 1 1)
            (data $d "{data}")
            (table $g 0 1000 funcref)
            (table $u 256 funcref)
            (table $w 2 funcref)
            (elem (table $w) (i32.const 0) func $nop $id)
            (elem $e func {elements})
            (global $g16 i32 (i32.const 16))
            (global $g200 i32 (i32.const 200))
            (global $trace (export "trace") (mut i32) (i32.const 0))
            (func $nop (type $v))
            (func $id (type $ii) local.get 0)
            {functions})"#,
        data = "x".repeat(256),
        elements = " $nop".repeat(256),
    )
}

const RANDOM_FUNCTIONS: usize = 24;

#[test]
fn random_bodies_stop_where_wasmtime_stops() {
    for seed in 1..=16 {
        let code = wat::parse_str(random_module(seed)).unwrap();
        let original = Compiled::original(&code, VIEW_BYTES);
        let metered = Compiled::metered(&meter_accepted(&code, VIEW_BYTES), VIEW_BYTES);
        for index in 0..RANDOM_FUNCTIONS {
            for arg in [0, 1, 300] {
                // a fresh instance each: no table grown by an earlier call
                let mut both = Both {
                    original: original.run(),
                    metered: metered.run(),
                };
                both.bisect_budgets(&format!("f{index}"), &[arg]);
            }
        }
    }
}

// --- what the pass costs on hostile shapes ---------------------------------

#[test]
fn a_grow_costs_the_same_however_many_tables() {
    let module = |tables: usize| {
        let tables = "(table 1 funcref) ".repeat(tables);
        let grows = "(drop (memory.grow (local.get 0))) \
                     (drop (table.grow 0 (ref.null func) (local.get 0))) "
            .repeat(2_000);
        wat::parse_str(format!(
            "(module (memory 1) {tables} (func (export \"f\") (param i32) {grows}))"
        ))
        .unwrap()
    };
    let (one, many) = (module(1), module(100));
    let (one, many) = (
        meter_accepted(&one, VIEW_BYTES).len(),
        meter_accepted(&many, VIEW_BYTES).len(),
    );
    // the 99 more tables cost their declarations and one helper's lines,
    // not a copy at each of the 4,000 grows
    assert!(many - one < 4_096, "1 table {one} bytes, 100 tables {many}");
}

#[test]
fn a_ref_func_of_a_function_only_its_export_declares_is_metered() {
    // the export names a wrapper now, or the import's adapter stands in:
    // the function the ref.func names is declared in its place
    let mut both = Both::new(
        r#"(module
            (import "ducktape_view" "panicked" (func $p (param i32 i32)))
            (export "p" (func $p))
            (func $own (export "own"))
            (func (export "import") (result i32) ref.func $p ref.is_null)
            (func (export "local") (result i32) ref.func $own ref.is_null))"#,
        VIEW_BYTES,
    );
    for name in ["import", "local"] {
        assert_eq!(
            both.call(name, &[], FUEL_PER_TICK).0,
            End::Returned(vec![0])
        );
    }
}
