//! The wasmtime [`ViewRunner`]: the desktop's and Android's. It compiles a
//! view's code with Cranelift on the loader thread (`prepare`), once
//! `view_meter::accepts` takes it (the rule the iOS runner applies too, so
//! a view runs on every platform or on none), finds the
//! compiled module by the code's sha256 while anything holds it, and runs
//! each instance on the
//! calling thread: every `begin_*` lands its answer on the seat's
//! [`Landing`] before it returns. A call runs until it returns or traps;
//! `ViewLimits` bounds memory and tables at the `memory.view_bytes`
//! setting `prepare` was given; a grow past it traps as [`Fault::Memory`].
//!
//! Compiled code is kept across launches as one file per code
//! ([`code_file`]), mapped as it is (`Module::deserialize_file`): clean
//! pages the OS shares and drops, where a decompressed copy (wasmtime's own
//! cache) would be this process's, and the heap its read went through.
//!
//! The wasm side of a view is here and nowhere else in the host: its store,
//! the panic hook it imports (`ducktape_view.panicked`) and the five
//! exports the bytes cross through (`wire::abi`).

use crate::contract::{
    CodeSource, Failure, Fault, Landed, Landing, Prepared, RunnerConfig, SeatId, ViewInstance,
    ViewRunner,
};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};
use view_wire as wire;
use wasmtime::{Caller, Config, Engine, Linker, Memory, Module, OptLevel, Store, TypedFunc};

/// The one engine, the compiled code kept across launches, and the code it
/// compiled.
pub struct WasmtimeRunner {
    engine: Engine,
    /// Where compiled code is kept across launches ([`code_file`]); none
    /// keeps it in memory only.
    dir: Option<PathBuf>,
    /// The engine's settings, as [`code_file`] names them.
    settings: u64,
    /// Compiled view code by the sha256 of its bytes, with the
    /// `view_bytes` it was last prepared for, while anything holds it
    /// (`Prepared::hold`, a running instance): code nothing holds is freed,
    /// and mapped again (from its file) when it is prepared again.
    code: Mutex<HashMap<[u8; 32], Compiled>>,
    /// Describe modules by the sha256 of their bytes; `None` for bytes that
    /// do not compile.
    described: Mutex<HashMap<[u8; 32], Option<Arc<Module>>>>,
    /// This binary and its compile child's arguments, when compiles run
    /// apart ([`RunnerConfig::compile_apart`]) and there is a cache dir.
    apart: Option<(PathBuf, [OsString; 3])>,
}

/// The argument that runs this binary as a compile child
/// ([`compile_child`]), followed by `<signal traps 0|1> <cache dir>`, the
/// code on its stdin.
pub const COMPILE_APART: &str = "--compile-view";

/// What the cache dir keeps ([`prune`]).
const CACHE_BYTES: u64 = 512 << 20;

/// A file in the cache dir that is not code, once it is older than this: a
/// write a killed process left, or what an older build kept there.
const STALE: Duration = Duration::from_secs(60 * 60);

/// Compiled code, as long as something holds it, and the `view_bytes` it
/// was last prepared for.
type Compiled = (Weak<Module>, usize);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The engine every compile and every load runs on: Cranelift at `Speed`,
/// fuel on, the shell's trap mode. A parent and its compile child build it
/// alike, so the file one writes is the file the other maps.
fn engine(signals_based_traps: bool) -> Engine {
    let mut wasm = Config::new();
    wasm.cranelift_opt_level(OptLevel::Speed);
    wasm.consume_fuel(true);
    wasm.signals_based_traps(signals_based_traps);
    Engine::new(&wasm).expect("wasmtime engine")
}

/// `engine`'s settings as one number: wasmtime's own account of what a
/// compiled module must match (its version, the target, every compile
/// setting).
fn settings(engine: &Engine) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    // fixed keys: the same number in a parent and its child
    let mut hasher = std::hash::DefaultHasher::new();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.finish()
}

/// Where `dir` keeps the code compiled out of the bytes hashed `hash` by an
/// engine with `settings`: named by both, so code another wasmtime or
/// other settings compiled is never looked for.
fn code_file(dir: &Path, hash: &[u8; 32], settings: u64) -> PathBuf {
    let hash = noded_client::hex::hex_encode(hash);
    dir.join(format!("{hash}-{settings:016x}.cwasm"))
}

