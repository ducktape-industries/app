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
//! - **Fuel.** One exported mutable `i64` global, [`GAS`], counts down the
//!   way wasmtime 46 counts fuel (`wasmtime-environ` `default_operator_cost`,
//!   `wasmtime-cranelift` `fuel_before_op` / `pre_translate_bulk_op`): 1 per
//!   operator except `nop`, `drop`, `block`, `loop`, `unreachable`, `return`,
//!   `else`, `end`; 1 more per function invocation; the length on
//!   `memory.copy`, `memory.fill`, `memory.init`, `table.copy`, `table.fill`,
//!   `table.init` and the delta on `table.grow`. Pending cost is charged at
//!   the points wasmtime charges it, and the budget is checked at function
//!   entry, at every loop header and after every size-charged operator; a
//!   check traps when [`GAS`] is `<= 0`. The host sets [`GAS`] to its budget
//!   before a call and reads it after: used = budget - left, equal to
//!   wasmtime's fuel count per call.
//! - **Memory.** Every `memory.grow` and `table.grow` first checks the whole
//!   budget the way `ViewLimits` does (the grown size, plus the memory or
//!   tables it does not grow, against `view_bytes`); past it the call traps
//!   with [`GAS`] set to [`GROW_REFUSED`]. A grow wasmtime answers with `-1`
//!   (past the module's own maximum, past 4 GiB, no allocation) still
//!   answers `-1`. The memory and table maxima are lowered to the budget too,
//!   and a module whose initial memory and tables alone exceed it is refused
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
    IfElse, Instr, InstrLocId, InstrSeqId, LocalGet, LocalTee, Loop, MemoryGrow, MemorySize,
    ReturnCall, ReturnCallIndirect, TableGrow, TableSize, UnaryOp, Unop, Unreachable, Value,
};
use walrus::{
    ConstExpr, FunctionBuilder, GlobalId, LocalId, MemoryId, ModuleConfig, TableId, ValType,
};
use wasmparser::{Validator, WasmFeatures};

/// The one wasm feature set every view runner accepts: what Rust's
/// `wasm32-unknown-unknown` emits (MVP, mutable globals, sign extension,
/// non-trapping float-to-int, multi-value, bulk memory, reference types).
/// No SIMD, tail calls, threads, exceptions, GC, multi-memory or memory64.
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

/// Bytes per wasm page.
const PAGE: u64 = 65_536;
/// A 32-bit memory's own limit, in pages: a grow past it answers `-1` before
/// any budget is asked (wasmtime `Memory::grow`, `allow_growth_to`).
const MEMORY_PAGES: i64 = 65_536;
/// What one table element costs the budget: a pointer, as `ViewLimits`
/// charges it (`size_of::<usize>()` on the 64-bit hosts views run on).
const ELEMENT_BYTES: u64 = 8;

