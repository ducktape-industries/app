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
//!   count is `<= 0`. As in wasmtime, the count lives in a function-local
//!   (a register) while a function runs and in one global between
//!   functions: loaded at function entry and after a call, stored before a
//!   call, a return and at the function's end. That global is internal;
//!   the host's side is one exported mutable `i64` global, [`GAS`], which
//!   every exported function's wrapper copies in before the call and out
//!   after it, and which a fuel or memory trap writes. The host sets [`GAS`]
//!   to its budget before a call and reads it after: used = budget - left,
//!   equal to wasmtime's fuel count per call, and fuel runs out at the same
//!   check.
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
use walrus::ir::{
    BinaryOp, Binop, Block, Br, BrIf, BrTable, Call, CallIndirect, Const, GlobalGet, GlobalSet,
    IfElse, Instr, InstrLocId, InstrSeqId, LocalGet, LocalSet, LocalTee, Loop, MemoryGrow,
    MemorySize, ReturnCall, ReturnCallIndirect, TableGrow, TableSize, UnaryOp, Unop, Unreachable,
    Value,
};
use walrus::{
    ConstExpr, FunctionBuilder, GlobalId, LocalId, MemoryId, ModuleConfig, TableId, ValType,
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
    let cx = Cx {
        gas,
        shared: module.globals.add_local(
            ValType::I64,
            true,
            false,
            ConstExpr::Value(Value::I64(0)),
        ),
        // one local id each serves every function (walrus numbers locals per
        // function on emit)
        fuel: module.locals.add(ValType::I64),
        scratch: module.locals.add(ValType::I32),
        memory: module.memories.iter().next().map(|memory| memory.id()),
        tables: module.tables.iter().map(|table| table.id()).collect(),
        // a budget past i64::MAX would never be reached by a wasm32 view
        budget: i64::try_from(view_bytes).unwrap_or(i64::MAX),
    };
    for (_, function) in module.funcs.iter_local_mut() {
        let entry = function.entry_block();
        // a function invocation costs 1 (wasmtime: `fuel_consumed: 1`)
        meter_seq(function.builder_mut(), &cx, entry, entry, 1, true);
    }
    hand_over(&mut module, &cx);
    module.exports.add(GAS, gas);
    Ok(module.emit_wasm())
}

/// Every exported function becomes a wrapper that hands the count over:
/// [`GAS`] into the internal global before the call, back after it. The
/// count crosses every internal call in the internal global, which engines
/// reach without the indirection an exported mutable global costs (in
/// JavaScriptCore the difference was 1.32x against 1.17x of the unmetered
/// tick). The wrapper is the pass's own code: never charged.
fn hand_over(module: &mut walrus::Module, cx: &Cx) {
    let exported: Vec<(walrus::ExportId, walrus::FunctionId)> = module
        .exports
        .iter()
        .filter_map(|export| match export.item {
            walrus::ExportItem::Function(function)
                if matches!(
                    module.funcs.get(function).kind,
                    walrus::FunctionKind::Local(_)
                ) =>
            {
                Some((export.id(), function))
            }
            _ => None,
        })
        .collect();
    for (export, function) in exported {
        let ty = module.types.get(module.funcs.get(function).ty());
        let (params, results) = (ty.params().to_vec(), ty.results().to_vec());
        let args: Vec<LocalId> = params
            .iter()
            .map(|&param| module.locals.add(param))
            .collect();
        let mut wrapper = FunctionBuilder::new(&mut module.types, &params, &results);
        let mut body = wrapper.func_body();
        body.global_get(cx.gas).global_set(cx.shared);
        for &arg in &args {
            body.local_get(arg);
        }
        body.call(function).global_get(cx.shared).global_set(cx.gas);
        let wrapper = wrapper.finish(args, &mut module.funcs);
        module.exports.get_mut(export).item = walrus::ExportItem::Function(wrapper);
    }
}

