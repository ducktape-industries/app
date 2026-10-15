//! The wasmtime [`ViewRunner`]: the desktop's and Android's. It compiles a
//! view's code with Cranelift on the loader thread (`prepare`), keeps the
//! compiled module by the code's sha256, and runs each instance on the
//! calling thread: every `begin_*` lands its answer on the seat's
//! [`Landing`] before it returns. Fuel ([`FUEL_PER_TICK`]) bounds each
//! call; `ViewLimits` bounds memory and tables at the `memory.view_bytes`
//! setting `prepare` was given; a grow past it traps as [`Fault::Memory`].
//!
//! The wasm side of a view is here and nowhere else in the host: its store,
//! the panic hook it imports (`ducktape_view.panicked`) and the five
//! exports the bytes cross through (`wire::abi`).

use crate::contract::{
    FUEL_PER_TICK, Failure, Fault, Landed, Landing, RunnerConfig, SeatId, ViewInstance, ViewRunner,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use view_wire as wire;
use wasmtime::{
    Cache, CacheConfig, Caller, Config, Engine, Linker, Memory, Module, OptLevel, Store, TypedFunc,
};

/// The one engine, its compile cache, and the code it compiled.
pub struct WasmtimeRunner {
    engine: Engine,
    /// A handle on the engine's disk cache: `Cache` is `Clone`, and the
    /// clone reads the hit counters the engine's copy writes.
    disk: Option<Cache>,
    /// Compiled view code by the sha256 of its bytes, with the
    /// `view_bytes` it was last prepared for.
    /// ponytail: kept for the process's life, one module per distinct view
    /// code prepared (a deployment's, an override file's each time it
    /// changes); release by hash when the seats that held it are gone if a
    /// long session's memory says so.
    code: Mutex<HashMap<[u8; 32], Prepared>>,
    /// Describe modules by the sha256 of their bytes; `None` for bytes that
    /// do not compile.
    described: Mutex<HashMap<[u8; 32], Option<Arc<Module>>>>,
}

/// Compiled code, and the `view_bytes` it was last prepared for.
type Prepared = (Arc<Module>, usize);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl WasmtimeRunner {
    /// Cranelift at `Speed`, fuel on, the shell's trap mode, and the disk
    /// cache under `config.cache_dir` when there is one.
    pub fn new(config: RunnerConfig) -> Self {
        let mut wasm = Config::new();
        wasm.cranelift_opt_level(OptLevel::Speed);
        wasm.consume_fuel(true);
        wasm.signals_based_traps(config.signals_based_traps);
        let mut disk = None;
        if let Some(dir) = config.cache_dir {
            let mut cache = CacheConfig::new();
            cache.with_directory(dir);
            match Cache::new(cache) {
                Ok(cache) => {
                    disk = Some(cache.clone());
                    wasm.cache(Some(cache));
                }
                Err(error) => tracing::warn!(reason = "view_cache_unavailable", %error),
            }
        }
        Self {
            engine: Engine::new(&wasm).expect("wasmtime engine"),
            disk,
            code: Mutex::default(),
            described: Mutex::default(),
        }
    }

    /// The engine, for the tests that build a module by hand.
    #[cfg(any(test, feature = "test-support"))]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    fn instance(&self, hash: [u8; 32], land: &Landing) -> Result<WasmtimeInstance, Fault> {
        let Some((module, view_bytes)) = lock(&self.code).get(&hash).cloned() else {
            return Err(Fault::Runner("the view's code was not prepared".into()));
        };
        let mut store = Store::new(
            &self.engine,
            HostState {
                limits: ViewLimits::new(view_bytes),
                panic: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        // A view's one import is the panic hook's (`wire::abi`); anything
        // else the module asks for traps if it is ever called.
        let mut linker = Linker::<HostState>::new(&self.engine);
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
                                .get(start..start.saturating_add(len as usize))?;
                            Some(String::from_utf8_lossy(bytes).into_owned())
                        })
                        .unwrap_or_default();
                    let line = message.lines().next().unwrap_or_default();
                    caller.data_mut().panic = Some(line.to_owned());
                },
            )
            .map_err(|error| Fault::Runner(error.to_string()))?;
        linker
            .define_unknown_imports_as_traps(&module)
            .map_err(|error| Fault::Runner(error.to_string()))?;
        arm(&mut store);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|error| Fault::Trap(first_line(&error)))?;
        let exports =
            Exports::bind(&mut store, &instance).map_err(|error| Fault::Trap(error.to_string()))?;
        Ok(WasmtimeInstance {
            hash,
            store,
            exports,
            land: land.clone(),
            _module: module,
        })
    }
}

