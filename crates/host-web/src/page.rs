//! The runner and its seats on the page's one thread: metered code by hash while it is held, its compile a promise of the page's, and each seat's exports in a map by id, so an instance stays `Send`.
//! A call arms `gas_left`, runs the export and reads the meter after it, as the JavaScriptCore runner's page does, with no glue in between.

use js_sys::{
    Array, ArrayBuffer, BigInt, Function, Object, Promise, Reflect, Uint8Array, WebAssembly,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use view_runner::{
    Code, CodeSource, Failure, Fault, Landed, Landing, Prepared, SeatId, ViewInstance, ViewRunner,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

const VIEW_FUEL: i64 = i64::MAX;
const NOT_HELD: &str = "the view's code is not prepared, or nothing holds it any more";

static NEXT: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static COMPILES: RefCell<HashMap<u64, Promise>> = RefCell::default();
    static SEATS: RefCell<HashMap<u64, Seat>> = RefCell::default();
    static DESCRIBED: RefCell<HashMap<[u8; 32], Option<Promise>>> = RefCell::default();
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct Prepare {
    id: u64,
    view_bytes: u64,
}

impl Drop for Prepare {
    fn drop(&mut self) {
        COMPILES.with_borrow_mut(|compiles| compiles.remove(&self.id));
    }
}

#[derive(Default)]
pub struct WebRunner {
    codes: Mutex<HashMap<[u8; 32], Vec<Weak<Prepare>>>>,
}

impl WebRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn held(&self, hash: &[u8; 32], view_bytes: Option<u64>) -> Option<Arc<Prepare>> {
        lock(&self.codes)
            .get(hash)?
            .iter()
            .rev()
            .filter_map(Weak::upgrade)
            .find(|code| view_bytes.is_none_or(|bytes| code.view_bytes == bytes))
    }
}

fn compiling(id: u64) -> Option<Promise> {
    COMPILES.with_borrow(|compiles| compiles.get(&id).cloned())
}

fn compile(bytes: &[u8]) -> Promise {
    WebAssembly::compile(&Uint8Array::from(bytes))
}

fn metered(code: &[u8], view_bytes: u64) -> Result<Vec<u8>, Failure> {
    view_meter::meter(code, view_bytes).map_err(|refusal| match refusal {
        view_meter::Refusal::Unsupported(_) => Failure::Unsupported(refusal.to_string()),
        view_meter::Refusal::Memory { .. } => Failure::Runner(refusal.to_string()),
    })
}

impl ViewRunner for WebRunner {
    fn prepare(
        &self,
        hash: [u8; 32],
        code: Code<'_>,
        view_bytes: usize,
    ) -> Result<Prepared, Failure> {
        let view_bytes = u64::try_from(view_bytes).unwrap_or(u64::MAX);
        let held = self
            .held(&hash, Some(view_bytes))
            .filter(|held| compiling(held.id).is_some());
        if let Some(held) = held {
            return Ok(Prepared {
                source: CodeSource::Memory,
                hold: held,
            });
        }
        let held = self.held(&hash, Some(view_bytes));
        let promise = compile(&metered(code.bytes, view_bytes)?);
        let prepared = match held {
            Some(held) => held,
            None => {
                let fresh = Arc::new(Prepare {
                    id: NEXT.fetch_add(1, Ordering::Relaxed),
                    view_bytes,
                });
                let mut codes = lock(&self.codes);
                codes.retain(|_, codes| {
                    codes.retain(|code| code.strong_count() > 0);
                    !codes.is_empty()
                });
                codes.entry(hash).or_default().push(Arc::downgrade(&fresh));
                fresh
            }
        };
        COMPILES.with_borrow_mut(|compiles| compiles.insert(prepared.id, promise));
        Ok(Prepared {
            source: CodeSource::Cold,
            hold: prepared,
        })
    }

    fn instantiate(&self, hash: [u8; 32], _: SeatId, land: Landing) {
        let Some(code) = self.held(&hash, None) else {
            return land.send(Landed::Fault(Fault::Runner(NOT_HELD.into())));
        };
        drop(rt::task::spawn(async move {
            let landed = match seated(&code).await {
                Ok(seat) => {
                    let id = NEXT.fetch_add(1, Ordering::Relaxed);
                    let memory = seat.memory();
                    SEATS.with_borrow_mut(|seats| seats.insert(id, seat));
                    Landed::Seated(Box::new(WebInstance {
                        id,
                        hash,
                        land: land.clone(),
                        fuel: 0,
                        memory,
                        _code: code,
                    }))
                }
                Err(fault) => Landed::Fault(fault),
            };
            land.send(landed);
        }));
    }

    fn describe(
        &self,
        hash: [u8; 32],
        module: Code<'_>,
        op: Vec<u8>,
    ) -> rt::Fut<'static, Option<describe::Description>> {
        let promise = DESCRIBED.with_borrow_mut(|described| {
            described
                .entry(hash)
                .or_insert_with(|| {
                    view_meter::meter(module.bytes, crate::DESCRIBE_MEMORY)
                        .ok()
                        .map(|metered| compile(&metered))
                })
                .clone()
        });
        Box::pin(async move { described(promise?, &op).await })
    }
}

