//! The meter pass: a view's wasm rewritten so it carries its own bounds.
//!
//! On the desktop and Android, wasmtime bounds a view at run time: fuel
//! (`FUEL_PER_TICK` per call) and the `memory.view_bytes` budget
//! (`ViewLimits`: linear memory plus 8 bytes per table element). On iOS the
//! views run in JavaScriptCore, which has neither, and a page stuck in a wasm
//! loop cannot be stopped from outside (wasm-mobile S0-webkit §7). So
//! [`meter`] compiles both bounds into the module, with the same numbers and
//! the same answers:
//!
//! - **Features.** The module is validated against [`FEATURES`] first, and
//!   anything outside it is refused as [`Refusal::Unsupported`]: untrusted
//!   network bytes never reach the rewrite unchecked. The wasmtime runner
//!   sets the same set on its `Config`, so every runner accepts and refuses
//!   the same modules.
//! - **Fuel.** The count wasmtime 46 keeps (`wasmtime-environ`
//!   `default_operator_cost`, `wasmtime-cranelift` `fuel_before_op` /
//!   `pre_translate_bulk_op`): 1 per operator except `nop`, `drop`, `block`,
//!   `loop`, `unreachable`, `return`, `else`, `end`; 1 more per function
//!   invocation; the length on `memory.copy`, `memory.fill`, `memory.init`,
//!   `table.copy`, `table.fill`, `table.init` and the delta on `table.grow`.
//!   Pending cost is summed per straight-line run and charged at the points
//!   wasmtime charges it; the budget is checked where wasmtime checks it
//!   (function entry, every loop header, after a size-charged operator whose
//!   length is not a constant of at most 128), and a check traps when the
//!   count is `<= 0`. The count never leaves a register between the
//!   exports' entry and exit: every function of the module takes it as one
//!   more parameter and gives it back as one more result (each call passes
//!   it and takes it back; imports keep their type, and an import used as a
//!   value is reached through an adapter of the widened type). The host's
//!   side is one exported mutable `i64` global, [`GAS`]: each exported
//!   function becomes a wrapper with its original type that passes [`GAS`]
//!   in and stores the result back, and a fuel or memory trap writes it.
//!   The host sets [`GAS`] to its budget before a call and reads it after:
//!   used = budget - left, equal to wasmtime's fuel count per call, and fuel
//!   runs out at the same check. (Measured on the chat view's recorded
//!   session, median per rendered tick, unmetered = 1, JavaScriptCore shell /
//!   iOS simulator: the count in an exported global at every charge
//!   3.30x / 2.69x; in a local, stored to that global around calls
//!   1.33x / 1.27x; to an internal global 1.19x / 1.17x; passed as a
//!   parameter 1.10x / 1.08x.)
//! - **Memory.** Every `memory.grow` and `table.grow` first checks the whole
//!   budget the way `ViewLimits` does (the grown size, plus the memory or
//!   tables it does not grow, against `view_bytes`); past it the call traps
//!   with [`GAS`] set to [`GROW_REFUSED`] (a value fuel never reaches, so the
//!   one export carries both answers; no second export). A grow within the
//!   budget that wasmtime answers with `-1` (past the module's own maximum,
//!   no allocation) still answers `-1`: the budget is asked before the grow,
//!   in wasmtime's order, rather than every `-1` turned into a trap. The memory
//!   and table maxima are lowered to the budget too, and a module whose
//!   initial memory and tables alone exceed it is refused
//!   ([`Refusal::Memory`]).
//! - **After a trap**, [`stop`] reads [`GAS`]: out of fuel, past the memory
//!   budget, or the view's own trap.
//!
//! The derived module carries one export more than the five the view ABI
//! names ([`GAS`]); the author ABI is unchanged and no view imports or
//! exports anything new. The output is deterministic: the same input and
//! budget give the same bytes.
//!
//! Map: this file is the whole pass; `main.rs` is the command
//! (`view-meter <in.wasm> <out.wasm> <view_bytes>`); `tests/` is the runner
//! equivalence gate against wasmtime (SPEC §13.3 (a)).
use std::collections::HashMap;