/// The compiled code at `file`, mapped; none when there is no file, or
/// wasmtime refuses it. A map is touched, so the file's time is its last
/// use ([`prune`]).
fn mapped(engine: &Engine, file: &Path) -> Option<Module> {
    // SAFETY: wasmtime checks the file's header (its version, the target,
    // every compile setting) and refuses a mismatch, but trusts the code in
    // it: whoever writes the file runs code here. It is ours: only this
    // binary writes it ([`keep`]), under the cache dir this user's shell
    // gave, named by the code and the settings ([`code_file`]), whole and
    // renamed into place, never written in place again, so no mapping
    // changes under a module (a prune unlinks it, which leaves a mapping
    // whole).
    let module = unsafe { Module::deserialize_file(engine, file) }.ok()?;
    let _ = std::fs::File::options()
        .write(true)
        .open(file)
        .and_then(|file| file.set_modified(SystemTime::now()));
    Some(module)
}

/// `compiled` written to `file` whole or not at all: a sibling of its own,
/// synced, renamed into place. A reader never maps half a file, and a crash
/// never leaves one for the next launch to run.
fn keep(file: &Path, compiled: &[u8]) {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let partial = file.with_extension(format!("{}-{n}.partial", std::process::id()));
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::File::create(&partial))
        .and_then(|mut kept| {
            kept.write_all(compiled)?;
            kept.sync_all()
        })
        .and_then(|()| std::fs::rename(&partial, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
}

/// `code` compiled by `engine` and kept at `file` ([`keep`]).
fn compile_to(engine: &Engine, code: &[u8], file: &Path) -> wasmtime::Result<Vec<u8>> {
    let compiled = engine.precompile_module(code)?;
    keep(file, &compiled);
    Ok(compiled)
}

impl WasmtimeRunner {
    /// The [`engine`] in the shell's trap mode, compiled code kept under
    /// `config.cache_dir` when there is one.
    pub fn new(config: RunnerConfig) -> Self {
        let engine = engine(config.signals_based_traps);
        let traps = OsString::from(match config.signals_based_traps {
            true => "1",
            false => "0",
        });
        let apart = match (&config.cache_dir, config.compile_apart) {
            (Some(dir), Some(exe)) => Some((exe, [COMPILE_APART.into(), traps, dir.into()])),
            _ => None,
        };
        if let Some(dir) = config.cache_dir.clone() {
            let _ = std::thread::Builder::new()
                .name("view-cache-upkeep".into())
                .spawn(move || prune(&dir, CACHE_BYTES));
        }
        Self {
            settings: settings(&engine),
            engine,
            dir: config.cache_dir,
            code: Mutex::default(),
            described: Mutex::default(),
            apart,
        }
    }

    /// Compiles `code` (`hash`) into its file in a child of this binary,
    /// which says whether it found it there (`Disk`) or compiled it
    /// (`Cold`). A child that fails is logged, and the compile happens here
    /// as it did before there was a child; code that does not compile is
    /// not logged, the compile here fails with the reason.
    fn compile_apart(&self, hash: [u8; 32], code: &[u8]) -> Option<CodeSource> {
        let (exe, args) = self.apart.as_ref()?;
        let ran = Command::new(exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                // a child gone early fails the write; its exit says why
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(code);
                }
                child.wait_with_output()
            });
        let hash = noded_client::hex::hex_encode(&hash);
        match ran {
            Ok(output) if output.status.success() => match &output.stdout[..] {
                b"disk" => Some(CodeSource::Disk),
                _ => Some(CodeSource::Cold),
            },
            Ok(output) if output.status.code() == Some(1) => None,
            Ok(output) => {
                tracing::warn!(
                    reason = "compile_apart_failed",
                    %hash,
                    status = %output.status,
                    error = %String::from_utf8_lossy(&output.stderr).trim(),
                );
                None
            }
            Err(error) => {
                tracing::warn!(reason = "compile_apart_unavailable", %hash, %error);
                None
            }
        }
    }

    /// `code` (sha256 `hash`) as a module, and where it came from: its file
    /// mapped (`Disk`), else compiled into its file, by a child where there
    /// is one, and mapped (`Cold`). With no cache dir, or no file written
    /// (no room), it is compiled here and held in memory.
    fn compiled(&self, hash: [u8; 32], code: &[u8]) -> Result<(Module, CodeSource), Failure> {
        let failed = |error: wasmtime::Error| Failure::Runner(error.to_string());
        let Some(dir) = &self.dir else {
            return Ok((
                Module::new(&self.engine, code).map_err(failed)?,
                CodeSource::Cold,
            ));
        };
        let file = code_file(dir, &hash, self.settings);
        if let Some(module) = mapped(&self.engine, &file) {
            return Ok((module, CodeSource::Disk));
        }
        if let Some(source) = self.compile_apart(hash, code) {
            if let Some(module) = mapped(&self.engine, &file) {
                return Ok((module, source));
            }
            // the child compiled it and it is still not there (no room, or
            // the dir not writable): compiled here too
            tracing::warn!(
                reason = "compile_apart_missed",
                hash = %noded_client::hex::hex_encode(&hash),
            );
        }
        let compiled = compile_to(&self.engine, code, &file).map_err(failed)?;
        let module = match mapped(&self.engine, &file) {
            Some(module) => module,
            // SAFETY: what this engine compiled just now
            None => unsafe { Module::deserialize(&self.engine, &compiled) }.map_err(failed)?,
        };
        Ok((module, CodeSource::Cold))
    }

    /// The engine, for the tests that build a module by hand.
    #[cfg(any(test, feature = "test-support"))]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    fn instance(&self, hash: [u8; 32], land: &Landing) -> Result<WasmtimeInstance, Fault> {
        let held = lock(&self.code)
            .get(&hash)
            .and_then(|(module, view_bytes)| Some((module.upgrade()?, *view_bytes)));
        let Some((module, view_bytes)) = held else {
            return Err(Fault::Runner(
                "the view's code is not prepared, or nothing holds it any more".into(),
            ));
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
    fn prepare(&self, hash: [u8; 32], code: &[u8], view_bytes: usize) -> Result<Prepared, Failure> {
        if let Some((module, bytes)) = lock(&self.code).get_mut(&hash)
            && let Some(module) = module.upgrade()
        {
            *bytes = view_bytes;
            return Ok(Prepared {
                source: CodeSource::Memory,
                hold: module,
            });
        }
        // compiled outside the lock, so unrelated views prepare at once; a
        // competing compile of the same code adopts the one kept first
        view_meter::accepts(code).map_err(|refusal| Failure::Unsupported(refusal.to_string()))?;
        let (module, source) = self.compiled(hash, code)?;
        let mut kept = lock(&self.code);
        kept.retain(|_, (module, _)| module.strong_count() > 0);
        let module = match kept.get(&hash).and_then(|(module, _)| module.upgrade()) {
            Some(first) => first,
            None => Arc::new(module),
        };
        kept.insert(hash, (Arc::downgrade(&module), view_bytes));
        Ok(Prepared {
            source,
            hold: module,
        })
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
                // the rule a view is taken by, as on iOS (`accepts`), kept
                // as a view's code is; and the rule `describe::host::compile`
                // takes a module by, that it imports nothing
                let compiled = view_meter::accepts(module)
                    .ok()
                    .and_then(|()| self.compiled(hash, module).ok())
                    .map(|(compiled, _)| compiled)
                    .filter(|compiled| compiled.imports().len() == 0)
                    .map(Arc::new);
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
        if error.downcast_ref::<PastViewMemory>().is_some()
            || root.downcast_ref::<PastViewMemory>().is_some()
        {
            return Fault::Memory;
        }
        Fault::Trap(first_line(error))
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
        u64::MAX - self.store.get_fuel().unwrap_or(0)
    }

    fn memory_bytes(&self) -> usize {
        self.exports.memory.data_size(&self.store)
    }

    fn stop(self: Box<Self>) {}

    #[cfg(test)]
    fn memory_for_test(&self) -> &[u8] {
        self.exports.memory.data(&self.store)
    }

    #[cfg(test)]
    fn memory_mut_for_test(&mut self) -> &mut [u8] {
        self.exports.memory.data_mut(&mut self.store)
    }
}

