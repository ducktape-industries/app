//! The meter pass: a view's wasm rewritten so it carries its own bounds.
//!
//! On the desktop and Android, wasmtime counts a view's fuel and bounds its
//! memory at run time: the `memory.view_bytes` budget
//! (`ViewLimits`: linear memory plus 8 bytes per table element). On iOS the
//! views run in JavaScriptCore, which has neither, and a page stuck in a wasm
//! loop cannot be stopped from outside (wasm-mobile S0-webkit §7). So
//! [`meter`] compiles both bounds into the module, with the same numbers and
//! the same answers:
//!
//! - **What a runner takes.** [`accepts`] is the one rule every view runner
//!   applies before it compiles a view: the module validates against
//!   [`FEATURES`], and none of the shapes below is in it. Anything else is
//!   refused as [`Refusal::Unsupported`], so untrusted network bytes never
//!   reach the rewrite unchecked, and wasmtime and JavaScriptCore take and
//!   refuse the same views. The shapes: an imported memory or table; an
//!   `externref` table or global (its barriers are control flow the pass
//!   does not replay); a start function (it would run before the fuel is
//!   set); an export named [`GAS`]; a function type at the 1000-parameter
//!   or 1000-result limit (the count would take it past); and a module
//!   whose metered form could pass one of wasm's other limits (locals per
//!   function, a body's size, the numbers of functions, types, globals,
//!   exports and element segments), bounded from what the pass adds without
//!   running it (`accepts.rs`). So whatever [`accepts`] takes, [`meter`]
//!   meters: it applies the same rule first, and validates its output again
//!   before it returns it.
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
//!   count after a trap is the last one it stored.
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
//! Map: this file is the rewrite; `accepts.rs` is the rule a module must
//! pass; `constants.rs` finds where wasmtime folds a length; `main.rs` is
//! the command
//! (`view-meter <in.wasm> <out.wasm> <view_bytes>`); `tests/` is the runner
//! equivalence gate against wasmtime (SPEC §13.3 (a)).
//!
//! The rewrite streams: it never holds the module as a tree. A first read
//! keeps the few small sections the pass adds to and meters each body,
//! operator by operator, into a code section it holds whole (`Pass::read`).
//! A second read writes every section as it is (wasm-encoder's `reencode`)
//! with the pass's additions appended, and copies that code section in at
//! its place. So it holds the input, the metered code section and the
//! output it is copied into: a few times the input (a 2.7 MB view took
//! 213 MB as walrus's tree, 11 MB here). Every function, type and global
//! the module had keeps its index, and what the pass adds comes after them;
//! only a body's own locals move, one up, past the count.
use std::collections::BTreeSet;
use std::convert::Infallible;

use wasm_encoder::reencode::{Reencode, utils};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ElementSection, Elements, ExportKind, ExportSection,
    FuncType, Function, FunctionSection, GlobalSection, GlobalType, InstructionSink, MemoryType,
    Module, NameSection, SectionId, TableType, TypeSection, ValType,
};
use wasmparser::{
    CodeSectionReader, CustomSectionReader, ElementSectionReader, ExportSectionReader,
    ExternalKind, FunctionBody, FunctionSectionReader, GlobalSectionReader, KnownCustom, Name,
    NameSectionReader, Operator, Parser, Payload, TypeRef, TypeSectionReader, Validator,
    WasmFeatures,
};

mod accepts;
mod constants;

use constants::Constants;

/// The one wasm feature set every view runner accepts: what Rust's
/// `wasm32-unknown-unknown` emits (MVP, mutable globals, sign extension,
/// non-trapping float-to-int, multi-value, bulk memory, reference types).
/// No tail calls, threads, exceptions, GC, multi-memory or memory64; and no
/// SIMD, which Rust emits only when asked and no view asks for (the pass
/// would charge it as wasmtime does, 1 per operator: adding it is this one
/// line).
/// Both runners apply it through [`accepts`], which validates against it
/// before anything is compiled.
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

pub(crate) const SMALL_BULK_OP_FOLDING_LIMIT: i64 = 128;
pub(crate) const PAGE: u64 = 1 << 16;
pub(crate) const MEMORY_PAGES: u64 = 1 << 16;
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