async fn described(promise: Promise, op: &[u8]) -> Option<describe::Description> {
    let module: WebAssembly::Module = JsFuture::from(promise).await.ok()?.unchecked_into();
    if WebAssembly::Module::imports(&module).length() != 0 {
        return None;
    }
    let made = WebAssembly::instantiate_module(&module, &Object::new());
    let instance: WebAssembly::Instance = JsFuture::from(made).await.ok()?.unchecked_into();
    let exports = instance.exports();
    let memory: WebAssembly::Memory = export(&exports, "memory")?;
    let alloc: Function = export(&exports, "alloc")?;
    let describe: Function = export(&exports, "describe")?;
    let gas: WebAssembly::Global = export(&exports, view_meter::GAS)?;
    gas.set_value(&BigInt::from(crate::DESCRIBE_FUEL).into());
    let (ptr, len) = give(&alloc, &memory, op).ok()?;
    let packed = describe
        .call2(&JsValue::NULL, &ptr.into(), &len.into())
        .ok()?;
    let bytes = answer(&memory, packed).ok()??;
    describe::Description::decode(&bytes)
}

fn export<T: JsCast>(exports: &Object, name: &str) -> Option<T> {
    Reflect::get(exports, &JsValue::from_str(name))
        .ok()?
        .dyn_into()
        .ok()
}

fn said(error: &JsValue) -> String {
    match error.dyn_ref::<js_sys::Error>() {
        Some(error) => String::from(error.message()),
        None => error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}

fn buffer(memory: &WebAssembly::Memory) -> ArrayBuffer {
    memory.buffer().unchecked_into()
}

fn give(
    alloc: &Function,
    memory: &WebAssembly::Memory,
    bytes: &[u8],
) -> Result<(u32, u32), JsValue> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| js_sys::Error::new("the argument is past wasm's 4 GiB"))?;
    let ptr = alloc.call1(&JsValue::NULL, &len.into())?;
    let ptr = ptr.as_f64().map_or(0, |ptr| ptr as i32 as u32);
    let room = buffer(memory);
    if u64::from(ptr) + u64::from(len) > u64::from(room.byte_length()) {
        return Err(js_sys::Error::new("the view's buffer is outside its memory").into());
    }
    Uint8Array::new_with_byte_offset_and_length(&room, ptr, len).copy_from(bytes);
    Ok((ptr, len))
}

fn answer(memory: &WebAssembly::Memory, packed: JsValue) -> Result<Option<Vec<u8>>, JsValue> {
    let packed = i64::try_from(packed)?.cast_unsigned();
    let (ptr, len) = view_wire::abi::unpack(packed);
    let room = buffer(memory);
    if u64::from(ptr) + u64::from(len) > u64::from(room.byte_length()) {
        return Ok(None);
    }
    Ok(Some(
        Uint8Array::new_with_byte_offset_and_length(&room, ptr, len).to_vec(),
    ))
}

#[derive(Clone)]
struct Exports {
    memory: WebAssembly::Memory,
    alloc: Function,
    init: Function,
    tick: Function,
    snapshot: Option<Function>,
    restore: Option<Function>,
    gas: WebAssembly::Global,
}

struct Seat {
    exports: Exports,
    panic: Rc<RefCell<Option<String>>>,
    _hooks: Vec<Closure<dyn FnMut(u32, u32)>>,
    _traps: Vec<Closure<dyn Fn() -> Result<(), JsValue>>>,
}

impl Seat {
    fn memory(&self) -> usize {
        buffer(&self.exports.memory).byte_length() as usize
    }
}