use walrus::ir::{
    BinaryOp, Binop, Block, Br, BrIf, BrTable, Call, CallIndirect, Const, IfElse, Instr,
    InstrLocId, InstrSeqId, InstrSeqType, LocalGet, LocalSet, LocalTee, Loop, MemoryGrow,
    MemorySize, RefFunc, ReturnCall, ReturnCallIndirect, TableGrow, TableSize, UnaryOp, Unop,
    Unreachable, Value,
};
use walrus::{
    ConstExpr, ElementItems, FunctionBuilder, FunctionId, FunctionKind, GlobalId, GlobalKind,
    LocalId, MemoryId, ModuleConfig, ModuleTypes, TableId, TypeId, ValType,
};
use wasmparser::{Validator, WasmFeatures};

/// The one wasm feature set every view runner accepts: what Rust's
/// `wasm32-unknown-unknown` emits (MVP, mutable globals, sign extension,
/// non-trapping float-to-int, multi-value, bulk memory, reference types).
/// No tail calls, threads, exceptions, GC, multi-memory or memory64; and no
/// SIMD, which Rust emits only when asked and no view asks for (the pass
/// would charge it as wasmtime does, 1 per operator: adding it is this one
/// line).
/// A wasmtime runner applies it with
/// `config.wasm_features(WasmFeatures::all().difference(FEATURES), false)`.
pub const FEATURES: WasmFeatures = WasmFeatures::WASM1
    .union(WasmFeatures::SIGN_EXTENSION)
    .union(WasmFeatures::SATURATING_FLOAT_TO_INT)
    .union(WasmFeatures::MULTI_VALUE)
    .union(WasmFeatures::BULK_MEMORY)
    .union(WasmFeatures::REFERENCE_TYPES);

/// The export the pass adds: the fuel left, a mutable `i64` global the host
/// sets before every call and reads after it.
pub const GAS: &str = "gas_left";

/// [`GAS`] after a trap that a `memory.grow` or `table.grow` past the budget
/// raised. Fuel never reaches it: a check traps within one operator's charge
/// of zero, and no charge exceeds `u32::MAX`.
pub const GROW_REFUSED: i64 = i64::MIN;

/// A bulk operator with a constant length up to this is folded into the
/// pending cost without a check (wasmtime `SMALL_BULK_OP_COST`).
const SMALL_BULK: i64 = 128;
/// Bytes per wasm page.
const PAGE: u64 = 65_536;
/// A 32-bit memory's own limit, in pages (4 GiB).
const MEMORY_PAGES: u64 = 65_536;
/// What one table element costs the budget: a pointer, as `ViewLimits`
/// charges it (`size_of::<usize>()` on the 64-bit hosts views run on).
const ELEMENT_BYTES: u64 = 8;

/// Why [`meter`] gave no module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The bytes are not wasm the runners accept: invalid, or using a
    /// feature outside [`FEATURES`], or a shape the pass does not take (an
    /// imported memory or table, an export already named [`GAS`], a start
    /// function, which would run before the host could set [`GAS`]). The text
    /// names what was refused.
    Unsupported(String),
    /// The module's initial memory and tables alone exceed `view_bytes`.
    Memory {
        /// What the initial memory and tables take, in bytes.
        needs: u64,
        /// The budget the pass was given.
        view_bytes: u64,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "unsupported wasm: {what}"),
            Self::Memory { needs, view_bytes } => write!(
                f,
                "the view starts with {needs} bytes of memory and tables, past its {view_bytes}"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// What stopped a metered call that trapped, read from [`GAS`] after the
/// trap (see [`stop`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The fuel ran out (wasmtime's `Trap::OutOfFuel`).
    OutOfFuel,
    /// A `memory.grow` or `table.grow` went past the budget (wasmtime's
    /// `ViewLimits` error).
    Memory,
    /// The view's own trap: a panic, an out-of-bounds access, a bad call.
    Trap,
}

