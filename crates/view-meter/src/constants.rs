//! Validation (the shapes every view runner refuses, [`crate::accepts`]),
//! and where wasmtime folds a size-charged operator's cost.
//!
//! wasmtime 46 charges `memory.copy`, `memory.fill`, `memory.init`,
//! `table.copy`, `table.fill`, `table.init` and `table.grow` by their length
//! (delta) and checks the budget after the charge, except when that length
//! is a compile-time constant: then the cost joins the straight-line run and
//! the check happens only past 128 (`wasmtime-cranelift` `func_environ.rs`,
//! `pre_translate_bulk_op` and `value_as_const_int`). "Constant" means the
//! Cranelift SSA value is an `iconst`, and a `local.get` reads its variable
//! through Cranelift's SSA builder (`cranelift-frontend` `ssa.rs`, Braun et
//! al.). That builder sees through `local.set` and `local.tee`, through
//! blocks with one predecessor and through joins where every predecessor
//! carries the same definition, and it leaves a phi it cannot yet prove
//! redundant: whether a length read after a loop is a constant depends on
//! whether the same variable was read inside the loop before the loop was
//! closed, and on every join wasmtime's own code adds inside the loop. A
//! pass that only looked at the instruction before the operator checks
//! where wasmtime does not; one that reasoned per wasm block would too.
//!
//! So this module replays the translation: as the validator walks each
//! original body, it builds the blocks wasmtime builds (`code_translator.rs`
//! for the wasm control flow; `func_environ.rs` for its own: the two-way
//! join of every fuel check, the lazy initialization of a funcref read, the
//! loops of `table.fill`, `table.grow`, `table.init` and `table.copy`), with
//! the same predecessor order and sealing order, the same reachability (no
//! dead code; nothing after a load or store past the memory's maximum),
//! and runs Cranelift's own variable lookup on them at each `local.get`,
//! with values reduced to "this `iconst`" or "something else". The operand
//! stack keeps each value's identity, as the translator's does.
//!
//! An `externref` table or global, whose garbage collector barriers would
//! add control flow of their own, is refused before any body is read, so
//! every table here holds `funcref`s.
//!
//! The result maps each size-charged operator wasmtime sees a constant
//! length for (by its byte offset in the module, which walrus keeps as the
//! instruction's `InstrLocId`) to the cost wasmtime folds for it.
use std::collections::{HashMap, HashSet};

use wasmparser::{
    BinaryReaderError, BlockType, CompositeInnerType, FuncType, FuncValidator,
    FuncValidatorAllocations, FunctionBody, MemoryType, Operator, Parser, Payload, RefType,
    TypeRef, ValType, ValidPayload, Validator, ValidatorResources, WasmModuleResources,
};

use crate::{FEATURES, GAS, MEMORY_PAGES, PAGE, Refusal, SMALL_BULK};

/// Each size-charged operator whose length wasmtime sees as a constant, by
/// its byte offset in the module, and the cost wasmtime folds for it.
pub(crate) type Constants = HashMap<u32, i64>;

/// Validates `code` against [`FEATURES`], refuses the shapes no view runner
/// takes, and, when `replay`, finds its [`Constants`].
pub(crate) fn validate(code: &[u8], replay: bool) -> Result<Constants, Refusal> {
    let mut validator = Validator::new_with_features(FEATURES);
    let mut module = Module::default();
    let mut constants = Constants::new();
    let mut allocations = FuncValidatorAllocations::default();
    for payload in Parser::new(0).parse_all(code) {
        let payload = payload.map_err(refuse)?;
        let valid = validator.payload(&payload).map_err(refuse)?;
        module.read(&payload)?;
        if let ValidPayload::Func(func, body) = valid
            && replay
        {
            let mut func = func.into_validator(std::mem::take(&mut allocations));
            Translation::new(&module, &func)?.run(&mut func, &body, &mut constants)?;
            allocations = func.into_allocations();
        }
    }
    Ok(constants)
}

fn refuse(error: BinaryReaderError) -> Refusal {
    Refusal::Unsupported(error.to_string())
}