/// Why [`meter`] gave no module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The bytes are not wasm the runners accept: invalid, or using a
    /// feature outside [`FEATURES`], or a shape the pass does not take (an
    /// imported memory or table, an export already named [`GAS`]). The text
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
/// A view trap in the few operators between a charge that took [`GAS`] to
/// zero or below and the next check reads as [`Stop::OutOfFuel`]; wasmtime
/// would name the view's trap there. Both stop the call at the same point.
pub fn stop(gas_left: i64) -> Stop {
    match gas_left {
        GROW_REFUSED => Stop::Memory,
        ..=0 => Stop::OutOfFuel,
        _ => Stop::Trap,
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
        let pages = view_bytes / PAGE;
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
    module.exports.add(GAS, gas);
    let cx = Cx {
        gas,
        // one scratch local id serves every function (walrus numbers locals
        // per function on emit): a size-charged operator's length, a grow's
        // delta
        scratch: module.locals.add(ValType::I32),
        memory: module.memories.iter().next().map(|memory| memory.id()),
        tables: module.tables.iter().map(|table| table.id()).collect(),
        // a budget past i64::MAX would never be reached by a wasm32 view
        budget: i64::try_from(view_bytes).unwrap_or(i64::MAX),
    };
    for (_, function) in module.funcs.iter_local_mut() {
        let entry = function.entry_block();
        // a function invocation costs 1 (wasmtime: `fuel_consumed: 1`)
        meter_seq(function.builder_mut(), &cx, entry, 1, true);
    }
    Ok(module.emit_wasm())
}

/// What every rewritten sequence needs.
struct Cx {
    gas: GlobalId,
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

/// `gas -= pending`; nothing when there is nothing to charge.
fn charge(cx: &Cx, seq: &mut Seq, pending: &mut i64) {
    if *pending > 0 {
        put(seq, GlobalGet { global: cx.gas });
        i64_const(seq, *pending);
        binop(seq, BinaryOp::I64Sub);
        put(seq, GlobalSet { global: cx.gas });
    }
    *pending = 0;
}

/// `if gas <= 0 { unreachable }`: wasmtime traps once the consumed fuel
/// reaches the budget.
fn check(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    let trap = {
        let mut trap = builder.dangling_instr_seq(None);
        trap.unreachable();
        trap.id()
    };
    let empty = builder.dangling_instr_seq(None).id();
    put(seq, GlobalGet { global: cx.gas });
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
/// is on top of the stack: `gas -= len`, the length kept in place and in the
/// scratch local, then a check.
fn charge_len(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    put(seq, LocalTee { local: cx.scratch });
    put(seq, GlobalGet { global: cx.gas });
    put(seq, LocalGet { local: cx.scratch });
    put(
        seq,
        Unop {
            op: UnaryOp::I64ExtendUI32,
        },
    );
    binop(seq, BinaryOp::I64Sub);
    put(seq, GlobalSet { global: cx.gas });
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
/// as [`GROW_REFUSED`] when the delta is not 0, the new size is within the
/// 32-bit limit and it, plus the tables, is past the budget. That is the
/// order wasmtime asks in: a zero delta asks nothing, a size past 4 GiB is
/// `-1`, then `ViewLimits::memory_growing`.
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
    let new_pages = |seq: &mut Seq| {
        put(seq, MemorySize { memory });
        put(
            seq,
            Unop {
                op: UnaryOp::I64ExtendUI32,
            },
        );
        scratch_i64(cx, seq);
        binop(seq, BinaryOp::I64Add);
    };
    new_pages(seq);
    i64_const(seq, MEMORY_PAGES);
    binop(seq, BinaryOp::I64LeU);
    binop(seq, BinaryOp::I32And);
    new_pages(seq);
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

/// Rewrites one instruction sequence of the original code. `pending` is the
/// cost carried in from the enclosing sequence (a `block` body continues
/// straight-line code); `entry_check` puts a budget check at its start
/// (function entry, loop header). Only the original sequences are visited:
/// the code the pass adds is never charged.
fn meter_seq(
    builder: &mut FunctionBuilder,
    cx: &Cx,
    seq: InstrSeqId,
    mut pending: i64,
    entry_check: bool,
) {
    let old = std::mem::take(builder.instr_seq(seq).instrs_mut());
    let mut new = Vec::with_capacity(old.len() + 8);
    if entry_check {
        charge(cx, &mut new, &mut pending);
        check(builder, cx, &mut new);
    }
    let mut children = Vec::new();
    for (instr, loc) in old {
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
            Instr::Br(Br { .. })
            | Instr::BrIf(BrIf { .. })
            | Instr::BrTable(BrTable { .. })
            | Instr::Call(Call { .. })
            | Instr::CallIndirect(CallIndirect { .. })
            // outside FEATURES, so never seen; charged as wasmtime would
            | Instr::ReturnCall(ReturnCall { .. })
            | Instr::ReturnCallIndirect(ReturnCallIndirect { .. }) => {
                pending += 1;
                charge(cx, &mut new, &mut pending);
            }
            Instr::Return(_) | Instr::Unreachable(Unreachable { .. }) => {
                charge(cx, &mut new, &mut pending);
            }
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
        new.push((instr, loc));
    }
    // the `end` (or `else`) of this sequence
    charge(cx, &mut new, &mut pending);
    *builder.instr_seq(seq).instrs_mut() = new;
    for (child, carried, entry_check) in children {
        meter_seq(builder, cx, child, carried, entry_check);
    }
}
