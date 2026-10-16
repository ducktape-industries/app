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
//!   the same modules. The metered module is validated again before it is
//!   returned; a module the rewrite cannot express in valid wasm (a function
//!   at the 1000-parameter or 1000-result limit, which the count would push
//!   past it; a `ref.func` of an import that only its export declares) is
//!   refused the same way.
//! - **Fuel.** The count wasmtime 46 keeps (`wasmtime-environ`
//!   `default_operator_cost`, `wasmtime-cranelift` `fuel_before_op` /
//!   `pre_translate_bulk_op`): 1 per operator except `nop`, `drop`, `block`,
//!   `loop`, `unreachable`, `return`, `else`, `end`; 1 more per function
//!   invocation; the length on `memory.copy`, `memory.fill`, `memory.init`,
//!   `table.copy`, `table.fill`, `table.init` and the delta on `table.grow`.
//!   It is charged where wasmtime charges it and checked exactly where
//!   wasmtime checks it: at function entry, at every loop header, and after
//!   a size-charged operator unless wasmtime sees its length as a constant
//!   of at most 128 (`constants.rs` replays wasmtime's translator to know
//!   when it does). Like wasmtime's `fuel_var`, the count is the fuel
//!   consumed, below 0 while fuel is left, and a check traps once it is 0
//!   or more. It never leaves a register between an export's entry and exit:
//!   every function of the module takes it as one more parameter and gives
//!   it back as one more result (imports keep their type, and an import used
//!   as a value is reached through an adapter of the widened type).
//! - **The host's side** is one exported mutable `i64` global, [`GAS`]: the
//!   fuel left. The host sets it to its budget before a call and reads it
//!   after; each exported function is a wrapper of its original type that
//!   passes the negated [`GAS`] in and stores the negated count back. For a
//!   call that returns, used = budget - left, wasmtime's fuel count for the
//!   call. A call can run past its budget between two checks and return, as
//!   it does under wasmtime: [`GAS`] is then below 0, which is not an
//!   out-of-fuel stop (wasmtime's `get_fuel` reports 0 there, so the host
//!   caps the use it reports at the budget).
//! - **Memory.** Every `memory.grow` and `table.grow` first asks one helper
//!   the pass adds: the memory and every table as they are plus what the
//!   grow adds, against `view_bytes`. Past it the call traps with [`GAS`]
//!   set to [`GROW_REFUSED`] (a value no fuel trap leaves, so the one export
//!   carries both answers). Within it, a grow wasmtime answers with `-1`
//!   (past the module's own maximum) still answers `-1` and holds nothing,
//!   so it counts nothing: the budget is asked before the grow, in
//!   wasmtime's order, rather than every `-1` turned into a trap. (The
//!   desktop's `ViewLimits` records such a grow today, so a run of them can
//!   refuse a later grow there that fits here: the host's to fix.) The
//!   memory and table maxima are lowered to the budget too, and a module
//!   whose initial memory and tables alone exceed it is refused
//!   ([`Refusal::Memory`]).
//! - **After a trap**, [`stop`] reads [`GAS`]: out of fuel, past the memory
//!   budget, or the view's own trap. The module gives no use for a trapped
//!   call: the view's own trap leaves [`GAS`] at what the host armed (the
//!   count is in a register, never written back there), and wasmtime's own
//!   count after a trap is the last one it stored. The iOS host's glue
//!   (lane J part 2) caps the use it reports at the budget.
//! - **Imports must not call back into the module.** An import call passes
//!   no count and stores none in [`GAS`] first: an import that called an
//!   export would start it from a stale [`GAS`], and the outer wrapper's
//!   store would drop what the inner call used. The view ABI's one import,
//!   `panicked`, does not call back.
//!
//! The derived module carries one export more than the five the view ABI
//! names ([`GAS`]); the author ABI is unchanged and no view imports or
//! exports anything new. The output is deterministic: the same input and
//! budget give the same bytes.
//!
//! Map: this file is the rewrite; `constants.rs` validates the input and
//! finds where wasmtime folds a length; `main.rs` is the command
//! (`view-meter <in.wasm> <out.wasm> <view_bytes>`); `tests/` is the runner
//! equivalence gate against wasmtime (SPEC §13.3 (a)).
use std::collections::HashMap;