async fn seated(code: &Prepare) -> Result<Seat, Fault> {
    let Some(promise) = compiling(code.id) else {
        return Err(Fault::Runner(NOT_HELD.into()));
    };
    let module: WebAssembly::Module = match JsFuture::from(promise).await {
        Ok(module) => module.unchecked_into(),
        Err(error) => {
            COMPILES.with_borrow_mut(|compiles| compiles.remove(&code.id));
            return Err(Fault::Runner(format!(
                "the view's code did not load: {}",
                said(&error)
            )));
        }
    };
    let panic: Rc<RefCell<Option<String>>> = Rc::default();
    let memory: Rc<RefCell<Option<WebAssembly::Memory>>> = Rc::default();
    let imports = Object::new();
    let (mut hooks, mut traps) = (Vec::new(), Vec::new());
    let wanted: Array = WebAssembly::Module::imports(&module);
    let runner = |error: JsValue| Fault::Runner(said(&error));
    for wanted in wanted.iter() {
        let field = |name: &str| {
            Reflect::get(&wanted, &JsValue::from_str(name))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default()
        };
        if field("kind") != "function" {
            continue;
        }
        let (from, name) = (field("module"), field("name"));
        let place = match Reflect::get(&imports, &JsValue::from_str(&from)).map_err(runner)? {
            place if place.is_object() => place,
            _ => {
                let place: JsValue = Object::new().into();
                Reflect::set(&imports, &JsValue::from_str(&from), &place).map_err(runner)?;
                place
            }
        };
        let panic_hook =
            (from.as_str(), name.as_str()) == (view_wire::abi::IMPORT_MODULE, "panicked");
        let function = if panic_hook {
            let (panic, memory) = (panic.clone(), memory.clone());
            let hook = Closure::<dyn FnMut(u32, u32)>::new(move |ptr: u32, len: u32| {
                let words = memory.borrow().as_ref().and_then(|memory| {
                    let room = buffer(memory);
                    let end = u64::from(ptr) + u64::from(len);
                    (end <= u64::from(room.byte_length())).then(|| {
                        let bytes = Uint8Array::new_with_byte_offset_and_length(&room, ptr, len);
                        String::from_utf8_lossy(&bytes.to_vec()).into_owned()
                    })
                });
                let words = words.unwrap_or_default();
                *panic.borrow_mut() = Some(words.lines().next().unwrap_or_default().to_owned());
            });
            let function = hook.as_ref().clone();
            hooks.push(hook);
            function
        } else {
            let words = format!("unknown import: {from}::{name} has not been defined");
            let trap = Closure::<dyn Fn() -> Result<(), JsValue>>::new(move || {
                Err(WebAssembly::RuntimeError::new(&words).into())
            });
            let function = trap.as_ref().clone();
            traps.push(trap);
            function
        };
        Reflect::set(&place, &JsValue::from_str(&name), &function).map_err(runner)?;
    }
    let made = WebAssembly::instantiate_module(&module, &imports);
    let instance: WebAssembly::Instance = match JsFuture::from(made).await {
        Ok(instance) => instance.unchecked_into(),
        Err(error) => return Err(Fault::Trap(said(&error))),
    };
    let exports = instance.exports();
    let needed = |name: &str| {
        export::<Function>(&exports, name)
            .ok_or_else(|| Fault::Trap(format!("the view exports no {name}")))
    };
    let bound = Exports {
        memory: export(&exports, "memory")
            .ok_or_else(|| Fault::Trap("the view exports no memory".into()))?,
        alloc: needed("alloc")?,
        init: needed("init")?,
        tick: needed("tick")?,
        snapshot: export(&exports, "snapshot"),
        restore: export(&exports, "restore"),
        gas: export(&exports, view_meter::GAS)
            .ok_or_else(|| Fault::Trap("the view is not metered".into()))?,
    };
    *memory.borrow_mut() = Some(bound.memory.clone());
    Ok(Seat {
        exports: bound,
        panic,
        _hooks: hooks,
        _traps: traps,
    })
}

struct Ran {
    outcome: Result<Vec<u8>, Fault>,
    fuel: u64,
    memory: usize,
}