/// Whether a view runner takes `code` (see the crate doc): the one rule both
/// the wasmtime and the JavaScriptCore runner apply, so a view runs on every
/// platform or on none.
pub fn accepts(code: &[u8]) -> Result<(), Refusal> {
    accepts::check(code)
}

/// The metered module for `code`: fuel counted into [`GAS`], every grow
/// checked against `view_bytes` (linear memory plus [`ELEMENT_BYTES`] per
/// table element), the maxima lowered to it. Refuses what [`accepts`]
/// refuses. Deterministic.
pub fn meter(code: &[u8], view_bytes: u64) -> Result<Vec<u8>, Refusal> {
    accepts::check(code)?;
    let constants = constants::find(code)?;
    let mut pass = Pass::read(code, &constants, view_bytes)?;
    let mut metered = Module::new();
    pass.parse_core_module(&mut metered, Parser::new(0), code)
        .map_err(unsupported)?;
    let metered = metered.finish();
    Validator::new_with_features(FEATURES)
        .validate_all(&metered)
        .map_err(|error| Refusal::Unsupported(format!("the metered module would be: {error}")))?;
    Ok(metered)
}

fn unsupported(error: impl std::fmt::Display) -> Refusal {
    Refusal::Unsupported(error.to_string())
}

type Error = wasm_encoder::reencode::Error<Infallible>;

#[derive(Default)]
struct Pass {
    view_bytes: u64,
    budget: i64,
    types: Vec<FuncType>,
    blocks: Vec<BlockType>,
    imports: Vec<u32>,
    functions: Vec<u32>,
    memories: Vec<u64>,
    tables: Vec<u64>,
    globals: u32,
    exported: Vec<u32>,
    code: CodeSection,
    referenced: BTreeSet<u32>,
    written: Vec<SectionId>,
}