/// Classifies a trap of a metered call by [`GAS`] read after it.
///
/// [`GAS`] is written only by the exports' wrappers and by the pass's own
/// traps, so a view's own trap leaves it at what the host armed: positive.
pub fn stop(gas_left: i64) -> Stop {
    if gas_left == GROW_REFUSED {
        Stop::Memory
    } else if gas_left <= 0 {
        Stop::OutOfFuel
    } else {
        Stop::Trap
    }
}

/// The metered module for `code`: fuel counted into [`GAS`], every grow
/// checked against `view_bytes` (linear memory plus [`ELEMENT_BYTES`] per
/// table element), the maxima lowered to it. Deterministic.
pub fn meter(code: &[u8], view_bytes: u64) -> Result<Vec<u8>, Refusal> {
    Validator::new_with_features(FEATURES)
        .validate_all(code)
        .map_err(|error| Refusal::Unsupported(error.to_string()))?;
    let mut module = ModuleConfig::new()
        .generate_producers_section(false)
        .parse(code)
        .map_err(|error| Refusal::Unsupported(format!("{error:#}")))?;

    if module.memories.iter().any(|memory| memory.import.is_some())
        || module.tables.iter().any(|table| table.import.is_some())
    {
        return Err(Refusal::Unsupported(
            "an imported memory or table".to_owned(),
        ));
    }
    if module.start.is_some() {
        return Err(Refusal::Unsupported(
            "a start function (it would run before the fuel is set)".to_owned(),
        ));
    }
    if module.exports.iter().any(|export| export.name == GAS) {
        return Err(Refusal::Unsupported(format!(
            "the module already exports {GAS}"
        )));
    }
    let needs = module
        .memories
        .iter()
        .map(|memory| memory.initial.saturating_mul(PAGE))
        .chain(
            module
                .tables
                .iter()
                .map(|table| table.initial.saturating_mul(ELEMENT_BYTES)),
        )
        .fold(0u64, u64::saturating_add);
    if needs > view_bytes {
        return Err(Refusal::Memory { needs, view_bytes });
    }
    // Never reached before the grow checks below; they bound what the
    // engine reserves and say the budget in the module itself.
    for memory in module.memories.iter_mut() {
        let pages = (view_bytes / PAGE).min(MEMORY_PAGES);
        memory.maximum = Some(memory.maximum.map_or(pages, |declared| declared.min(pages)));
    }
    for table in module.tables.iter_mut() {
        let elements = (view_bytes / ELEMENT_BYTES).min(u64::from(u32::MAX));
        table.maximum = Some(
            table
                .maximum
                .map_or(elements, |declared| declared.min(elements)),
        );
    }

    let gas = module
        .globals
        .add_local(ValType::I64, true, false, ConstExpr::Value(Value::I64(0)));
    // one local id each serves every function (walrus numbers locals per
    // function on emit)
    let fuel = module.locals.add(ValType::I64);
    let adapters = adapt_imports(&mut module);
    let cx = Cx {
        gas,
        fuel,
        scratch: module.locals.add(ValType::I32),
        memory: module.memories.iter().next().map(|memory| memory.id()),
        tables: module.tables.iter().map(|table| table.id()).collect(),
        // a budget past i64::MAX would never be reached by a wasm32 view
        budget: i64::try_from(view_bytes).unwrap_or(i64::MAX),
        adapters,
    };
    let original: Vec<FunctionId> = module
        .funcs
        .iter_local()
        .map(|(id, _)| id)
        .filter(|id| !cx.adapters.values().any(|adapter| adapter == id))
        .collect();
    for &id in &original {
        if let FunctionKind::Local(function) = &mut module.funcs.get_mut(id).kind {
            let entry = function.entry_block();
            meter_function(function.builder_mut(), &mut module.types, &cx, entry);
        }
    }
    for &id in &original {
        widen(&mut module, id, fuel);
    }
    redirect_import_values(&mut module, &cx.adapters);
    hand_over(&mut module, gas);
    module.exports.add(GAS, gas);
    Ok(module.emit_wasm())
}