/// What every rewritten sequence needs.
struct Cx {
    /// [`GAS`]: what the host sets and reads, written by the wrappers of the
    /// exports and by a trap
    gas: GlobalId,
    /// the count between functions (internal, so cheap to reach)
    shared: GlobalId,
    /// the count while the function runs (wasmtime's `fuel_var`): loaded
    /// from `shared` at entry and after a call, stored back before a call,
    /// a return and the function's end
    fuel: LocalId,
    /// a size-charged operator's length, a grow's delta
    scratch: LocalId,
    memory: Option<MemoryId>,
    tables: Vec<TableId>,
    budget: i64,
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

/// `shared` = the function's count: before control leaves the function or
/// another function may read it (a call, a return, the end).
fn store(cx: &Cx, seq: &mut Seq) {
    put(seq, LocalGet { local: cx.fuel });
    put(seq, GlobalSet { global: cx.shared });
}

/// The function's count = `shared`: at entry and after a call returns.
fn load(cx: &Cx, seq: &mut Seq) {
    put(seq, GlobalGet { global: cx.shared });
    put(seq, LocalSet { local: cx.fuel });
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

/// Rewrites one instruction sequence of the original code. `entry` is the
/// function's body (a branch to it returns); `pending` is the cost carried
/// in from the enclosing sequence (a `block` body continues straight-line
/// code); `entry_check` puts a budget check at its start (function entry,
/// loop header). Only the original sequences are visited: the code the pass
/// adds is never charged.
fn meter_seq(
    builder: &mut FunctionBuilder,
    cx: &Cx,
    entry: InstrSeqId,
    seq: InstrSeqId,
    mut pending: i64,
    entry_check: bool,
) {
    let old = std::mem::take(builder.instr_seq(seq).instrs_mut());
    let mut new = Vec::with_capacity(old.len() + 8);
    if seq == entry {
        load(cx, &mut new);
    }
    if entry_check {
        charge(cx, &mut new, &mut pending);
        check(builder, cx, &mut new);
    }
    let mut children = Vec::new();
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
        match &instr {
            Instr::Block(Block { seq: child }) => {
                // no charge on entry: the body continues our straight line
                children.push((*child, pending, false));
                pending = 0;
            }
            Instr::Loop(Loop { seq: child }) => {
                charge(cx, &mut new, &mut pending);
                children.push((*child, 0, true));
            }
            Instr::IfElse(IfElse {
                consequent,
                alternative,
            }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                children.push((*consequent, 0, false));
                children.push((*alternative, 0, false));
            }
            Instr::Br(Br { block }) | Instr::BrIf(BrIf { block }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                if *block == entry {
                    store(cx, &mut new);
                }
            }
            Instr::BrTable(BrTable { blocks, default }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                if *default == entry || blocks.contains(&entry) {
                    store(cx, &mut new);
                }
            }
            Instr::Call(Call { .. })
            | Instr::CallIndirect(CallIndirect { .. })
            // outside FEATURES, so never seen; charged as wasmtime would
            | Instr::ReturnCall(ReturnCall { .. })
            | Instr::ReturnCallIndirect(ReturnCallIndirect { .. }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
                store(cx, &mut new);
            }
            Instr::Return(_) => {
                charge(cx, &mut new, &mut pending);
                store(cx, &mut new);
            }
            // a trap: nothing reads the count after it but the host, from
            // [`GAS`], which keeps what it was armed with: the view's trap
            Instr::Unreachable(Unreachable { .. }) => charge(cx, &mut new, &mut pending),
            Instr::Drop(_) => {}
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
            }
            _ => pending += 1,
        }
        match &instr {
            Instr::MemoryGrow(MemoryGrow { memory }) => {
                guard_memory_grow(builder, cx, *memory, &mut new);
            }
            Instr::TableGrow(TableGrow { .. }) => guard_table_grow(builder, cx, &mut new),
            _ => {}
        }
        let returns_here = matches!(instr, Instr::Call(_) | Instr::CallIndirect(_));
        new.push((instr, loc));
        if returns_here {
            load(cx, &mut new);
        }
    }
    // the `end` (or `else`) of this sequence
    charge(cx, &mut new, &mut pending);
    if seq == entry {
        store(cx, &mut new);
    }
    *builder.instr_seq(seq).instrs_mut() = new;
    for (child, carried, entry_check) in children {
        meter_seq(builder, cx, entry, child, carried, entry_check);
    }
}