use walrus::ir::{
    BinaryOp, Binop, Block, Br, BrIf, BrTable, Call, CallIndirect, Const, IfElse, Instr,
    InstrLocId, InstrSeqId, InstrSeqType, LocalGet, LocalSet, LocalTee, Loop, RefFunc, ReturnCall,
    ReturnCallIndirect, TableGrow, UnaryOp, Unop, Unreachable, Value,
};
use walrus::{
    ConstExpr, ElementItems, FunctionBuilder, FunctionId, FunctionKind, GlobalId, GlobalKind,
    LocalId, ModuleConfig, ModuleTypes, TypeId, ValType,
};
use wasmparser::{Validator, WasmFeatures};

mod constants;

use constants::Constants;

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
/// sets before every call and reads after it. Below 0 after a call that ran
/// past its budget and returned (not an out-of-fuel stop).
pub const GAS: &str = "gas_left";

/// [`GAS`] after a trap that a `memory.grow` or `table.grow` past the budget
/// raised. No fuel trap leaves it: a check traps when the fuel consumed is
/// between 0 and `i64::MAX`, and leaves [`GAS`] at minus that.
pub const GROW_REFUSED: i64 = i64::MIN;

/// A bulk operator with a constant length up to this is folded into the
/// pending cost without a check (wasmtime `SMALL_BULK_OP_COST`).
pub(crate) const SMALL_BULK: i64 = 128;
/// Bytes per wasm page.
pub(crate) const PAGE: u64 = 1 << 16;
/// A 32-bit memory's own limit, in pages (4 GiB).
pub(crate) const MEMORY_PAGES: u64 = 1 << 16;
/// What one table element costs the budget: a pointer, as `ViewLimits`
/// charges it (`size_of::<usize>()` on the 64-bit hosts views run on).
const ELEMENT_BYTES: u64 = 8;

/// Why [`meter`] gave no module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The bytes are not wasm the runners accept: invalid, or using a
    /// feature outside [`FEATURES`], or a shape the pass does not take (an
    /// imported memory or table, an export already named [`GAS`], a start
    /// function, which would run before the host could set [`GAS`], or a
    /// module whose metered form would not be valid wasm). The text names
    /// what was refused.
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
    let constants = constants::validate(code)?;
    let mut module = ModuleConfig::new()
        .generate_producers_section(false)
        .parse(code)
        .map_err(|error| Refusal::Unsupported(format!("{error:#}")))?;
    refuse_shapes(&module)?;
    let budget = lower_maxima(&mut module, view_bytes)?;
    instrument(&mut module, constants, budget);
    let metered = module.emit_wasm();
    Validator::new_with_features(FEATURES)
        .validate_all(&metered)
        .map_err(|error| Refusal::Unsupported(format!("the metered module would be: {error}")))?;
    Ok(metered)
}

/// The shapes the pass does not take.
fn refuse_shapes(module: &walrus::Module) -> Result<(), Refusal> {
    let refuse = |what: &str| Err(Refusal::Unsupported(what.to_owned()));
    if module.memories.iter().any(|memory| memory.import.is_some())
        || module.tables.iter().any(|table| table.import.is_some())
    {
        return refuse("an imported memory or table");
    }
    if module.start.is_some() {
        return refuse("a start function (it would run before the fuel is set)");
    }
    if module.exports.iter().any(|export| export.name == GAS) {
        return refuse(&format!("the module already exports {GAS}"));
    }
    Ok(())
}

/// Refuses a module that starts past the budget and lowers the memory and
/// table maxima to it. Returns the budget as the pass compiles it in.
fn lower_maxima(module: &mut walrus::Module, view_bytes: u64) -> Result<i64, Refusal> {
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
    // never reached before the grow checks; they bound what the engine
    // reserves and say the budget in the module itself
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
    // a budget past i64::MAX would never be reached by a wasm32 view
    Ok(i64::try_from(view_bytes).unwrap_or(i64::MAX))
}