/// What every rewritten sequence needs.
struct Cx {
    /// [`GAS`]: what the host sets and reads, written by the wrappers of the
    /// exports and by the pass's traps
    gas: GlobalId,
    /// the count: each function's extra parameter, which it returns as its
    /// extra result (wasmtime's `fuel_var`, kept in a register across calls)
    fuel: LocalId,
    /// a size-charged operator's length, a grow's delta, an indirect
    /// call's table index
    scratch: LocalId,
    memory: Option<MemoryId>,
    tables: Vec<TableId>,
    budget: i64,
    /// each imported function and the adapter that stands in for it where
    /// the module uses it as a value
    adapters: HashMap<FunctionId, FunctionId>,
}

/// One adapter per imported function, with the widened type: it calls the
/// import and hands the count through. An import keeps its own type, so a
/// direct call to it passes nothing; the adapter is what a table or a
/// `ref.func` holds instead, so an indirect call through the widened type
/// still reaches it. The adapter is the pass's own code: never charged.
fn adapt_imports(module: &mut walrus::Module) -> HashMap<FunctionId, FunctionId> {
    let imports: Vec<(FunctionId, Vec<ValType>, Vec<ValType>)> = module
        .funcs
        .iter()
        .filter(|function| matches!(function.kind, FunctionKind::Import(_)))
        .map(|function| {
            let (params, results) = module.types.params_results(function.ty());
            (function.id(), params.to_vec(), results.to_vec())
        })
        .collect();
    let mut adapters = HashMap::new();
    for (import, params, results) in imports {
        let args: Vec<LocalId> = params
            .iter()
            .map(|&param| module.locals.add(param))
            .collect();
        let count = module.locals.add(ValType::I64);
        let mut adapter = FunctionBuilder::new(&mut module.types, &wide(&params), &wide(&results));
        let mut body = adapter.func_body();
        for &arg in &args {
            body.local_get(arg);
        }
        body.call(import).local_get(count);
        let all = args.into_iter().chain([count]).collect();
        adapters.insert(import, adapter.finish(all, &mut module.funcs));
    }
    adapters
}

/// Elements and global initializers that name an import name its adapter.
fn redirect_import_values(module: &mut walrus::Module, adapters: &HashMap<FunctionId, FunctionId>) {
    let swap = |function: &mut FunctionId| {
        if let Some(&adapter) = adapters.get(function) {
            *function = adapter;
        }
    };
    let swap_expr = |expr: &mut ConstExpr| {
        if let ConstExpr::RefFunc(function) = expr {
            swap(function);
        }
    };
    for element in module.elements.iter_mut() {
        match &mut element.items {
            ElementItems::Functions(functions) => functions.iter_mut().for_each(swap),
            ElementItems::Expressions(_, exprs) => exprs.iter_mut().for_each(swap_expr),
        }
    }
    let globals: Vec<GlobalId> = module.globals.iter().map(|global| global.id()).collect();
    for global in globals {
        if let GlobalKind::Local(expr) = &mut module.globals.get_mut(global).kind {
            swap_expr(expr);
        }
    }
}

/// `types` with the count appended.
fn wide(types: &[ValType]) -> Vec<ValType> {
    types.iter().copied().chain([ValType::I64]).collect()
}

/// The widened form of function type `ty`: the count as one more parameter
/// and one more result.
fn wide_type(types: &mut ModuleTypes, ty: TypeId) -> TypeId {
    let (params, results) = types.params_results(ty);
    let (params, results) = (wide(params), wide(results));
    types.add(&params, &results)
}