/// The most parameters, and the most results, a function type may have
/// (wasmparser's `MAX_WASM_FUNCTION_PARAMS` and `MAX_WASM_FUNCTION_RETURNS`,
/// which wasmtime validates with): the count is one more of each.
const ARITY: usize = 1000;

/// What a body's translation reads from the rest of the module.
#[derive(Default)]
struct Module {
    /// Per global: the immediate of the `iconst` wasmtime compiles a
    /// `global.get` of it to (an immutable global defined by a lone
    /// `i32.const`, sign-extended as `translate_global_get` does, or a lone
    /// `i64.const`).
    globals: Vec<Option<i64>>,
    /// Memory 0's largest size in bytes (`Memory::maximum_byte_size`): a load
    /// or store past it traps unconditionally, and wasmtime stops
    /// translating there.
    memory_bytes: Option<u64>,
}

impl Module {
    /// Reads what the translation needs and refuses the shapes no runner
    /// takes ([`crate::accepts`]).
    fn read(&mut self, payload: &Payload<'_>) -> Result<(), Refusal> {
        let refused = |what: &str| Err(Refusal::Unsupported(what.to_owned()));
        let externref = "an externref table or global";
        match payload {
            Payload::TypeSection(types) => {
                for ty in types.clone().into_iter_err_on_gc_types() {
                    let ty = ty.map_err(refuse)?;
                    if ty.params().len() >= ARITY || ty.results().len() >= ARITY {
                        return refused(&format!(
                            "a function type at the {ARITY}-parameter or {ARITY}-result \
                             limit (the count would take it past)"
                        ));
                    }
                }
            }
            Payload::ImportSection(imports) => {
                for import in imports.clone().into_imports() {
                    match import.map_err(refuse)?.ty {
                        TypeRef::Memory(_) | TypeRef::Table(_) => {
                            return refused("an imported memory or table");
                        }
                        TypeRef::Global(global) if is_externref(global.content_type) => {
                            return refused(externref);
                        }
                        TypeRef::Global(_) => self.globals.push(None),
                        _ => {}
                    }
                }
            }
            Payload::TableSection(tables) => {
                for table in tables.clone() {
                    if table.map_err(refuse)?.ty.element_type != RefType::FUNCREF {
                        return refused(externref);
                    }
                }
            }
            Payload::MemorySection(memories) => {
                for memory in memories.clone() {
                    self.memory(memory.map_err(refuse)?);
                }
            }
            Payload::GlobalSection(globals) => {
                for global in globals.clone() {
                    let global = global.map_err(refuse)?;
                    if is_externref(global.ty.content_type) {
                        return refused(externref);
                    }
                    let mut init = global.init_expr.get_operators_reader();
                    let value = match init.read().map_err(refuse)? {
                        Operator::I32Const { value } => Some(i64::from(value)),
                        Operator::I64Const { value } => Some(value),
                        _ => None,
                    };
                    let lone = matches!(init.read().map_err(refuse)?, Operator::End);
                    self.globals
                        .push(value.filter(|_| lone && !global.ty.mutable));
                }
            }
            Payload::StartSection { .. } => {
                return refused("a start function (it would run before the fuel is set)");
            }
            Payload::ExportSection(exports) => {
                for export in exports.clone() {
                    if export.map_err(refuse)?.name == GAS {
                        return refused(&format!("the module already exports {GAS}"));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn memory(&mut self, memory: MemoryType) {
        if self.memory_bytes.is_none() {
            let pages = memory
                .maximum
                .unwrap_or_else(|| memory.initial.max(MEMORY_PAGES));
            self.memory_bytes = Some(pages.saturating_mul(PAGE));
        }
    }
}

/// A reference type other than `funcref`: under reference types, `externref`.
fn is_externref(ty: ValType) -> bool {
    matches!(ty, ValType::Ref(element) if element != RefType::FUNCREF)
}

fn integer(ty: ValType) -> bool {
    matches!(ty, ValType::I32 | ValType::I64)
}

/// An SSA value, by index.
type Value = u32;
/// The shared value for everything that is not an integer constant.
const OPAQUE: Value = 0;
/// The function's entry block.
const ENTRY: u32 = 0;

#[derive(Clone, Copy)]
enum Val {
    /// An `iconst`, with its immediate as Cranelift stores it.
    Const(i64),
    /// Anything else.
    Opaque,
    /// A block parameter the SSA builder placed for a variable.
    Param,
    /// A parameter found redundant: the value it stands for.
    Alias(Value),
}

#[derive(Default)]
struct Block {
    preds: Vec<u32>,
    sealed: bool,
    single_pred: Option<u32>,
    /// variables looked up before the block was sealed, with their parameter
    undef: Vec<(u32, Value)>,
}

enum Call {
    UseVar(u32),
    Finish { sentinel: Value, block: u32 },
}

/// Cranelift's `SSABuilder` for one function, on the blocks the translator
/// builds; values are [`Val`]s.
struct Ssa {
    values: Vec<Val>,
    blocks: Vec<Block>,
    defs: HashMap<(u32, u32), Value>,
    /// each variable's definition in the entry block
    entry: Vec<Value>,
    /// whether each variable is an integer (a missing definition is `iconst 0`)
    int: Vec<bool>,
    calls: Vec<Call>,
    results: Vec<Value>,
    visited: HashSet<u32>,
}

impl Ssa {
    fn value(&mut self, value: Val) -> Value {
        self.values.push(value);
        u32::try_from(self.values.len() - 1).unwrap_or(OPAQUE)
    }

    fn block(&mut self) -> u32 {
        self.blocks.push(Block::default());
        u32::try_from(self.blocks.len() - 1).unwrap_or(ENTRY)
    }

    fn resolve(&self, mut value: Value) -> Value {
        while let Some(Val::Alias(next)) = self.values.get(value as usize) {
            value = *next;
        }
        value
    }

    /// The immediate of the `iconst` `value` stands for, if it is one.
    fn constant(&self, value: Value) -> Option<i64> {
        match self.values.get(self.resolve(value) as usize) {
            Some(Val::Const(imm)) => Some(*imm),
            _ => None,
        }
    }

    fn add_pred(&mut self, block: u32, pred: u32) {
        self.blocks[block as usize].preds.push(pred);
    }

    fn def_var(&mut self, var: u32, block: u32, value: Value) {
        self.defs.insert((var, block), value);
    }

    fn def(&self, var: u32, block: u32) -> Option<Value> {
        if let Some(&value) = self.defs.get(&(var, block)) {
            Some(value)
        } else if block == ENTRY {
            Some(self.entry[var as usize])
        } else {
            None
        }
    }

    fn use_var(&mut self, var: u32, block: u32) -> Value {
        self.use_var_nonlocal(var, block);
        self.run(var)
    }

    /// Cranelift's `run_state_machine`: an explicit stack, never recursion.
    fn run(&mut self, var: u32) -> Value {
        while let Some(call) = self.calls.pop() {
            match call {
                Call::UseVar(block) => self.use_var_nonlocal(var, block),
                Call::Finish { sentinel, block } => self.finish(var, sentinel, block),
            }
        }
        self.results.pop().unwrap_or(OPAQUE)
    }

    fn use_var_nonlocal(&mut self, var: u32, mut block: u32) {
        if let Some(value) = self.def(var, block) {
            self.results.push(value);
            return;
        }
        let (value, from) = self.find_var(var, block);
        while block != from {
            self.def_var(var, block, value);
            match self.blocks[block as usize].single_pred {
                Some(pred) => block = pred,
                None => break,
            }
        }
    }

    fn find_var(&mut self, var: u32, mut block: u32) -> (Value, u32) {
        self.visited.clear();
        while let Some(pred) = self.blocks[block as usize].single_pred {
            if !self.visited.insert(block) {
                break;
            }
            block = pred;
            if let Some(value) = self.def(var, block) {
                self.results.push(value);
                return (value, block);
            }
        }
        let param = self.value(Val::Param);
        self.def_var(var, block, param);
        if self.blocks[block as usize].sealed {
            self.begin_preds(param, block);
        } else {
            self.blocks[block as usize].undef.push((var, param));
            self.results.push(param);
        }
        (param, block)
    }

    fn begin_preds(&mut self, sentinel: Value, block: u32) {
        self.calls.push(Call::Finish { sentinel, block });
        let preds = &self.blocks[block as usize].preds;
        self.calls
            .extend(preds.iter().rev().map(|&pred| Call::UseVar(pred)));
    }

    /// Cranelift's `finish_predecessors_lookup`: a parameter every
    /// predecessor fills with the same definition is that definition.
    fn finish(&mut self, var: u32, sentinel: Value, block: u32) {
        let count = self.blocks[block as usize].preds.len();
        let at = self.results.len().saturating_sub(count);
        let mut found = None;
        let mut agree = true;
        for &value in &self.results[at..] {
            let value = self.resolve(value);
            if value == sentinel {
                continue;
            }
            match found {
                None => found = Some(value),
                Some(first) if first != value => agree = false,
                Some(_) => {}
            }
        }
        self.results.truncate(at);
        let result = match found {
            Some(value) if agree => value,
            Some(_) => sentinel,
            // used but never defined: Cranelift's `iconst 0`
            None => match self.int[var as usize] {
                true => self.value(Val::Const(0)),
                false => OPAQUE,
            },
        };
        if result != sentinel {
            self.values[sentinel as usize] = Val::Alias(result);
        }
        self.results.push(result);
    }

    fn seal(&mut self, block: u32) {
        let data = &mut self.blocks[block as usize];
        if data.sealed {
            return;
        }
        data.sealed = true;
        if let [pred] = data.preds[..] {
            data.single_pred = Some(pred);
        }
        for (var, param) in std::mem::take(&mut data.undef) {
            self.begin_preds(param, block);
            self.run(var);
        }
    }
}

#[derive(PartialEq)]
enum Kind {
    Block,
    Loop,
    If,
}

/// A control frame (`ControlStackFrame`).
struct Frame {
    kind: Kind,
    /// entered in reachable code (otherwise a placeholder: nothing in it is
    /// translated)
    live: bool,
    /// the operand stack height below the frame's parameters
    base: usize,
    /// the block after the frame (`following_code`; an `if`'s destination)
    next: u32,
    /// a loop's header (`br_destination`)
    header: u32,
    /// an `if`'s head block and parameters, for its `else`
    head: u32,
    params: Vec<Value>,
    /// an `if` whose parameters equal its results: the head branches to the
    /// destination until an `else` moves that edge (`ElseData::NoElse`)
    else_lazy: bool,
    else_seen: bool,
    consequent_ends_reachable: bool,
    branched_to: bool,
}

impl Frame {
    fn new(kind: Kind, live: bool, base: usize, next: u32) -> Self {
        Self {
            kind,
            live,
            base,
            next,
            header: ENTRY,
            head: ENTRY,
            params: Vec::new(),
            else_lazy: false,
            else_seen: false,
            consequent_ends_reachable: false,
            branched_to: false,
        }
    }
}

/// One body's translation.
struct Translation<'m> {
    module: &'m Module,
    ssa: Ssa,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    current: u32,
    reachable: bool,
}

/// The arity of a translated operator, which every valid one has.
fn need(arity: Option<(u32, u32)>) -> Result<(usize, usize), Refusal> {
    arity
        .map(|(params, results)| (params as usize, results as usize))
        .ok_or_else(|| Refusal::Unsupported("an operator of unknown arity".to_owned()))
}

/// A load's or store's offset and the bytes it touches (wasmtime's
/// `access_size`), for the memory operators in [`FEATURES`].
fn access(op: &Operator<'_>) -> Option<(u64, u64)> {
    use Operator as O;
    let (memarg, size) = match op {
        O::I32Load8S { memarg }
        | O::I32Load8U { memarg }
        | O::I64Load8S { memarg }
        | O::I64Load8U { memarg }
        | O::I32Store8 { memarg }
        | O::I64Store8 { memarg } => (memarg, 1),
        O::I32Load16S { memarg }
        | O::I32Load16U { memarg }
        | O::I64Load16S { memarg }
        | O::I64Load16U { memarg }
        | O::I32Store16 { memarg }
        | O::I64Store16 { memarg } => (memarg, 2),
        O::I32Load { memarg }
        | O::F32Load { memarg }
        | O::I64Load32S { memarg }
        | O::I64Load32U { memarg }
        | O::I32Store { memarg }
        | O::F32Store { memarg }
        | O::I64Store32 { memarg } => (memarg, 4),
        O::I64Load { memarg }
        | O::F64Load { memarg }
        | O::I64Store { memarg }
        | O::F64Store { memarg } => (memarg, 8),
        _ => return None,
    };
    Some((memarg.offset, size))
}

impl<'m> Translation<'m> {
    fn new(module: &'m Module, func: &FuncValidator<ValidatorResources>) -> Result<Self, Refusal> {
        let resources = func.resources();
        let params = resources
            .type_index_of_function(func.index())
            .and_then(|ty| function_type(func, ty))
            .map(|ty| ty.params().to_vec())
            .ok_or_else(|| Refusal::Unsupported("a function without a type".to_owned()))?;
        let mut ssa = Ssa {
            values: vec![Val::Opaque],
            blocks: Vec::new(),
            defs: HashMap::new(),
            entry: vec![OPAQUE; params.len()],
            int: params.iter().map(|&ty| integer(ty)).collect(),
            calls: Vec::new(),
            results: Vec::new(),
            visited: HashSet::new(),
        };
        let entry = ssa.block();
        ssa.seal(entry);
        let exit = ssa.block();
        Ok(Self {
            module,
            ssa,
            stack: Vec::new(),
            frames: vec![Frame::new(Kind::Block, true, 0, exit)],
            current: entry,
            reachable: true,
        })
    }

    fn run(
        mut self,
        func: &mut FuncValidator<ValidatorResources>,
        body: &FunctionBody<'_>,
        constants: &mut Constants,
    ) -> Result<(), Refusal> {
        let mut locals = body.get_locals_reader().map_err(refuse)?;
        for _ in 0..locals.get_count() {
            let offset = locals.original_position();
            let (count, ty) = locals.read().map_err(refuse)?;
            func.define_locals(offset, count, ty).map_err(refuse)?;
            // `declare_locals`: one `iconst 0` per declaration, for all its
            // locals
            let zero = match integer(ty) {
                true => self.ssa.value(Val::Const(0)),
                false => OPAQUE,
            };
            let count = count as usize;
            self.ssa.entry.extend(std::iter::repeat_n(zero, count));
            self.ssa.int.extend(std::iter::repeat_n(integer(ty), count));
        }
        // `fuel_function_entry`
        self.fuel_check();
        let mut operators = body.get_operators_reader().map_err(refuse)?;
        while !operators.eof() {
            let (op, offset) = operators.read_with_offset().map_err(refuse)?;
            let arity = op.operator_arity(&*func);
            func.op(offset, &op).map_err(refuse)?;
            let offset = u32::try_from(offset)
                .map_err(|_| Refusal::Unsupported("a module past 4 GiB".to_owned()))?;
            if self.reachable {
                self.step(func, &op, offset, arity, constants)?;
            } else {
                self.step_unreachable(&op, arity)?;
            }
        }
        operators.finish().map_err(refuse)
    }

    fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(OPAQUE)
    }

    fn pop_n(&mut self, count: usize) {
        self.stack.truncate(self.stack.len().saturating_sub(count));
    }

    fn push_opaque(&mut self, count: usize) {
        self.stack.extend(std::iter::repeat_n(OPAQUE, count));
    }

    /// A new block whose only predecessor is the current one, sealed, made
    /// current (an `if`'s consequent, `br_if`'s fallthrough).
    fn continue_alone(&mut self) {
        let block = self.ssa.block();
        self.ssa.add_pred(block, self.current);
        self.ssa.seal(block);
        self.current = block;
    }

    /// A branch around one block that rejoins (`brif` to a side block that
    /// jumps to the continuation): the fuel check's out-of-gas call, a
    /// funcref's lazy initialization.
    fn rejoin(&mut self) {
        let side = self.ssa.block();
        let join = self.ssa.block();
        self.ssa.add_pred(join, self.current);
        self.ssa.add_pred(side, self.current);
        self.ssa.seal(side);
        self.ssa.add_pred(join, side);
        self.ssa.seal(join);
        self.current = join;
    }

    /// `fuel_check`.
    fn fuel_check(&mut self) {
        self.rejoin();
    }

    /// `emit_raw_array_or_table_fill` on a lazily initialized funcref table:
    /// a loop of one block.
    fn fill_loop(&mut self) {
        let start = self.current;
        let body = self.ssa.block();
        let after = self.ssa.block();
        self.ssa.add_pred(after, start);
        self.ssa.add_pred(body, start);
        self.ssa.add_pred(after, body);
        self.ssa.add_pred(body, body);
        self.current = after;
        self.ssa.seal(body);
        self.ssa.seal(after);
    }

    /// `translate_per_element_copy`: a forward and a backward loop, each
    /// reading a lazily initialized funcref when the source is a table.
    fn per_element_copy(&mut self, read_rejoins: bool) {
        let start = self.current;
        let nonempty = self.ssa.block();
        let forward = self.ssa.block();
        let backward = self.ssa.block();
        let done = self.ssa.block();
        self.ssa.add_pred(nonempty, start);
        self.ssa.add_pred(done, start);
        self.ssa.add_pred(forward, nonempty);
        self.ssa.add_pred(backward, nonempty);
        for direction in [forward, backward] {
            self.current = direction;
            if read_rejoins {
                self.rejoin();
            }
            self.ssa.add_pred(done, self.current);
            self.ssa.add_pred(direction, self.current);
        }
        self.current = done;
        for block in [nonempty, forward, backward, done] {
            self.ssa.seal(block);
        }
    }

    /// `translate_table_grow` past its fuel: on success, a fill of the new
    /// elements, then the join.
    fn table_grow(&mut self) {
        let grow = self.current;
        let fill = self.ssa.block();
        let done = self.ssa.block();
        self.ssa.add_pred(done, grow);
        self.ssa.add_pred(fill, grow);
        self.current = fill;
        self.fill_loop();
        self.ssa.add_pred(done, self.current);
        self.current = done;
        self.ssa.seal(fill);
        self.ssa.seal(done);
    }

    /// The edge of a branch to label `depth` (`br_destination`).
    fn branch(&mut self, depth: u32) -> u32 {
        let at = self.frames.len().saturating_sub(1 + depth as usize);
        let frame = &mut self.frames[at];
        match frame.kind {
            Kind::Loop => frame.header,
            Kind::Block | Kind::If => {
                frame.branched_to = true;
                frame.next
            }
        }
    }

    /// The `else` of a live `if` (its head reachable): a block whose
    /// predecessor is the head, with the `if`'s parameters again.
    fn enter_else(&mut self) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        frame.else_seen = true;
        let (head, next, base, params) = (frame.head, frame.next, frame.base, frame.params.clone());
        if frame.else_lazy {
            // `change_jump_destination`: the head's edge leaves the
            // destination (`swap_remove`) for the new else block
            let preds = &mut self.ssa.blocks[next as usize].preds;
            if let Some(at) = preds.iter().position(|&pred| pred == head) {
                preds.swap_remove(at);
            }
            let block = self.ssa.block();
            self.ssa.add_pred(block, head);
            self.ssa.seal(block);
            self.current = block;
        } else {
            // allocated at the `if`: its one predecessor is the head
            let block = self.ssa.block();
            self.ssa.add_pred(block, head);
            self.ssa.seal(block);
            self.current = block;
        }
        self.reachable = true;
        self.stack.truncate(base);
        self.stack.extend(params);
    }

    /// Continues after `frame` in its next block, its results on the stack.
    fn follow(&mut self, frame: &Frame, results: usize) {
        self.current = frame.next;
        self.ssa.seal(frame.next);
        self.reachable = true;
        self.stack.truncate(frame.base);
        self.push_opaque(results);
    }

    fn step(
        &mut self,
        func: &FuncValidator<ValidatorResources>,
        op: &Operator<'_>,
        offset: u32,
        arity: Option<(u32, u32)>,
        constants: &mut Constants,
    ) -> Result<(), Refusal> {
        match *op {
            Operator::Block { .. } => {
                let (params, _) = need(arity)?;
                let next = self.ssa.block();
                let base = self.stack.len().saturating_sub(params);
                self.frames.push(Frame::new(Kind::Block, true, base, next));
            }
            Operator::Loop { .. } => {
                let (params, _) = need(arity)?;
                self.pop_n(params);
                let base = self.stack.len();
                self.push_opaque(params);
                let header = self.ssa.block();
                let next = self.ssa.block();
                self.ssa.add_pred(header, self.current);
                self.current = header;
                let mut frame = Frame::new(Kind::Loop, true, base, next);
                frame.header = header;
                self.frames.push(frame);
                // `translate_loop_header`
                self.fuel_check();
            }
            Operator::If { blockty } => {
                let (inputs, _) = need(arity)?;
                self.pop();
                let base = self.stack.len().saturating_sub(inputs.saturating_sub(1));
                let head = self.current;
                let consequent = self.ssa.block();
                let next = self.ssa.block();
                let mut frame = Frame::new(Kind::If, true, base, next);
                frame.head = head;
                frame.params = self.stack[base..].to_vec();
                frame.else_lazy = params_equal_results(func, blockty)?;
                // the head's `brif`: the consequent, and the destination
                // until an `else` (or a block allocated for the `else`)
                self.ssa.add_pred(consequent, head);
                if frame.else_lazy {
                    self.ssa.add_pred(next, head);
                }
                self.frames.push(frame);
                self.ssa.seal(consequent);
                self.current = consequent;
            }
            Operator::Else => {
                if let Some(frame) = self.frames.last_mut() {
                    frame.consequent_ends_reachable = true;
                    let next = frame.next;
                    self.ssa.add_pred(next, self.current);
                }
                self.enter_else();
            }
            Operator::End => {
                let (_, results) = need(arity)?;
                if let Some(frame) = self.frames.pop() {
                    self.ssa.add_pred(frame.next, self.current);
                    self.follow(&frame, results);
                    if frame.kind == Kind::Loop {
                        self.ssa.seal(frame.header);
                    }
                }
            }
            Operator::Br { relative_depth } => {
                let dest = self.branch(relative_depth);
                self.ssa.add_pred(dest, self.current);
                self.reachable = false;
            }
            Operator::BrIf { relative_depth } => {
                self.pop();
                let dest = self.branch(relative_depth);
                self.ssa.add_pred(dest, self.current);
                self.continue_alone();
            }
            Operator::BrTable { ref targets } => {
                self.pop();
                let mut dests = Vec::new();
                for depth in targets.targets().chain([Ok(targets.default())]) {
                    let dest = self.branch(depth.map_err(refuse)?);
                    if !dests.contains(&dest) {
                        dests.push(dest);
                    }
                }
                for dest in dests {
                    self.ssa.add_pred(dest, self.current);
                }
                self.reachable = false;
            }
            Operator::Return | Operator::Unreachable => self.reachable = false,
            Operator::LocalGet { local_index } => {
                let value = self.ssa.use_var(local_index, self.current);
                self.stack.push(value);
            }
            Operator::LocalSet { local_index } => {
                let value = self.pop();
                self.ssa.def_var(local_index, self.current, value);
            }
            Operator::LocalTee { local_index } => {
                let value = self.stack.last().copied().unwrap_or(OPAQUE);
                self.ssa.def_var(local_index, self.current, value);
            }
            Operator::I32Const { value } => {
                let value = self.ssa.value(Val::Const(i64::from(value.cast_unsigned())));
                self.stack.push(value);
            }
            Operator::I64Const { value } => {
                let value = self.ssa.value(Val::Const(value));
                self.stack.push(value);
            }
            Operator::GlobalGet { global_index } => {
                let value = match self.module.globals.get(global_index as usize) {
                    Some(&Some(imm)) => self.ssa.value(Val::Const(imm)),
                    _ => OPAQUE,
                };
                self.stack.push(value);
            }
            _ => self.step_other(op, offset, arity, constants)?,
        }
        Ok(())
    }

    /// Every other operator: its stack effect, wasmtime's own control flow
    /// for it, its constant length when it is size-charged.
    fn step_other(
        &mut self,
        op: &Operator<'_>,
        offset: u32,
        arity: Option<(u32, u32)>,
        constants: &mut Constants,
    ) -> Result<(), Refusal> {
        let length = self.stack.last().copied().unwrap_or(OPAQUE);
        let (params, results) = need(arity)?;
        self.pop_n(params);
        self.push_opaque(results);
        let mut size_charged = || {
            // `pre_translate_bulk_op`: a constant is folded, its immediate
            // read unsigned and clamped to i64; past 128, or not a constant,
            // the fuel is checked
            match self.ssa.constant(length) {
                Some(imm) => {
                    let cost = i64::try_from(imm.cast_unsigned()).unwrap_or(i64::MAX);
                    constants.insert(offset, cost);
                    if cost > SMALL_BULK {
                        self.fuel_check();
                    }
                }
                None => self.fuel_check(),
            }
        };
        match *op {
            Operator::MemoryCopy { .. }
            | Operator::MemoryFill { .. }
            | Operator::MemoryInit { .. } => {
                size_charged();
            }
            Operator::TableFill { .. } => {
                size_charged();
                self.fill_loop();
            }
            Operator::TableGrow { .. } => {
                size_charged();
                self.table_grow();
            }
            Operator::TableInit { .. } => {
                size_charged();
                self.per_element_copy(false);
            }
            Operator::TableCopy { .. } => {
                size_charged();
                self.per_element_copy(true);
            }
            // `table_get_funcref`, here and where call_indirect reads a table
            Operator::TableGet { .. } | Operator::CallIndirect { .. } => self.rejoin(),
            _ => {
                if let Some((at, size)) = access(op) {
                    // `bounds_check_field_access`: past the largest memory,
                    // an unconditional trap, and nothing after it is
                    // translated
                    let largest = self.module.memory_bytes.unwrap_or(u64::MAX);
                    if at.saturating_add(size) > largest {
                        self.reachable = false;
                    }
                }
            }
        }
        Ok(())
    }

    /// `translate_unreachable_operator`: only the control frames.
    fn step_unreachable(
        &mut self,
        op: &Operator<'_>,
        arity: Option<(u32, u32)>,
    ) -> Result<(), Refusal> {
        match op {
            Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                let base = self.stack.len();
                self.frames
                    .push(Frame::new(Kind::Block, false, base, ENTRY));
            }
            Operator::Else => {
                if let Some(frame) = self.frames.last_mut().filter(|frame| frame.live) {
                    frame.consequent_ends_reachable = false;
                    self.enter_else();
                }
            }
            Operator::End => {
                let Some(frame) = self.frames.pop().filter(|frame| frame.live) else {
                    return Ok(());
                };
                if frame.kind == Kind::Loop {
                    self.ssa.seal(frame.header);
                }
                let anyway = match frame.kind {
                    Kind::If if frame.else_seen => frame.consequent_ends_reachable,
                    Kind::If => true,
                    Kind::Block | Kind::Loop => false,
                };
                if frame.branched_to || anyway {
                    let (_, results) = need(arity)?;
                    self.follow(&frame, results);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// The function type at `ty` in the module's type space.
fn function_type(func: &FuncValidator<ValidatorResources>, ty: u32) -> Option<&FuncType> {
    match &func.resources().sub_type_at(ty)?.composite_type.inner {
        CompositeInnerType::Func(ty) => Some(ty),
        _ => None,
    }
}

/// Whether an `if` of `blockty` has the same parameter and result types:
/// then wasmtime allocates its `else` block only at the `else`.
fn params_equal_results(
    func: &FuncValidator<ValidatorResources>,
    blockty: BlockType,
) -> Result<bool, Refusal> {
    Ok(match blockty {
        BlockType::Empty => true,
        BlockType::Type(_) => false,
        BlockType::FuncType(ty) => {
            let ty = function_type(func, ty)
                .ok_or_else(|| Refusal::Unsupported("a block without a type".to_owned()))?;
            ty.params() == ty.results()
        }
    })
}