/// Meters every function of the module, widens it to carry the count, and
/// hands the count over at the exports.
fn instrument(module: &mut walrus::Module, constants: Constants, budget: i64) {
    // the module's own functions, before the pass adds any
    let original: Vec<FunctionId> = module.funcs.iter_local().map(|(id, _)| id).collect();
    let gas = module
        .globals
        .add_local(ValType::I64, true, false, ConstExpr::Value(Value::I64(0)));
    let grow = budget_helper(module, gas, budget);
    let adapters = adapt_imports(module);
    // one local id each serves every function (walrus numbers locals per
    // function on emit)
    let fuel = module.locals.add(ValType::I64);
    let cx = Cx {
        gas,
        fuel,
        scratch: module.locals.add(ValType::I32),
        grow,
        adapters,
        constants,
    };
    for &id in &original {
        if let FunctionKind::Local(function) = &mut module.funcs.get_mut(id).kind {
            let entry = function.entry_block();
            meter_function(function.builder_mut(), &mut module.types, &cx, entry);
        }
    }
    for &id in &original {
        widen(module, id, fuel);
    }
    redirect_import_values(module, &cx.adapters);
    hand_over(module, gas);
    module.exports.add(GAS, gas);
}

/// What every rewritten sequence needs.
struct Cx {
    /// [`GAS`]: what the host sets and reads, written by the wrappers of the
    /// exports and by the pass's traps
    gas: GlobalId,
    /// the count: each function's extra parameter, which it returns as its
    /// extra result (wasmtime's `fuel_var`: the fuel consumed, kept in a
    /// register across calls)
    fuel: LocalId,
    /// a size-charged operator's length, a grow's delta, an indirect
    /// call's table index
    scratch: LocalId,
    /// the helper every grow asks (`budget_helper`)
    grow: FunctionId,
    /// each imported function and the adapter that stands in for it where
    /// the module uses it as a value
    adapters: HashMap<FunctionId, FunctionId>,
    /// where wasmtime folds a size-charged operator's length
    constants: Constants,
}

impl Cx {
    fn is_import(&self, function: FunctionId) -> bool {
        self.adapters.contains_key(&function)
    }
}