/// Gives function `id` the widened type: its (metered) body is copied, as
/// it is, into a block of the function's original results inside a new body
/// that takes the count as its last parameter and pushes it as its last
/// result. A branch to the old body is a branch to that block, which then
/// falls through to the push; a `return` already pushes it (`meter_seq`).
fn widen(module: &mut walrus::Module, id: FunctionId, fuel: LocalId) {
    let ty = module.funcs.get(id).ty();
    let (params, results) = {
        let (params, results) = module.types.params_results(ty);
        (params.to_vec(), results.to_vec())
    };
    let inner_ty = InstrSeqType::new(&mut module.types, &[], &results);
    let mut builder = FunctionBuilder::new(&mut module.types, &wide(&params), &wide(&results));
    let FunctionKind::Local(old) = &module.funcs.get(id).kind else {
        return;
    };
    let inner = builder.dangling_instr_seq(inner_ty).id();
    copy_body(old, &mut builder, inner);
    builder
        .func_body()
        .instr(Block { seq: inner })
        .local_get(fuel);
    let args = old.args.iter().copied().chain([fuel]).collect();
    let widened = builder.finish(args, &mut module.funcs);
    let kind = std::mem::replace(
        &mut module.funcs.get_mut(widened).kind,
        FunctionKind::Uninitialized(ty),
    );
    module.funcs.get_mut(id).kind = kind;
    module.funcs.delete(widened);
}

/// Copies every sequence of `old`'s body into `builder`, its entry into
/// `inner`, with the block and branch references renamed. A worklist, not
/// recursion: untrusted nesting depth never grows the stack.
fn copy_body(old: &walrus::LocalFunction, builder: &mut FunctionBuilder, inner: InstrSeqId) {
    let mut names = HashMap::from([(old.entry_block(), inner)]);
    let mut work = vec![(old.entry_block(), inner)];
    let nest = |seq: InstrSeqId,
                builder: &mut FunctionBuilder,
                names: &mut HashMap<InstrSeqId, InstrSeqId>,
                work: &mut Vec<(InstrSeqId, InstrSeqId)>| {
        let copy = builder.dangling_instr_seq(old.block(seq).ty).id();
        names.insert(seq, copy);
        work.push((seq, copy));
        copy
    };
    while let Some((from, to)) = work.pop() {
        let mut copied = Vec::with_capacity(old.block(from).instrs.len());
        for (instr, loc) in &old.block(from).instrs {
            // a branch names an enclosing block, named before its body
            let name = |seq: &InstrSeqId| names.get(seq).copied().unwrap_or(*seq);
            let instr = match instr {
                Instr::Block(Block { seq }) => Instr::Block(Block {
                    seq: nest(*seq, builder, &mut names, &mut work),
                }),
                Instr::Loop(Loop { seq }) => Instr::Loop(Loop {
                    seq: nest(*seq, builder, &mut names, &mut work),
                }),
                Instr::IfElse(IfElse {
                    consequent,
                    alternative,
                }) => Instr::IfElse(IfElse {
                    consequent: nest(*consequent, builder, &mut names, &mut work),
                    alternative: nest(*alternative, builder, &mut names, &mut work),
                }),
                Instr::Br(Br { block }) => Instr::Br(Br { block: name(block) }),
                Instr::BrIf(BrIf { block }) => Instr::BrIf(BrIf { block: name(block) }),
                Instr::BrTable(BrTable { blocks, default }) => Instr::BrTable(BrTable {
                    blocks: blocks.iter().map(name).collect(),
                    default: name(default),
                }),
                other => other.clone(),
            };
            copied.push((instr, *loc));
        }
        *builder.instr_seq(to).instrs_mut() = copied;
    }
}

/// Every exported function becomes a wrapper with its original type that
/// hands the count over: [`GAS`] in as the extra argument, the extra result
/// back into [`GAS`]. The wrapper is the pass's own code: never charged.
fn hand_over(module: &mut walrus::Module, gas: GlobalId) {
    let exported: Vec<(walrus::ExportId, FunctionId)> = module
        .exports
        .iter()
        .filter_map(|export| match export.item {
            walrus::ExportItem::Function(function)
                if matches!(module.funcs.get(function).kind, FunctionKind::Local(_)) =>
            {
                Some((export.id(), function))
            }
            _ => None,
        })
        .collect();
    for (export, function) in exported {
        let (params, results) = module.types.params_results(module.funcs.get(function).ty());
        // the widened type less the count
        let params = params[..params.len().saturating_sub(1)].to_vec();
        let results = results[..results.len().saturating_sub(1)].to_vec();
        let args: Vec<LocalId> = params
            .iter()
            .map(|&param| module.locals.add(param))
            .collect();
        let mut wrapper = FunctionBuilder::new(&mut module.types, &params, &results);
        let mut body = wrapper.func_body();
        for &arg in &args {
            body.local_get(arg);
        }
        body.global_get(gas).call(function).global_set(gas);
        let wrapper = wrapper.finish(args, &mut module.funcs);
        module.exports.get_mut(export).item = walrus::ExportItem::Function(wrapper);
    }
}