impl Pass {
    fn read(code: &[u8], constants: &Constants, view_bytes: u64) -> Result<Self, Refusal> {
        let mut pass = Self {
            view_bytes,
            ..Self::default()
        };
        for payload in Parser::new(0).parse_all(code) {
            match payload.map_err(unsupported)? {
                Payload::TypeSection(types) => {
                    for ty in types.into_iter_err_on_gc_types() {
                        let ty = FuncType::try_from(ty.map_err(unsupported)?);
                        pass.types.push(ty.map_err(unsupported)?);
                    }
                    pass.blocks = pass.blocks();
                }
                Payload::ImportSection(imports) => {
                    for import in imports.into_imports() {
                        match import.map_err(unsupported)?.ty {
                            TypeRef::Func(ty) => pass.imports.push(ty),
                            TypeRef::Global(_) => pass.globals += 1,
                            _ => {}
                        }
                    }
                }
                Payload::FunctionSection(functions) => {
                    for ty in functions {
                        pass.functions.push(ty.map_err(unsupported)?);
                    }
                }
                Payload::TableSection(tables) => {
                    for table in tables {
                        pass.tables.push(table.map_err(unsupported)?.ty.initial);
                    }
                }
                Payload::MemorySection(memories) => {
                    for memory in memories {
                        pass.memories.push(memory.map_err(unsupported)?.initial);
                    }
                }
                Payload::GlobalSection(globals) => pass.globals += globals.count(),
                Payload::ExportSection(exports) => {
                    for export in exports {
                        let export = export.map_err(unsupported)?;
                        if export.kind == ExternalKind::Func && export.index >= pass.imported() {
                            pass.exported.push(export.index);
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let body = pass.meter_body(&body, constants)?;
                    pass.code.function(&body);
                }
                _ => {}
            }
        }
        pass.budget = pass.budget()?;
        Ok(pass)
    }

    fn budget(&self) -> Result<i64, Refusal> {
        let needs = self
            .memories
            .iter()
            .map(|pages| pages.saturating_mul(PAGE))
            .chain(
                self.tables
                    .iter()
                    .map(|elements| elements.saturating_mul(ELEMENT_BYTES)),
            )
            .fold(0u64, u64::saturating_add);
        let view_bytes = self.view_bytes;
        if needs > view_bytes {
            return Err(Refusal::Memory { needs, view_bytes });
        }
        Ok(i64::try_from(view_bytes).unwrap_or(i64::MAX))
    }

    fn blocks(&self) -> Vec<BlockType> {
        let mut added = 2 * self.types.len() as u32;
        self.types
            .iter()
            .map(|ty| match ty.results() {
                [] => BlockType::Empty,
                &[result] => BlockType::Result(result),
                _ => {
                    added += 1;
                    BlockType::FunctionType(added)
                }
            })
            .collect()
    }

    fn imported(&self) -> u32 {
        self.imports.len() as u32
    }

    fn wide(&self, ty: u32) -> u32 {
        self.types.len() as u32 + ty
    }

    fn params(&self, ty: u32) -> u32 {
        self.types[ty as usize].params().len() as u32
    }

    fn type_of(&self, function: u32) -> u32 {
        match function.checked_sub(self.imported()) {
            Some(defined) => self.functions[defined as usize],
            None => self.imports[function as usize],
        }
    }

    fn helper(&self) -> u32 {
        self.imported() + self.functions.len() as u32
    }

    fn value(&self, function: u32) -> u32 {
        match function < self.imported() {
            true => self.helper() + 1 + function,
            false => function,
        }
    }

    fn meter_body(
        &mut self,
        body: &FunctionBody<'_>,
        constants: &Constants,
    ) -> Result<Function, Refusal> {
        let ty = self.functions[self.code.len() as usize];
        let params = self.params(ty);
        let mut locals = Vec::new();
        let mut declared = 0;
        for local in body.get_locals_reader().map_err(unsupported)? {
            let (count, ty) = local.map_err(unsupported)?;
            declared += count;
            locals.push((count, ValType::try_from(ty).map_err(unsupported)?));
        }
        locals.push((1, ValType::I32));
        let mut out = Body {
            function: Function::new(locals),
            fuel: params,
            scratch: params + 1 + declared,
            gas: self.globals,
            grow: self.helper(),
            pending: 1,
        };
        let local = |index: u32| index + u32::from(index >= params);
        let fuel = out.fuel;
        out.sink().block(self.blocks[ty as usize]);
        out.charge();
        out.check();
        let mut depth = 0u32;
        let mut operators = body.get_operators_reader().map_err(unsupported)?;
        while !operators.eof() {
            let (op, offset) = operators.read_with_offset().map_err(unsupported)?;
            match op {
                Operator::MemoryCopy { .. }
                | Operator::MemoryFill { .. }
                | Operator::MemoryInit { .. }
                | Operator::TableCopy { .. }
                | Operator::TableFill { .. }
                | Operator::TableInit { .. }
                | Operator::TableGrow { .. } => {
                    out.pending += 1;
                    let grow = matches!(op, Operator::TableGrow { .. });
                    let at = u32::try_from(offset).ok();
                    match at.and_then(|at| constants.get(&at)) {
                        Some(&cost) => {
                            out.pending = out.pending.saturating_add(cost);
                            if cost > SMALL_BULK_OP_FOLDING_LIMIT {
                                out.charge();
                                out.check();
                            }
                            if grow {
                                let scratch = out.scratch;
                                out.sink().local_tee(scratch);
                            }
                        }
                        None => {
                            out.charge();
                            out.charge_dynamic_bulk_length();
                        }
                    }
                    if grow {
                        out.ask_budget(ELEMENT_BYTES);
                    }
                }
                Operator::MemoryGrow { .. } => {
                    out.pending += 1;
                    let scratch = out.scratch;
                    out.sink().local_tee(scratch);
                    out.ask_budget(PAGE);
                }
                Operator::Block { .. } => depth += 1,
                Operator::Loop { blockty } => {
                    depth += 1;
                    out.charge();
                    let blockty = self.block_type(blockty).map_err(unsupported)?;
                    out.sink().loop_(blockty);
                    out.check();
                    continue;
                }
                Operator::If { .. } => {
                    depth += 1;
                    out.pending += 1;
                    out.charge();
                }
                Operator::Else => out.charge(),
                Operator::End => {
                    out.charge();
                    if depth == 0 {
                        out.sink().end().local_get(fuel).end();
                        continue;
                    }
                    depth -= 1;
                }
                Operator::Br { .. } | Operator::BrIf { .. } | Operator::BrTable { .. } => {
                    out.pending += 1;
                    out.charge();
                }
                Operator::Call { function_index } => {
                    out.pending += 1;
                    out.charge();
                    if function_index < self.imported() {
                        out.sink().call(function_index);
                    } else {
                        out.sink()
                            .local_get(fuel)
                            .call(function_index)
                            .local_set(fuel);
                    }
                    continue;
                }
                Operator::CallIndirect {
                    type_index,
                    table_index,
                } => {
                    out.pending += 1;
                    out.charge();
                    let scratch = out.scratch;
                    out.sink()
                        .local_set(scratch)
                        .local_get(fuel)
                        .local_get(scratch)
                        .call_indirect(table_index, self.wide(type_index))
                        .local_set(fuel);
                    continue;
                }
                Operator::Return => {
                    out.charge();
                    out.sink().local_get(fuel);
                }
                Operator::Unreachable => out.charge(),
                Operator::RefFunc { function_index } => {
                    out.pending += 1;
                    let function = self.value(function_index);
                    self.referenced.insert(function);
                    out.sink().ref_func(function);
                    continue;
                }
                Operator::LocalGet { local_index } => {
                    out.pending += 1;
                    out.sink().local_get(local(local_index));
                    continue;
                }
                Operator::LocalSet { local_index } => {
                    out.pending += 1;
                    out.sink().local_set(local(local_index));
                    continue;
                }
                Operator::LocalTee { local_index } => {
                    out.pending += 1;
                    out.sink().local_tee(local(local_index));
                    continue;
                }
                Operator::Drop | Operator::Nop => {}
                _ => out.pending += 1,
            }
            let instruction = self.instruction(op).map_err(unsupported)?;
            out.function.instruction(&instruction);
        }
        operators.finish().map_err(unsupported)?;
        Ok(out.function)
    }

    fn budget_helper(&self) -> Function {
        let mut helper = Function::new([]);
        let mut body = helper.instructions();
        body.local_get(0);
        if !self.memories.is_empty() {
            body.memory_size(0)
                .i64_extend_i32_u()
                .i64_const(PAGE.cast_signed())
                .i64_mul()
                .i64_add();
        }
        for table in 0..self.tables.len() as u32 {
            body.table_size(table)
                .i64_extend_i32_u()
                .i64_const(ELEMENT_BYTES.cast_signed())
                .i64_mul()
                .i64_add();
        }
        body.i64_const(self.budget)
            .i64_gt_u()
            .if_(BlockType::Empty)
            .i64_const(GROW_REFUSED)
            .global_set(self.globals)
            .unreachable()
            .end()
            .end();
        helper
    }

    fn adapter(&self, import: u32) -> Function {
        let params = self.params(self.imports[import as usize]);
        let mut adapter = Function::new([]);
        let mut body = adapter.instructions();
        for param in 0..params {
            body.local_get(param);
        }
        body.call(import).local_get(params).end();
        adapter
    }

    fn wrapper(&self, function: u32) -> Function {
        let mut wrapper = Function::new([]);
        let mut body = wrapper.instructions();
        for param in 0..self.params(self.type_of(function)) {
            body.local_get(param);
        }
        body.global_get(self.globals)
            .i64_const(-1)
            .i64_mul()
            .call(function)
            .i64_const(-1)
            .i64_mul()
            .global_set(self.globals)
            .end();
        wrapper
    }

    fn write_types(&mut self, types: &mut TypeSection) {
        self.written.push(SectionId::Type);
        for ty in &self.types {
            types.ty().func_type(ty);
        }
        for ty in &self.types {
            types
                .ty()
                .func_type(&FuncType::new(wide(ty.params()), wide(ty.results())));
        }
        types.ty().function([ValType::I64], []);
        for ty in self.types.iter().filter(|ty| ty.results().len() > 1) {
            types.ty().function([], ty.results().iter().copied());
        }
    }

    fn write_functions(&mut self, functions: &mut FunctionSection) {
        self.written.push(SectionId::Function);
        for &ty in &self.functions {
            functions.function(self.wide(ty));
        }
        functions.function(2 * self.types.len() as u32);
        for &ty in &self.imports {
            functions.function(self.wide(ty));
        }
        for &function in &self.exported {
            functions.function(self.type_of(function));
        }
    }

    fn write_globals(&mut self, globals: &mut GlobalSection) {
        self.written.push(SectionId::Global);
        let ty = GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        };
        globals.global(ty, &ConstExpr::i64_const(0));
    }

    fn write_exports(&mut self, exports: &mut ExportSection) {
        self.written.push(SectionId::Export);
        exports.export(GAS, ExportKind::Global, self.globals);
    }

    fn write_elements(&mut self, elements: &mut ElementSection) {
        self.written.push(SectionId::Element);
        if !self.referenced.is_empty() {
            let referenced: Vec<u32> = self.referenced.iter().copied().collect();
            elements.declared(Elements::Functions(referenced.into()));
        }
    }

    fn write_code(&mut self, code: &mut CodeSection) {
        self.written.push(SectionId::Code);
        *code = std::mem::take(&mut self.code);
        code.function(&self.budget_helper());
        for import in 0..self.imported() {
            code.function(&self.adapter(import));
        }
        for &function in &self.exported {
            code.function(&self.wrapper(function));
        }
    }

    fn names(&mut self, names: NameSectionReader<'_>) -> Result<NameSection, Error> {
        let mut out = NameSection::new();
        for subsection in names {
            match subsection? {
                Name::Function(map) => out.functions(&utils::name_map(map, Ok::<u32, Error>)?),
                Name::Local(_) | Name::Label(_) => {}
                other => utils::parse_custom_name_subsection(self, &mut out, other)?,
            }
        }
        Ok(out)
    }
}

fn wide(types: &[ValType]) -> impl Iterator<Item = ValType> + '_ {
    types.iter().copied().chain([ValType::I64])
}