fn arm(store: &mut Store<HostState>) {
    let _ = store.set_fuel(u64::MAX);
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

/// Counts only grows that succeed. wasmtime asks the limiter before it
/// checks the module's own maximum, so a grow past that maximum would fail
/// after it was counted, and every later grow would be judged against
/// memory or tables the view never had: such a grow is refused here (the
/// view's `grow` answers -1, as wasmtime's own refusal does) and counted
/// nowhere.
impl wasmtime::ResourceLimiter for ViewLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if maximum.is_some_and(|maximum| desired > maximum) {
            return Ok(false);
        }
        self.fits(desired, self.tables)?;
        self.memory = desired;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if maximum.is_some_and(|maximum| desired > maximum) {
            return Ok(false);
        }
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
/// `snapshot` and `restore` are optional (`ViewInstance`): a program's
/// notifier hands no state over.
struct Exports {
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    init: TypedFunc<(), ()>,
    tick: TypedFunc<(u32, u32), u64>,
    snapshot: Option<TypedFunc<(), u64>>,
    restore: Option<TypedFunc<(u32, u32), u64>>,
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
            snapshot: optional(store, instance, "snapshot")?,
            restore: optional(store, instance, "restore")?,
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
        let packed = exported(&self.snapshot, "snapshot")?.call(&mut *store, ())?;
        self.result(store, packed)
    }

    fn restore(
        &self,
        store: &mut Store<HostState>,
        state: &[u8],
    ) -> wasmtime::Result<Result<(), String>> {
        let restore = exported(&self.restore, "restore")?;
        let (ptr, len) = self.give(store, state)?;
        let packed = restore.call(&mut *store, (ptr, len))?;
        Ok(self.result(store, packed)?.map(|_| ()))
    }
}