impl ViewRunner for WasmtimeRunner {
    fn prepare(
        &self,
        hash: [u8; 32],
        code: &[u8],
        view_bytes: usize,
    ) -> Result<&'static str, Failure> {
        if let Some((_, bytes)) = lock(&self.code).get_mut(&hash) {
            *bytes = view_bytes;
            return Ok("memory");
        }
        // compiled outside the lock, so unrelated views prepare at once; a
        // competing compile of the same code adopts the one kept first.
        // ponytail: the disk/cold split reads wasmtime's process-wide hit
        // counter around the compile, and loaders compile concurrently on
        // purpose, so a hit landing from another thread can name a cold
        // compile `disk`.
        let before = self.disk.as_ref().map(Cache::cache_hits);
        let module =
            Module::new(&self.engine, code).map_err(|error| Failure::Runner(error.to_string()))?;
        let source = match (before, self.disk.as_ref().map(Cache::cache_hits)) {
            (Some(before), Some(after)) if after > before => "disk",
            _ => "cold",
        };
        lock(&self.code)
            .entry(hash)
            .or_insert_with(|| (Arc::new(module), view_bytes))
            .1 = view_bytes;
        Ok(source)
    }

    fn instantiate(&self, hash: [u8; 32], _: SeatId, land: Landing) {
        let landed = match self.instance(hash, &land) {
            Ok(instance) => Landed::Seated(Box::new(instance)),
            Err(fault) => Landed::Fault(fault),
        };
        land.send(landed);
    }

    fn describe(
        &self,
        hash: [u8; 32],
        module: &[u8],
        op: &[u8],
    ) -> Option<view_wire::methods::Description> {
        let compiled = lock(&self.described).get(&hash).cloned();
        let compiled = match compiled {
            Some(compiled) => compiled,
            None => {
                let compiled = ::describe::host::compile(&self.engine, module).map(Arc::new);
                lock(&self.described)
                    .entry(hash)
                    .or_insert(compiled)
                    .clone()
            }
        };
        ::describe::host::run(&self.engine, &*compiled?, op)
    }
}

/// One view's store and exports, and the landing its answers go to.
pub struct WasmtimeInstance {
    hash: [u8; 32],
    store: Store<HostState>,
    exports: Exports,
    land: Landing,
    /// The code, held while an instance of it runs.
    _module: Arc<Module>,
}

impl WasmtimeInstance {
    /// Why a call failed: the view's own panic words when its hook handed
    /// some over, else the trap.
    fn fault(&mut self, error: &wasmtime::Error) -> Fault {
        if let Some(words) = panic_message(&mut self.store) {
            return Fault::Panicked(words);
        }
        let root = error.root_cause();
        let trap = error
            .downcast_ref::<wasmtime::Trap>()
            .or_else(|| root.downcast_ref::<wasmtime::Trap>());
        if trap == Some(&wasmtime::Trap::OutOfFuel) {
            return Fault::OutOfFuel;
        }
        if error.downcast_ref::<PastViewMemory>().is_some()
            || root.downcast_ref::<PastViewMemory>().is_some()
        {
            return Fault::Memory;
        }
        Fault::Trap(first_line(error))
    }

    /// The view's linear memory, for the tests that read it.
    #[cfg(test)]
    pub fn memory_for_test(&self) -> &[u8] {
        self.exports.memory.data(&self.store)
    }

    /// The view's linear memory, for the tests that script it.
    #[cfg(test)]
    pub fn memory_mut_for_test(&mut self) -> &mut [u8] {
        self.exports.memory.data_mut(&mut self.store)
    }
}

impl ViewInstance for WasmtimeInstance {
    fn code_hash(&self) -> [u8; 32] {
        self.hash
    }

    fn begin_init(&mut self) {
        arm(&mut self.store);
        let landed = match self.exports.init(&mut self.store) {
            Ok(()) => Landed::Inited,
            Err(error) => Landed::Fault(self.fault(&error)),
        };
        self.land.send(landed);
    }

    fn begin_tick(&mut self, events: Vec<u8>) {
        arm(&mut self.store);
        let landed = match self.exports.tick(&mut self.store, &events) {
            Ok(frame) => Landed::Frame(frame),
            Err(error) => Landed::Fault(self.fault(&error)),
        };
        self.land.send(landed);
    }

    fn begin_snapshot(&mut self) {
        arm(&mut self.store);
        let landed = match self.exports.snapshot(&mut self.store) {
            Ok(Ok(state)) => Landed::Snapshot(state),
            Ok(Err(refusal)) => Landed::SnapshotRefused(refusal),
            Err(error) => Landed::Fault(self.fault(&error)),
        };
        self.land.send(landed);
    }

    fn begin_restore(&mut self, snapshot: Vec<u8>) {
        arm(&mut self.store);
        let landed = match self.exports.restore(&mut self.store, &snapshot) {
            Ok(Ok(())) => Landed::Restored,
            Ok(Err(refusal)) => Landed::RestoreRefused(refusal),
            Err(error) => Landed::Fault(self.fault(&error)),
        };
        self.land.send(landed);
    }

    fn fuel_used(&self) -> u64 {
        FUEL_PER_TICK - self.store.get_fuel().unwrap_or(0)
    }

    fn memory_bytes(&self) -> usize {
        self.exports.memory.data_size(&self.store)
    }

    fn stop(self: Box<Self>) {}
}

/// Reset the instruction allowance before entering the view.
fn arm(store: &mut Store<HostState>) {
    let _ = store.set_fuel(FUEL_PER_TICK);
}