const WASM_SECTION_ORDER: [SectionId; 13] = [
    SectionId::Type,
    SectionId::Import,
    SectionId::Function,
    SectionId::Table,
    SectionId::Memory,
    SectionId::Tag,
    SectionId::Global,
    SectionId::Export,
    SectionId::Start,
    SectionId::Element,
    SectionId::DataCount,
    SectionId::Code,
    SectionId::Data,
];

fn place(id: SectionId) -> usize {
    WASM_SECTION_ORDER
        .iter()
        .position(|&known| known == id)
        .unwrap_or(WASM_SECTION_ORDER.len())
}

impl Reencode for Pass {
    type Error = Infallible;

    fn function_index(&mut self, function: u32) -> Result<u32, Error> {
        Ok(self.value(function))
    }

    fn memory_type(&mut self, memory: wasmparser::MemoryType) -> Result<MemoryType, Error> {
        let mut memory = utils::memory_type(self, memory);
        let pages = (self.view_bytes / PAGE).min(MEMORY_PAGES);
        memory.maximum = Some(memory.maximum.map_or(pages, |declared| declared.min(pages)));
        Ok(memory)
    }

    fn table_type(&mut self, table: wasmparser::TableType) -> Result<TableType, Error> {
        let mut table = utils::table_type(self, table)?;
        let elements = (self.view_bytes / ELEMENT_BYTES).min(u64::from(u32::MAX));
        table.maximum = Some(
            table
                .maximum
                .map_or(elements, |declared| declared.min(elements)),
        );
        Ok(table)
    }