/// The export `name` when the module has one, typed as `bind` reads it; a
/// module that has one of another type is refused as a missing required
/// one is.
fn optional<Params: wasmtime::WasmParams, Results: wasmtime::WasmResults>(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    name: &str,
) -> wasmtime::Result<Option<TypedFunc<Params, Results>>> {
    match instance.get_func(&mut *store, name) {
        Some(func) => Ok(Some(func.typed(&*store)?)),
        None => Ok(None),
    }
}

/// An optional export, or the trap a call into one the module lacks is.
fn exported<'a, T>(export: &'a Option<T>, name: &str) -> wasmtime::Result<&'a T> {
    export
        .as_ref()
        .ok_or_else(|| wasmtime::Error::msg(format!("the view exports no {name}")))
}

fn panic_message(store: &mut Store<HostState>) -> Option<String> {
    let text = store.data_mut().panic.take()?;
    (!text.is_empty()).then_some(text)
}

/// The compile child's body ([`COMPILE_APART`]): the code on stdin into
/// the cache dir, and `disk` or `cold` on stdout as [`compile_into`] found
/// it. Exit 1 with wasmtime's error when the code does not compile, 2 on a
/// bad call.
pub fn compile_child(args: &[OsString]) -> i32 {
    let (traps, dir) = match args {
        [traps, dir] if traps == "0" || traps == "1" => (traps == "1", dir),
        _ => {
            eprintln!("usage: {COMPILE_APART} <signal traps 0|1> <cache dir>, the code on stdin");
            return 2;
        }
    };
    let mut code = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut code) {
        eprintln!("{error}");
        return 2;
    }
    match compile_into(traps, dir.into(), &code) {
        Ok(CodeSource::Disk) => print!("disk"),
        Ok(_) => print!("cold"),
        Err((exit, error)) => {
            eprintln!("{error}");
            return exit;
        }
    }
    0
}

/// Compiles `code` into the file in `dir` a parent with these
/// `signals_based_traps` maps ([`code_file`]); whether it was there already.
fn compile_into(
    signals_based_traps: bool,
    dir: PathBuf,
    code: &[u8],
) -> Result<CodeSource, (i32, String)> {
    use sha2::Digest as _;
    let engine = engine(signals_based_traps);
    let file = code_file(&dir, &sha2::Sha256::digest(code).into(), settings(&engine));
    if mapped(&engine, &file).is_some() {
        return Ok(CodeSource::Disk);
    }
    compile_to(&engine, code, &file).map_err(|error| (1, first_line(&error)))?;
    Ok(CodeSource::Cold)
}