type Seq = Vec<(Instr, InstrLocId)>;

fn put(seq: &mut Seq, instr: impl Into<Instr>) {
    seq.push((instr.into(), InstrLocId::default()));
}

fn i64_const(seq: &mut Seq, value: i64) {
    put(
        seq,
        Const {
            value: Value::I64(value),
        },
    );
}

fn binop(seq: &mut Seq, op: BinaryOp) {
    put(seq, Binop { op });
}

/// `fuel -= pending`; nothing when there is nothing to charge.
fn charge(cx: &Cx, seq: &mut Seq, pending: &mut i64) {
    if *pending > 0 {
        put(seq, LocalGet { local: cx.fuel });
        i64_const(seq, *pending);
        binop(seq, BinaryOp::I64Sub);
        put(seq, LocalSet { local: cx.fuel });
    }
    *pending = 0;
}

/// `if fuel <= 0 { gas = fuel; unreachable }`: wasmtime traps once the
/// consumed fuel reaches the budget.
fn check(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    let trap = {
        let mut trap = builder.dangling_instr_seq(None);
        trap.local_get(cx.fuel).global_set(cx.gas).unreachable();
        trap.id()
    };
    let empty = builder.dangling_instr_seq(None).id();
    put(seq, LocalGet { local: cx.fuel });
    i64_const(seq, 0);
    binop(seq, BinaryOp::I64LeS);
    put(
        seq,
        IfElse {
            consequent: trap,
            alternative: empty,
        },
    );
}

/// The size charge of a bulk operator whose length (or `table.grow` delta)
/// is on top of the stack: `fuel -= len`, the length kept in place and in
/// the scratch local, then a check.
fn charge_len(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    put(seq, LocalTee { local: cx.scratch });
    put(seq, LocalGet { local: cx.fuel });
    put(seq, LocalGet { local: cx.scratch });
    put(
        seq,
        Unop {
            op: UnaryOp::I64ExtendUI32,
        },
    );
    binop(seq, BinaryOp::I64Sub);
    put(seq, LocalSet { local: cx.fuel });
    check(builder, cx, seq);
}

/// Pushes the scratch local (an i32) widened to i64.
fn scratch_i64(cx: &Cx, seq: &mut Seq) {
    put(seq, LocalGet { local: cx.scratch });
    put(
        seq,
        Unop {
            op: UnaryOp::I64ExtendUI32,
        },
    );
}

/// Pushes the memory's size in bytes as i64 (0 without a memory).
fn memory_bytes(cx: &Cx, seq: &mut Seq) {
    match cx.memory {
        Some(memory) => {
            put(seq, MemorySize { memory });
            put(
                seq,
                Unop {
                    op: UnaryOp::I64ExtendUI32,
                },
            );
            i64_const(seq, PAGE as i64);
            binop(seq, BinaryOp::I64Mul);
        }
        None => i64_const(seq, 0),
    }
}

/// Pushes what every table's elements cost the budget, as i64.
fn table_bytes(cx: &Cx, seq: &mut Seq) {
    i64_const(seq, 0);
    for &table in &cx.tables {
        put(seq, TableSize { table });
        put(
            seq,
            Unop {
                op: UnaryOp::I64ExtendUI32,
            },
        );
        i64_const(seq, ELEMENT_BYTES as i64);
        binop(seq, BinaryOp::I64Mul);
        binop(seq, BinaryOp::I64Add);
    }
}

/// Consumes an i32 condition: when it holds, [`GAS`] becomes
/// [`GROW_REFUSED`] and the call traps.
fn refuse_if(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    let trap = {
        let mut trap = builder.dangling_instr_seq(None);
        trap.i64_const(GROW_REFUSED)
            .global_set(cx.gas)
            .unreachable();
        trap.id()
    };
    let empty = builder.dangling_instr_seq(None).id();
    put(
        seq,
        IfElse {
            consequent: trap,
            alternative: empty,
        },
    );
}