/// The helper every `memory.grow` and `table.grow` calls first with the
/// bytes it would add: when the memory and every table as they are, plus
/// those bytes, are past the budget, [`GAS`] becomes [`GROW_REFUSED`] and
/// the call traps (`ViewLimits::memory_growing` / `table_growing`). One
/// function, so a grow costs the same few instructions however many tables
/// the module has. A zero delta asks nothing in wasmtime; here it asks and
/// always fits, since every size the module reached was asked first. The
/// helper is the pass's own code: never charged.
fn budget_helper(module: &mut walrus::Module, gas: GlobalId, budget: i64) -> FunctionId {
    let memory = module.memories.iter().next().map(|memory| memory.id());
    let tables: Vec<_> = module.tables.iter().map(|table| table.id()).collect();
    let adds = module.locals.add(ValType::I64);
    let mut helper = FunctionBuilder::new(&mut module.types, &[ValType::I64], &[]);
    let mut body = helper.func_body();
    body.local_get(adds);
    if let Some(memory) = memory {
        body.memory_size(memory)
            .unop(UnaryOp::I64ExtendUI32)
            .i64_const(PAGE.cast_signed())
            .binop(BinaryOp::I64Mul)
            .binop(BinaryOp::I64Add);
    }
    for table in tables {
        body.table_size(table)
            .unop(UnaryOp::I64ExtendUI32)
            .i64_const(ELEMENT_BYTES.cast_signed())
            .binop(BinaryOp::I64Mul)
            .binop(BinaryOp::I64Add);
    }
    body.i64_const(budget).binop(BinaryOp::I64GtU).if_else(
        None,
        |past| {
            past.i64_const(GROW_REFUSED).global_set(gas).unreachable();
        },
        |_| {},
    );
    helper.finish(vec![adds], &mut module.funcs)
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
    // `id` is one of the module's own functions (`instrument`); an emitted
    // module that broke this would fail the output validation in `meter`
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
            // a branch names an enclosing block, named before its body (a
            // name not found would be invalid wasm, refused in `meter`)
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
/// hands the count over: the fuel left in [`GAS`] goes in as the fuel
/// consumed (negated), and the count that comes back goes into [`GAS`]
/// negated again. The wrapper is the pass's own code: never charged.
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
        body.global_get(gas)
            .i64_const(-1)
            .binop(BinaryOp::I64Mul)
            .call(function)
            .i64_const(-1)
            .binop(BinaryOp::I64Mul)
            .global_set(gas);
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

/// `fuel += pending`; nothing when there is nothing to charge.
fn charge(cx: &Cx, seq: &mut Seq, pending: &mut i64) {
    if *pending > 0 {
        put(seq, LocalGet { local: cx.fuel });
        i64_const(seq, *pending);
        binop(seq, BinaryOp::I64Add);
        put(seq, LocalSet { local: cx.fuel });
    }
    *pending = 0;
}

/// `if fuel >= 0 { gas = -fuel; unreachable }`: wasmtime's `fuel_check`
/// (the fuel consumed has reached the budget).
fn check(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    let trap = {
        let mut trap = builder.dangling_instr_seq(None);
        trap.local_get(cx.fuel)
            .i64_const(-1)
            .binop(BinaryOp::I64Mul)
            .global_set(cx.gas)
            .unreachable();
        trap.id()
    };
    let empty = builder.dangling_instr_seq(None).id();
    put(seq, LocalGet { local: cx.fuel });
    i64_const(seq, 0);
    binop(seq, BinaryOp::I64GeS);
    put(
        seq,
        IfElse {
            consequent: trap,
            alternative: empty,
        },
    );
}

/// The size charge of a bulk operator whose length (or `table.grow` delta)
/// is on top of the stack and not a constant wasmtime folds: `fuel += len`,
/// the length kept in place and in the scratch local, then a check.
fn charge_len(builder: &mut FunctionBuilder, cx: &Cx, seq: &mut Seq) {
    put(seq, LocalTee { local: cx.scratch });
    put(seq, LocalGet { local: cx.fuel });
    scratch_i64(cx, seq);
    binop(seq, BinaryOp::I64Add);
    put(seq, LocalSet { local: cx.fuel });
    check(builder, cx, seq);
}

/// Before a grow whose delta is in the scratch local: asks the budget
/// helper about `delta * unit` more bytes.
fn ask_budget(cx: &Cx, seq: &mut Seq, unit: u64) {
    scratch_i64(cx, seq);
    i64_const(seq, unit.cast_signed());
    binop(seq, BinaryOp::I64Mul);
    put(seq, Call { func: cx.grow });
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
    for (instr, loc) in old {
        let instr = match instr {
            Instr::MemoryCopy(_)
            | Instr::MemoryFill(_)
            | Instr::MemoryInit(_)
            | Instr::TableCopy(_)
            | Instr::TableFill(_)
            | Instr::TableInit(_)
            | Instr::TableGrow(_) => {
                pending += 1;
                let grow = matches!(instr, Instr::TableGrow(TableGrow { .. }));
                match cx.constants.get(&loc.data()) {
                    // wasmtime folds a constant length into the run and
                    // checks only past 128
                    Some(&cost) => {
                        pending = pending.saturating_add(cost);
                        if cost > SMALL_BULK {
                            charge(cx, &mut new, &mut pending);
                            check(builder, cx, &mut new);
                        }
                        if grow {
                            put(&mut new, LocalTee { local: cx.scratch });
                        }
                    }
                    None => {
                        charge(cx, &mut new, &mut pending);
                        charge_len(builder, cx, &mut new);
                    }
                }
                // the budget after the fuel, in wasmtime's order
                if grow {
                    ask_budget(cx, &mut new, ELEMENT_BYTES);
                }
                instr
            }
            Instr::MemoryGrow(_) => {
                pending += 1;
                put(&mut new, LocalTee { local: cx.scratch });
                ask_budget(cx, &mut new, PAGE);
                instr
            }
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
                if cx.is_import(func) {
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
            _ => {
                pending += 1;
                instr
            }
        };
        new.push((instr, loc));
    }
    // the `end` (or `else`) of this sequence
    charge(cx, &mut new, &mut pending);
    *builder.instr_seq(seq).instrs_mut() = new;
}