    fn parse_type_section(
        &mut self,
        types: &mut TypeSection,
        _: TypeSectionReader<'_>,
    ) -> Result<(), Error> {
        self.write_types(types);
        Ok(())
    }

    fn parse_function_section(
        &mut self,
        functions: &mut FunctionSection,
        _: FunctionSectionReader<'_>,
    ) -> Result<(), Error> {
        self.write_functions(functions);
        Ok(())
    }

    fn parse_global_section(
        &mut self,
        globals: &mut GlobalSection,
        section: GlobalSectionReader<'_>,
    ) -> Result<(), Error> {
        utils::parse_global_section(self, globals, section)?;
        self.write_globals(globals);
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut ExportSection,
        section: ExportSectionReader<'_>,
    ) -> Result<(), Error> {
        let mut wrapper = self.helper() + 1 + self.imported();
        for export in section {
            let export = export?;
            let mut index = export.index;
            if export.kind == ExternalKind::Func && index >= self.imported() {
                index = wrapper;
                wrapper += 1;
            }
            exports.export(export.name, utils::export_kind(self, export.kind), index);
        }
        self.write_exports(exports);
        Ok(())
    }

    fn parse_element_section(
        &mut self,
        elements: &mut ElementSection,
        section: ElementSectionReader<'_>,
    ) -> Result<(), Error> {
        utils::parse_element_section(self, elements, section)?;
        self.write_elements(elements);
        Ok(())
    }