/// Before `memory.grow` (its delta on top of the stack, kept there): traps
/// as [`GROW_REFUSED`] when the delta is not 0 and the new size plus the
/// tables is past the budget. That is the order wasmtime asks in
/// (`Memory::grow`): a zero delta asks nothing, then
/// `ViewLimits::memory_growing`, and only then the memory's own maximum and
/// 4 GiB, which answer `-1`.
fn guard_memory_grow(builder: &mut FunctionBuilder, cx: &Cx, memory: MemoryId, seq: &mut Seq) {
    put(seq, LocalTee { local: cx.scratch });
    put(seq, LocalGet { local: cx.scratch });
    put(
        seq,
        Const {
            value: Value::I32(0),
        },
    );
    binop(seq, BinaryOp::I32Ne);
    put(seq, MemorySize { memory });
    put(
        seq,
        Unop {
            op: UnaryOp::I64ExtendUI32,
        },
    );
    scratch_i64(cx, seq);
    binop(seq, BinaryOp::I64Add);
    i64_const(seq, PAGE as i64);
    binop(seq, BinaryOp::I64Mul);
    table_bytes(cx, seq);
    binop(seq, BinaryOp::I64Add);
    i64_const(seq, cx.budget);
    binop(seq, BinaryOp::I64GtU);
    binop(seq, BinaryOp::I32And);
    refuse_if(builder, cx, seq);
}

/// Before `table.grow` (its delta on top of the stack and, after
/// [`charge_len`], in the scratch local): traps as [`GROW_REFUSED`] when the
/// delta is not 0 and memory plus every table plus the delta's elements is
/// past the budget (`ViewLimits::table_growing`).
fn guard_table_grow(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    put(seq, LocalGet { local: cx.scratch });
    put(
        seq,
        Const {
            value: Value::I32(0),
        },
    );
    binop(seq, BinaryOp::I32Ne);
    memory_bytes(cx, seq);
    table_bytes(cx, seq);
    binop(seq, BinaryOp::I64Add);
    scratch_i64(cx, seq);
    i64_const(seq, ELEMENT_BYTES as i64);
    binop(seq, BinaryOp::I64Mul);
    binop(seq, BinaryOp::I64Add);
    i64_const(seq, cx.budget);
    binop(seq, BinaryOp::I64GtU);
    binop(seq, BinaryOp::I32And);
    refuse_if(builder, cx, seq);
}

/// Meters one function's body: every sequence of the original code, from
/// the entry. A worklist, not recursion: untrusted nesting depth never grows
/// the stack.
fn meter_function(
    builder: &mut FunctionBuilder,
    types: &mut ModuleTypes,
    cx: &Cx,
    entry: InstrSeqId,
) {
    // a function invocation costs 1 (wasmtime: `fuel_consumed: 1`)
    let mut work = vec![(entry, 1, true)];
    while let Some((seq, pending, entry_check)) = work.pop() {
        meter_seq(builder, types, cx, seq, pending, entry_check, &mut work);
    }
}