fn enter(id: u64, run: impl FnOnce(&Exports) -> Result<Option<Vec<u8>>, JsValue>) -> Ran {
    let seat = SEATS.with_borrow(|seats| {
        seats
            .get(&id)
            .map(|seat| (seat.exports.clone(), seat.panic.clone()))
    });
    let Some((exports, panic)) = seat else {
        return Ran {
            outcome: Err(Fault::Runner(format!("no view is seated as {id}"))),
            fuel: 0,
            memory: 0,
        };
    };
    exports.gas.set_value(&BigInt::from(VIEW_FUEL).into());
    let ran = run(&exports);
    let gas = i64::try_from(exports.gas.value()).unwrap_or(VIEW_FUEL);
    let memory = buffer(&exports.memory).byte_length() as usize;
    let outcome = match ran {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => Err(Fault::Trap("the view answered outside its memory".into())),
        Err(error) => match panic.borrow_mut().take() {
            Some(words) => Err(Fault::Panicked(words)),
            None => match view_meter::stop(gas) {
                view_meter::Stop::Memory => Err(Fault::Memory),
                view_meter::Stop::OutOfFuel | view_meter::Stop::Trap => {
                    Err(Fault::Trap(said(&error)))
                }
            },
        },
    };
    let fuel = match outcome {
        Ok(_) => (VIEW_FUEL - gas.max(0)).unsigned_abs(),
        Err(_) => 0,
    };
    Ran {
        outcome,
        fuel,
        memory,
    }
}

fn result(outcome: Result<Vec<u8>, Fault>) -> Result<Result<Vec<u8>, String>, Fault> {
    let bytes = outcome?;
    view_wire::abi::decode_result(&bytes)
        .ok_or_else(|| Fault::Trap("the view's answer is not a result".into()))
}

fn optional<'a>(export: &'a Option<Function>, name: &str) -> Result<&'a Function, JsValue> {
    export
        .as_ref()
        .ok_or_else(|| js_sys::Error::new(&format!("the view exports no {name}")).into())
}

struct WebInstance {
    id: u64,
    hash: [u8; 32],
    land: Landing,
    fuel: u64,
    memory: usize,
    _code: Arc<Prepare>,
}

impl WebInstance {
    fn call(
        &mut self,
        run: impl FnOnce(&Exports) -> Result<Option<Vec<u8>>, JsValue>,
        landed: impl FnOnce(Result<Vec<u8>, Fault>) -> Landed,
    ) {
        let ran = enter(self.id, run);
        self.fuel = ran.fuel;
        self.memory = ran.memory;
        self.land.send(landed(ran.outcome));
    }
}

fn fault_or(outcome: Result<Vec<u8>, Fault>, landed: impl FnOnce(Vec<u8>) -> Landed) -> Landed {
    match outcome {
        Ok(bytes) => landed(bytes),
        Err(fault) => Landed::Fault(fault),
    }
}

impl ViewInstance for WebInstance {
    fn code_hash(&self) -> [u8; 32] {
        self.hash
    }

    fn begin_init(&mut self) {
        self.call(
            |e| e.init.call0(&JsValue::NULL).map(|_| Some(Vec::new())),
            |outcome| fault_or(outcome, |_| Landed::Inited),
        );
    }

    fn begin_tick(&mut self, events: Vec<u8>) {
        self.call(
            |e| {
                let (ptr, len) = give(&e.alloc, &e.memory, &events)?;
                let packed = e.tick.call2(&JsValue::NULL, &ptr.into(), &len.into())?;
                answer(&e.memory, packed)
            },
            |outcome| fault_or(outcome, Landed::Frame),
        );
    }

    fn begin_snapshot(&mut self) {
        self.call(
            |e| {
                let packed = optional(&e.snapshot, "snapshot")?.call0(&JsValue::NULL)?;
                answer(&e.memory, packed)
            },
            |outcome| match result(outcome) {
                Ok(Ok(state)) => Landed::Snapshot(state),
                Ok(Err(refusal)) => Landed::SnapshotRefused(refusal),
                Err(fault) => Landed::Fault(fault),
            },
        );
    }

    fn begin_restore(&mut self, snapshot: Vec<u8>) {
        self.call(
            |e| {
                let restore = optional(&e.restore, "restore")?;
                let (ptr, len) = give(&e.alloc, &e.memory, &snapshot)?;
                let packed = restore.call2(&JsValue::NULL, &ptr.into(), &len.into())?;
                answer(&e.memory, packed)
            },
            |outcome| match result(outcome) {
                Ok(Ok(_)) => Landed::Restored,
                Ok(Err(refusal)) => Landed::RestoreRefused(refusal),
                Err(fault) => Landed::Fault(fault),
            },
        );
    }

    fn fuel_used(&self) -> u64 {
        self.fuel
    }

    fn memory_bytes(&self) -> usize {
        self.memory
    }

    fn stop(self: Box<Self>) {}
}

impl Drop for WebInstance {
    fn drop(&mut self) {
        SEATS.with_borrow_mut(|seats| seats.remove(&self.id));
    }
}

#[cfg(test)]
mod tests;