/// What a view's store holds: its limits, and the message its panic hook
/// handed over before the trap that follows.
struct HostState {
    limits: ViewLimits,
    panic: Option<String>,
}

/// A grow past the `memory.view_bytes` setting, as the trap that ends the
/// call.
#[derive(Debug)]
struct PastViewMemory;

impl std::fmt::Display for PastViewMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Fault::Memory.fmt(f)
    }
}

impl std::error::Error for PastViewMemory {}

struct ViewLimits {
    budget: usize,
    memory: usize,
    tables: usize,
}

impl ViewLimits {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            memory: 0,
            tables: 0,
        }
    }

    fn fits(&self, memory: usize, tables: usize) -> wasmtime::Result<()> {
        match memory.saturating_add(tables) <= self.budget {
            true => Ok(()),
            false => Err(wasmtime::Error::new(PastViewMemory)),
        }
    }
}

impl wasmtime::ResourceLimiter for ViewLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.fits(desired, self.tables)?;
        self.memory = desired;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let added = desired.saturating_sub(current);
        let tables =
            (self.tables).saturating_add(added.saturating_mul(std::mem::size_of::<usize>()));
        self.fits(self.memory, tables)?;
        self.tables = tables;
        Ok(true)
    }

    fn instances(&self) -> usize {
        1
    }

    fn memories(&self) -> usize {
        1
    }
}

/// The view's exports and the memory their bytes cross in (`wire::abi`).
struct Exports {
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    init: TypedFunc<(), ()>,
    tick: TypedFunc<(u32, u32), u64>,
    snapshot: TypedFunc<(), u64>,
    restore: TypedFunc<(u32, u32), u64>,
}

impl Exports {
    fn bind(store: &mut Store<HostState>, instance: &wasmtime::Instance) -> wasmtime::Result<Self> {
        Ok(Self {
            memory: instance
                .get_memory(&mut *store, "memory")
                .ok_or_else(|| wasmtime::Error::msg("the view exports no memory"))?,
            alloc: instance.get_typed_func(&mut *store, "alloc")?,
            init: instance.get_typed_func(&mut *store, "init")?,
            tick: instance.get_typed_func(&mut *store, "tick")?,
            snapshot: instance.get_typed_func(&mut *store, "snapshot")?,
            restore: instance.get_typed_func(&mut *store, "restore")?,
        })
    }

    /// `bytes` in a buffer the view allocated and owns from the next call on.
    fn give(&self, store: &mut Store<HostState>, bytes: &[u8]) -> wasmtime::Result<(u32, u32)> {
        let len = u32::try_from(bytes.len())?;
        let ptr = self.alloc.call(&mut *store, len)?;
        self.memory.write(&mut *store, ptr as usize, bytes)?;
        Ok((ptr, len))
    }

    /// The bytes an answer names, copied out before the view is entered
    /// again; nothing about the pair is trusted.
    fn answer(&self, store: &Store<HostState>, packed: u64) -> wasmtime::Result<Vec<u8>> {
        let (ptr, len) = wire::abi::unpack(packed);
        let start = ptr as usize;
        self.memory
            .data(store)
            .get(start..start.saturating_add(len as usize))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| wasmtime::Error::msg("the view answered outside its memory"))
    }

    fn result(
        &self,
        store: &Store<HostState>,
        packed: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        wire::abi::decode_result(&self.answer(store, packed)?)
            .ok_or_else(|| wasmtime::Error::msg("the view's answer is not a result"))
    }

    fn init(&self, store: &mut Store<HostState>) -> wasmtime::Result<()> {
        self.init.call(store, ())
    }

    fn tick(&self, store: &mut Store<HostState>, events: &[u8]) -> wasmtime::Result<Vec<u8>> {
        let (ptr, len) = self.give(store, events)?;
        let packed = self.tick.call(&mut *store, (ptr, len))?;
        self.answer(store, packed)
    }

    /// The view's state, or its own word that it cannot hand it over now.
    fn snapshot(&self, store: &mut Store<HostState>) -> wasmtime::Result<Result<Vec<u8>, String>> {
        let packed = self.snapshot.call(&mut *store, ())?;
        self.result(store, packed)
    }

    fn restore(
        &self,
        store: &mut Store<HostState>,
        state: &[u8],
    ) -> wasmtime::Result<Result<(), String>> {
        let (ptr, len) = self.give(store, state)?;
        let packed = self.restore.call(&mut *store, (ptr, len))?;
        Ok(self.result(store, packed)?.map(|_| ()))
    }
}

fn panic_message(store: &mut Store<HostState>) -> Option<String> {
    let text = store.data_mut().panic.take()?;
    (!text.is_empty()).then_some(text)
}

/// Why a call failed: the trap itself, not the wrapper and backtrace
/// wasmtime prints around it.
fn first_line(error: &wasmtime::Error) -> String {
    error
        .root_cause()
        .to_string()
        .lines()
        .next()
        .unwrap_or("trap")
        .to_string()
}