/// Rewrites one instruction sequence of the original code. `pending` is the
/// cost carried in from the enclosing sequence (a `block` body continues
/// straight-line code); `entry_check` puts a budget check at its start
/// (function entry, loop header). The nested sequences go on `work`: only
/// the original sequences are visited, so the code the pass adds is never
/// charged.
fn meter_seq(
    builder: &mut FunctionBuilder,
    types: &mut ModuleTypes,
    cx: &Cx,
    seq: InstrSeqId,
    mut pending: i64,
    entry_check: bool,
    work: &mut Vec<(InstrSeqId, i64, bool)>,
) {
    let old = std::mem::take(builder.instr_seq(seq).instrs_mut());
    let mut new = Vec::with_capacity(old.len() + 8);
    if entry_check {
        charge(cx, &mut new, &mut pending);
        check(builder, cx, &mut new);
    }
    // the length of a bulk operator when the operator before it pushed it
    // as a constant: wasmtime folds such a cost and checks only past 128
    let mut constant: Option<i64> = None;
    for (instr, loc) in old {
        let length = constant.take();
        if let Instr::Const(Const {
            value: Value::I32(value),
        }) = &instr
        {
            constant = Some(i64::from(*value as u32));
        }
        let bulk = matches!(
            instr,
            Instr::MemoryCopy(_)
                | Instr::MemoryFill(_)
                | Instr::MemoryInit(_)
                | Instr::TableCopy(_)
                | Instr::TableFill(_)
                | Instr::TableInit(_)
        );
        if let (true, Some(length)) = (bulk, length) {
            pending += 1 + length;
            if length > SMALL_BULK {
                charge(cx, &mut new, &mut pending);
                check(builder, cx, &mut new);
            }
            new.push((instr, loc));
            continue;
        }
        let instr = match instr {
            Instr::Block(Block { seq: child }) => {
                // no charge on entry: the body continues our straight line
                work.push((child, pending, false));
                pending = 0;
                instr
            }
            Instr::Loop(Loop { seq: child }) => {
                charge(cx, &mut new, &mut pending);
                work.push((child, 0, true));
                instr
            }
            Instr::IfElse(IfElse {
                consequent,
                alternative,
            }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                work.push((consequent, 0, false));
                work.push((alternative, 0, false));
                instr
            }
            Instr::Br(Br { .. })
            | Instr::BrIf(BrIf { .. })
            | Instr::BrTable(BrTable { .. })
            // outside FEATURES, so never seen; charged as wasmtime would
            | Instr::ReturnCall(ReturnCall { .. })
            | Instr::ReturnCallIndirect(ReturnCallIndirect { .. }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                instr
            }
            Instr::Call(Call { func }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                // an import keeps its type: it neither takes nor gives the count
                if cx.adapters.contains_key(&func) {
                    new.push((instr, loc));
                    continue;
                }
                put(&mut new, LocalGet { local: cx.fuel });
                new.push((instr, loc));
                put(&mut new, LocalSet { local: cx.fuel });
                continue;
            }
            Instr::CallIndirect(CallIndirect { ty, table }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                // the count goes under the table index
                put(&mut new, LocalSet { local: cx.scratch });
                put(&mut new, LocalGet { local: cx.fuel });
                put(&mut new, LocalGet { local: cx.scratch });
                let ty = wide_type(types, ty);
                new.push((CallIndirect { ty, table }.into(), loc));
                put(&mut new, LocalSet { local: cx.fuel });
                continue;
            }
            Instr::Return(_) => {
                charge(cx, &mut new, &mut pending);
                put(&mut new, LocalGet { local: cx.fuel });
                instr
            }
            Instr::Unreachable(Unreachable { .. }) => {
                charge(cx, &mut new, &mut pending);
                instr
            }
            Instr::RefFunc(RefFunc { func }) => {
                pending += 1;
                RefFunc {
                    func: cx.adapters.get(&func).copied().unwrap_or(func),
                }
                .into()
            }
            Instr::Drop(_) => instr,
            Instr::MemoryCopy(_)
            | Instr::MemoryFill(_)
            | Instr::MemoryInit(_)
            | Instr::TableCopy(_)
            | Instr::TableFill(_)
            | Instr::TableInit(_)
            | Instr::TableGrow(_) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                charge_len(builder, cx, &mut new);
                instr
            }
            _ => {
                pending += 1;
                instr
            }
        };
        match &instr {
            Instr::MemoryGrow(MemoryGrow { memory }) => {
                guard_memory_grow(builder, cx, *memory, &mut new);
            }
            Instr::TableGrow(TableGrow { .. }) => guard_table_grow(builder, cx, &mut new),
            _ => {}
        }
        new.push((instr, loc));
    }
    // the `end` (or `else`) of this sequence
    charge(cx, &mut new, &mut pending);
    *builder.instr_seq(seq).instrs_mut() = new;
}