    fn parse_code_section(
        &mut self,
        code: &mut CodeSection,
        _: CodeSectionReader<'_>,
    ) -> Result<(), Error> {
        self.write_code(code);
        Ok(())
    }

    fn parse_custom_section(
        &mut self,
        module: &mut Module,
        section: CustomSectionReader<'_>,
    ) -> Result<(), Error> {
        match (section.name(), section.as_known()) {
            ("producers", _) => {}
            ("name", KnownCustom::Name(names)) => {
                if let Ok(names) = self.names(names) {
                    module.section(&names);
                }
            }
            ("name", _) => {}
            _ => {
                module.section(&utils::custom_section(self, section));
            }
        }
        Ok(())
    }

    fn intersperse_section_hook(
        &mut self,
        module: &mut Module,
        _: Option<SectionId>,
        before: Option<SectionId>,
    ) -> Result<(), Error> {
        let next = before.map_or(WASM_SECTION_ORDER.len(), place);
        let added = [
            SectionId::Type,
            SectionId::Function,
            SectionId::Global,
            SectionId::Export,
            SectionId::Element,
            SectionId::Code,
        ];
        for id in added {
            if place(id) >= next || self.written.contains(&id) {
                continue;
            }
            match id {
                SectionId::Type => {
                    let mut types = TypeSection::new();
                    self.write_types(&mut types);
                    module.section(&types);
                }
                SectionId::Function => {
                    let mut functions = FunctionSection::new();
                    self.write_functions(&mut functions);
                    module.section(&functions);
                }
                SectionId::Global => {
                    let mut globals = GlobalSection::new();
                    self.write_globals(&mut globals);
                    module.section(&globals);
                }
                SectionId::Export => {
                    let mut exports = ExportSection::new();
                    self.write_exports(&mut exports);
                    module.section(&exports);
                }
                SectionId::Element => {
                    let mut elements = ElementSection::new();
                    self.write_elements(&mut elements);
                    if !elements.is_empty() {
                        module.section(&elements);
                    }
                }
                _ => {
                    let mut code = CodeSection::new();
                    self.write_code(&mut code);
                    module.section(&code);
                }
            }
        }
        Ok(())
    }
}

struct Body {
    function: Function,
    fuel: u32,
    scratch: u32,
    gas: u32,
    grow: u32,
    pending: i64,
}

impl Body {
    fn sink(&mut self) -> InstructionSink<'_> {
        self.function.instructions()
    }

    fn charge(&mut self) {
        if self.pending > 0 {
            let (fuel, pending) = (self.fuel, self.pending);
            self.sink()
                .local_get(fuel)
                .i64_const(pending)
                .i64_add()
                .local_set(fuel);
        }
        self.pending = 0;
    }

    fn check(&mut self) {
        let (fuel, gas) = (self.fuel, self.gas);
        self.sink()
            .local_get(fuel)
            .i64_const(0)
            .i64_ge_s()
            .if_(BlockType::Empty)
            .local_get(fuel)
            .i64_const(-1)
            .i64_mul()
            .global_set(gas)
            .unreachable()
            .end();
    }

    fn charge_dynamic_bulk_length(&mut self) {
        let (fuel, scratch) = (self.fuel, self.scratch);
        self.sink()
            .local_tee(scratch)
            .local_get(fuel)
            .local_get(scratch)
            .i64_extend_i32_u()
            .i64_add()
            .local_set(fuel);
        self.check();
    }

    fn ask_budget(&mut self, unit: u64) {
        let (scratch, grow) = (self.scratch, self.grow);
        self.sink()
            .local_get(scratch)
            .i64_extend_i32_u()
            .i64_const(unit.cast_signed())
            .i64_mul()
            .call(grow);
    }
}