/// The cache dir's upkeep, once at start: what is not code there goes once
/// it is older than [`STALE`] (a write a killed process left, what an older
/// build kept), then the code least recently mapped goes until the rest
/// fits `limit`. A map touches its file ([`mapped`]), so a file's time is
/// its last use.
fn prune(dir: &Path, limit: u64) {
    let now = SystemTime::now();
    let age = |meta: &std::fs::Metadata| {
        meta.modified()
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .unwrap_or_default()
    };
    let mut code = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let (path, Ok(meta)) = (entry.path(), entry.metadata()) else {
            continue;
        };
        if meta.is_file() && path.extension().is_some_and(|ext| ext == "cwasm") {
            code.push((age(&meta), meta.len(), path));
        } else if age(&meta) > STALE {
            let _ = match meta.is_dir() {
                true => std::fs::remove_dir_all(&path),
                false => std::fs::remove_file(&path),
            };
        }
    }
    code.sort_by_key(|(unused, ..)| *unused);
    let mut kept = 0;
    for (_, bytes, path) in code {
        kept += bytes;
        if kept > limit {
            let _ = std::fs::remove_file(path);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 65_536;

    /// The child's compile is the one this process maps: it builds its
    /// engine from the same settings, so the file it writes is the one
    /// looked for here. Settings drifting apart would compile every view
    /// twice, the second time here, and keep that compile's memory here.
    #[test]
    fn the_childs_compile_is_a_disk_hit_here() {
        use sha2::Digest as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let code = wat::parse_str(
            r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 64))"#,
        )
        .expect("parses");
        let child = |code: &[u8]| compile_into(true, dir.path().into(), code);
        assert!(matches!(child(&code), Ok(CodeSource::Cold)));
        assert!(matches!(child(&code), Ok(CodeSource::Disk)));
        assert!(matches!(child(b"not wasm"), Err((1, _))));
        let runner = WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: Some(dir.path().into()),
            compile_apart: None,
        });
        let hash = sha2::Sha256::digest(&code).into();
        let prepared = runner.prepare(hash, &code, PAGE).expect("prepares");
        assert!(matches!(prepared.source, CodeSource::Disk));
    }

    /// Compiled code is kept as a file of its own, named by the code and
    /// the engine's settings, and mapped as it is: the next launch reads it
    /// off the disk. A file at that name another engine wrote (a stale
    /// build's, other settings) is refused, never run: the code compiles
    /// again over it.
    #[test]
    fn compiled_code_is_mapped_from_its_own_file_and_a_foreign_one_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let code = wat::parse_str(
            r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0))"#,
        )
        .expect("parses");
        let hash = [5; 32];
        let runner = |traps| {
            WasmtimeRunner::new(RunnerConfig {
                signals_based_traps: traps,
                cache_dir: Some(dir.path().into()),
                compile_apart: None,
            })
        };
        let (here, other) = (runner(true), runner(false));
        let file = code_file(dir.path(), &hash, here.settings);
        assert_eq!(
            here.prepare(hash, &code, PAGE).expect("compiles").source,
            CodeSource::Cold
        );
        assert!(file.exists(), "kept as its own file");
        // a later launch: nothing in memory, the file mapped
        assert_eq!(
            runner(true)
                .prepare(hash, &code, PAGE)
                .expect("maps")
                .source,
            CodeSource::Disk
        );
        // another engine's code under this engine's name
        assert_ne!(here.settings, other.settings);
        let _ = other.prepare(hash, &code, PAGE).expect("compiles");
        std::fs::copy(code_file(dir.path(), &hash, other.settings), &file).expect("copies");
        assert!(mapped(&here.engine, &file).is_none(), "refused, not run");
        let again = runner(true).prepare(hash, &code, PAGE).expect("compiles");
        assert_eq!(again.source, CodeSource::Cold);
        assert!(
            mapped(&here.engine, &file).is_some(),
            "compiled again over it"
        );
    }

    /// The upkeep: code mapped least recently goes first until the rest
    /// fits; a write left past [`STALE`] and what an older build kept go,
    /// a younger write (live) stays.
    #[test]
    fn upkeep_drops_the_least_used_code_and_stale_leftovers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let hours_ago = |hours: u64| SystemTime::now() - Duration::from_secs(hours * 3600);
        let file = |path: &Path, bytes: usize, at: SystemTime| {
            std::fs::write(path, vec![0; bytes]).expect("write");
            std::fs::File::options()
                .write(true)
                .open(path)
                .and_then(|file| file.set_modified(at))
                .expect("touch");
        };
        let at = |name: &str| dir.path().join(name);
        file(&at("old.cwasm"), 10, hours_ago(9));
        file(&at("new.cwasm"), 10, hours_ago(1));
        file(&at("new.1-0.partial"), 1, hours_ago(9));
        file(&at("now.1-0.partial"), 1, SystemTime::now());
        // wasmtime's own cache, as an older build kept it
        let modules = at("modules").join("wasmtime-46.0.3");
        std::fs::create_dir_all(&modules).expect("dirs");
        file(&modules.join("code"), 1, hours_ago(9));
        std::fs::File::open(at("modules"))
            .and_then(|dir| dir.set_modified(hours_ago(9)))
            .expect("touch");
        prune(dir.path(), 15);
        assert!(!at("old.cwasm").exists());
        assert!(at("new.cwasm").exists());
        assert!(!at("new.1-0.partial").exists());
        assert!(at("now.1-0.partial").exists());
        assert!(!at("modules").exists());
    }

    /// A grow past the module's own maximum is refused and counts nothing:
    /// a table grow that fits beside the memory the view really has, then a
    /// memory grow within its maximum and the budget, both succeed after
    /// it. Before, the refused grow's size stayed counted, and the table
    /// grow trapped past the budget.
    #[test]
    fn a_grow_refused_at_the_modules_own_maximum_counts_nothing() {
        let runner = WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: None,
            compile_apart: None,
        });
        // budget 4 pages; memory 1 page, its maximum 2; two pages of table
        // elements at 8 bytes each
        let elements = 2 * PAGE / std::mem::size_of::<usize>();
        let code = format!(
            r#"(module
            (memory (export "memory") 1 2)
            (table 0 {elements} funcref)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init")
                ;; 3 pages: within the budget, past the module's maximum
                (if (i32.ne (memory.grow (i32.const 2)) (i32.const -1)) (then unreachable))
                ;; 1 page of memory + 2 of table: fits the budget
                (if (i32.eq (table.grow (ref.null func) (i32.const {elements})) (i32.const -1))
                    (then unreachable))
                ;; 2 pages of memory + 2 of table: fits, at the maximum
                (if (i32.ne (memory.grow (i32.const 1)) (i32.const 1)) (then unreachable)))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0)
            (func (export "snapshot") (result i64) i64.const 0)
            (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#
        );
        let hash = [7; 32];
        let _held = runner
            .prepare(hash, &wat::parse_str(&code).expect("parses"), 4 * PAGE)
            .expect("compiles");
        let (land, landing) = Landing::channel();
        runner.instantiate(hash, SeatId("grow-past-maximum", 0), land);
        let Ok(Landed::Seated(mut instance)) = landing.try_recv() else {
            panic!("not seated");
        };
        instance.begin_init();
        let inited = landing.try_recv().expect("lands at once");
        assert!(
            matches!(inited, Landed::Inited),
            "a grow after a refused one: {inited:?}"
        );
    }

    /// Compiled code is found while anything holds it (the hold `prepare`
    /// answers, a running instance) and freed once nothing does: seating it
    /// then lands the runner's fault, and preparing it again compiles it.
    /// One rule on both sides (wasm-mobile SPEC §6.2): the runner prepares
    /// what `view_meter::accepts` takes, the rule the iOS runner applies,
    /// and refuses the rest as `Unsupported` in the meter's words. Each
    /// refused shape is one wasmtime alone would compile.
    #[test]
    fn the_runner_takes_what_the_meter_accepts_and_nothing_else() {
        let runner = WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: None,
            compile_apart: None,
        });
        let view = |more: &str| {
            format!(
                r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32) i32.const 64)
                {more})"#
            )
        };
        let cases = [
            ("a view", view(""), true),
            ("a start function", view("(func $go) (start $go)"), false),
            (
                "an imported memory",
                r#"(module (import "host" "memory" (memory 1)))"#.to_owned(),
                false,
            ),
            (
                "an export named as the meter's fuel",
                view(r#"(global (export "gas_left") (mut i64) (i64.const 0))"#),
                false,
            ),
            (
                "SIMD",
                view("(func (result v128) v128.const i64x2 0 0)"),
                false,
            ),
            (
                "a tail call",
                view("(func $again return_call $again)"),
                false,
            ),
        ];
        for (nth, (what, text, takes)) in cases.into_iter().enumerate() {
            let code = wat::parse_str(&text).expect("parses");
            let meter = view_meter::accepts(&code);
            assert_eq!(meter.is_ok(), takes, "the meter on {what}: {meter:?}");
            assert_eq!(
                runner.prepare([nth as u8; 32], &code, 4 * PAGE).err(),
                meter
                    .err()
                    .map(|refusal| Failure::Unsupported(refusal.to_string())),
                "the runner on {what}"
            );
        }
    }

    /// A describe module is taken by the same rule (`view_meter::accepts`),
    /// so a program's ops read the same on every platform: the tail call is
    /// one wasmtime alone would compile.
    #[test]
    fn a_describe_module_is_taken_by_the_meters_rule() {
        let runner = WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: None,
            compile_apart: None,
        });
        let module = |more: &str| {
            wat::parse_str(format!(
                r#"(module
                (memory (export "memory") 1)
                (func (export "alloc") (param i32) (result i32) i32.const 64)
                (func (export "describe") (param i32 i32) (result i64) i64.const 0)
                {more})"#
            ))
            .expect("parses")
        };
        for (hash, more, compiled) in [
            ([1; 32], "", true),
            ([2; 32], "(func $again return_call $again)", false),
        ] {
            // an empty answer decodes to no description either way
            assert!(runner.describe(hash, &module(more), b"op").is_none());
            assert_eq!(lock(&runner.described)[&hash].is_some(), compiled, "{more}");
        }
    }

    #[test]
    fn code_nothing_holds_is_freed() {
        let runner = WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: None,
            compile_apart: None,
        });
        let code = r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0)
            (func (export "snapshot") (result i64) i64.const 0)
            (func (export "restore") (param i32 i32) (result i64) i64.const 0))"#;
        let hash = [8; 32];
        let code = wat::parse_str(code).expect("parses");
        let prepare = || runner.prepare(hash, &code, 4 * PAGE).expect("compiles");
        let seat = || {
            let (land, landing) = Landing::channel();
            runner.instantiate(hash, SeatId("code-freed", 0), land);
            landing.try_recv().expect("lands at once")
        };
        let held = prepare();
        assert_eq!(held.source, CodeSource::Cold);
        assert_eq!(
            prepare().source,
            CodeSource::Memory,
            "held: not compiled again"
        );
        let Landed::Seated(instance) = seat() else {
            panic!("not seated");
        };
        drop(held);
        assert!(
            matches!(seat(), Landed::Seated(_)),
            "a running instance holds its code"
        );
        drop(instance);
        assert!(
            matches!(seat(), Landed::Fault(Fault::Runner(_))),
            "nothing holds the code: it is freed"
        );
        assert_eq!(prepare().source, CodeSource::Cold, "compiled again");
    }

    /// A module with no state to hand over (a program's notifier) is
    /// seated and ticked; asked for its state, it traps.
    #[test]
    fn a_module_without_snapshot_and_restore_seats_and_traps_when_asked() {
        let runner = WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: None,
            compile_apart: None,
        });
        let code = r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 64)
            (func (export "init"))
            (func (export "tick") (param i32 i32) (result i64) i64.const 0))"#;
        let hash = [9; 32];
        let code = wat::parse_str(code).expect("parses");
        let _held = runner.prepare(hash, &code, 4 * PAGE).expect("compiles");
        let (land, landing) = Landing::channel();
        runner.instantiate(hash, SeatId("headless", 0), land);
        let Ok(Landed::Seated(mut instance)) = landing.try_recv() else {
            panic!("not seated");
        };
        instance.begin_tick(Vec::new());
        assert!(matches!(landing.try_recv(), Ok(Landed::Frame(_))));
        instance.begin_snapshot();
        assert!(
            matches!(landing.try_recv(), Ok(Landed::Fault(Fault::Trap(words))) if words == "the view exports no snapshot")
        );
    }
}
