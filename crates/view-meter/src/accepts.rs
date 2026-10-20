//! The rule every view runner applies before it compiles a view
//! ([`crate::accepts`]): the module validates against [`FEATURES`], has none
//! of the shapes the pass does not take, and stays inside wasm's limits once
//! the pass has added its own code. Whatever passes here [`crate::meter`]
//! meters.
//!
//! The limits are wasmparser's (`limits.rs`), the ones wasmtime validates
//! with. Per module the pass adds one global, one export, at most one
//! element segment, a function per imported function, per exported function
//! and one more, and at most two types per type; per function, one
//! parameter, one local, and code whose size is bounded here operator by
//! operator ([`added`]). The bound is what is checked, not the pass's actual
//! output: a view is refused before the pass would reach a limit, and
//! nothing is metered to find out. How far before depends on the body: the
//! bound counts each operator's most, so a loop-heavy body is refused at up
//! to about three times the size the pass would actually make of it.
use wasmparser::{
    BinaryReaderError, BlockType, CompositeInnerType, ExternalKind, FuncValidator,
    FuncValidatorAllocations, FunctionBody, Operator, Parser, Payload, RefType, TypeRef, ValType,
    ValidPayload, Validator, ValidatorResources, WasmModuleResources,
};

use crate::{FEATURES, GAS, Refusal};

const WASMPARSER_MAX_TYPES: usize = 1_000_000;
const WASMPARSER_MAX_FUNCTIONS: usize = 1_000_000;
const WASMPARSER_MAX_GLOBALS: usize = 1_000_000;
const WASMPARSER_MAX_EXPORTS: usize = 1_000_000;
const WASMPARSER_MAX_ELEMENT_SEGMENTS: usize = 100_000;
const WASMPARSER_MAX_PARAMS_OR_RESULTS: usize = 1_000;
const WASMPARSER_MAX_LOCALS: u64 = 50_000;
const WASMPARSER_MAX_FUNCTION_BODY_BYTES: u64 = 7_654_321;

pub(crate) fn check(code: &[u8]) -> Result<(), Refusal> {
    let mut validator = Validator::new_with_features(FEATURES);
    let mut counts = Counts::default();
    let mut allocations = FuncValidatorAllocations::default();
    for payload in Parser::new(0).parse_all(code) {
        let payload = payload.map_err(refuse)?;
        let valid = validator.payload(&payload).map_err(refuse)?;
        counts.read(&payload)?;
        if let ValidPayload::Func(func, body) = valid {
            let mut func = func.into_validator(std::mem::take(&mut allocations));
            body_fits(&mut func, &body)?;
            allocations = func.into_allocations();
        }
    }
    counts.fit()
}

pub(crate) fn refuse(error: BinaryReaderError) -> Refusal {
    Refusal::Unsupported(error.to_string())
}

fn refused(what: impl Into<String>) -> Result<(), Refusal> {
    Err(Refusal::Unsupported(what.into()))
}

#[derive(Default)]
struct Counts {
    types: usize,
    imported_functions: usize,
    defined_functions: usize,
    exported_functions: usize,
    globals: usize,
    exports: usize,
    element_segments: usize,
}

impl Counts {
    fn read(&mut self, payload: &Payload<'_>) -> Result<(), Refusal> {
        let externref = "an externref table or global (its barriers are control flow the pass \
                         does not replay)";
        match payload {
            Payload::TypeSection(types) => {
                for ty in types.clone().into_iter_err_on_gc_types() {
                    let ty = ty.map_err(refuse)?;
                    if ty.params().len().max(ty.results().len()) >= WASMPARSER_MAX_PARAMS_OR_RESULTS
                    {
                        return refused(format!(
                            "a function type at the {WASMPARSER_MAX_PARAMS_OR_RESULTS}-parameter or \
                             -result limit (the count would take it past)"
                        ));
                    }
                    self.types += 1;
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
                        TypeRef::Global(_) => self.globals += 1,
                        _ => self.imported_functions += 1,
                    }
                }
            }
            Payload::FunctionSection(functions) => {
                self.defined_functions += functions.count() as usize;
            }
            Payload::TableSection(tables) => {
                for table in tables.clone() {
                    if table.map_err(refuse)?.ty.element_type != RefType::FUNCREF {
                        return refused(externref);
                    }
                }
            }
            Payload::GlobalSection(globals) => {
                for global in globals.clone() {
                    if is_externref(global.map_err(refuse)?.ty.content_type) {
                        return refused(externref);
                    }
                    self.globals += 1;
                }
            }
            Payload::ExportSection(exports) => {
                for export in exports.clone() {
                    let export = export.map_err(refuse)?;
                    if export.name == GAS {
                        return refused(format!("the module already exports {GAS}"));
                    }
                    self.exports += 1;
                    let own = export.index as usize >= self.imported_functions;
                    if export.kind == ExternalKind::Func && own {
                        self.exported_functions += 1;
                    }
                }
            }
            Payload::StartSection { .. } => {
                return refused("a start function (it would run before the fuel is set)");
            }
            Payload::ElementSection(elements) => {
                self.element_segments += elements.count() as usize;
            }
            _ => {}
        }
        Ok(())
    }

    fn fit(&self) -> Result<(), Refusal> {
        let limits = [
            (3 * self.types + 1, WASMPARSER_MAX_TYPES, "types"),
            (
                2 * self.imported_functions + self.defined_functions + self.exported_functions + 1,
                WASMPARSER_MAX_FUNCTIONS,
                "functions",
            ),
            (self.globals + 1, WASMPARSER_MAX_GLOBALS, "globals"),
            (self.exports + 1, WASMPARSER_MAX_EXPORTS, "exports"),
            (
                self.element_segments + 1,
                WASMPARSER_MAX_ELEMENT_SEGMENTS,
                "element segments",
            ),
        ];
        for (needs, limit, what) in limits {
            if needs > limit {
                return refused(format!(
                    "the metered module could need {needs} {what}, past wasm's {limit}"
                ));
            }
        }
        Ok(())
    }
}

fn is_externref(ty: ValType) -> bool {
    matches!(ty, ValType::Ref(element) if element != RefType::FUNCREF)
}

fn body_fits(
    func: &mut FuncValidator<ValidatorResources>,
    body: &FunctionBody<'_>,
) -> Result<(), Refusal> {
    let resources = func.resources();
    let params = resources
        .type_index_of_function(func.index())
        .and_then(|ty| resources.sub_type_at(ty))
        .map_or(0, |ty| match &ty.composite_type.inner {
            CompositeInnerType::Func(ty) => ty.params().len(),
            _ => 0,
        });
    let mut locals = params as u64;
    let mut reader = body.get_locals_reader().map_err(refuse)?;
    for _ in 0..reader.get_count() {
        let offset = reader.original_position();
        let (count, ty) = reader.read().map_err(refuse)?;
        func.define_locals(offset, count, ty).map_err(refuse)?;
        locals += u64::from(count);
    }
    if locals + 2 > WASMPARSER_MAX_LOCALS {
        return refused(format!(
            "a function with {locals} locals and parameters (the pass adds 2; wasm allows \
             {WASMPARSER_MAX_LOCALS})"
        ));
    }
    let mut size = body.range().len() as u64 + MAX_ADDED_BYTES_PER_FUNCTION;
    let mut operators = body.get_operators_reader().map_err(refuse)?;
    while !operators.eof() {
        let (op, offset) = operators.read_with_offset().map_err(refuse)?;
        func.op(offset, &op).map_err(refuse)?;
        size += added(&op);
    }
    operators.finish().map_err(refuse)?;
    if size > WASMPARSER_MAX_FUNCTION_BODY_BYTES {
        return refused(format!(
            "a function whose metered body could reach {size} bytes (wasm allows {WASMPARSER_MAX_FUNCTION_BODY_BYTES})"
        ));
    }
    Ok(())
}

const LOCAL: u64 = 4;
const GLOBAL: u64 = 4;
const CALL: u64 = 5;
const I64: u64 = 11;
const CHARGE: u64 = LOCAL + I64 + 1 + LOCAL;
const CHECK: u64 = LOCAL + 2 + 1 + 2 + LOCAL + 2 + 1 + GLOBAL + 1 + 1 + 1;
const CHARGE_LEN: u64 = 4 * LOCAL + 2 + CHECK;
const ASK_BUDGET: u64 = LOCAL + 1 + I64 + 1 + CALL;
const MAX_REINDEX_ADDED_BYTES: u64 = 3;
const MAX_ADDED_BYTES_PER_FUNCTION: u64 =
    CHARGE + CHECK + CHARGE + (1 + 5 + 1) + LOCAL + 6 * (3 + 1) + 3;

fn added(op: &Operator<'_>) -> u64 {
    use Operator as O;
    let typed = |blockty: &BlockType| match blockty {
        BlockType::FuncType(_) => MAX_REINDEX_ADDED_BYTES,
        _ => 0,
    };
    match op {
        O::MemoryCopy { .. } | O::MemoryFill { .. } | O::TableCopy { .. } | O::TableFill { .. } => {
            CHARGE + CHARGE_LEN
        }
        O::MemoryInit { .. } | O::TableInit { .. } => CHARGE + CHARGE_LEN + MAX_REINDEX_ADDED_BYTES,
        O::TableGrow { .. } => CHARGE + CHARGE_LEN + ASK_BUDGET,
        O::MemoryGrow { .. } => LOCAL + ASK_BUDGET,
        O::Block { blockty } => CHARGE + typed(blockty),
        O::Loop { blockty } => CHARGE + CHECK + CHARGE + typed(blockty),
        O::If { blockty } => 3 * CHARGE + 1 + typed(blockty),
        O::Br { .. } | O::BrIf { .. } | O::BrTable { .. } | O::Unreachable => CHARGE,
        O::Call { .. } => CHARGE + 2 * LOCAL + MAX_REINDEX_ADDED_BYTES,
        O::CallIndirect { .. } => CHARGE + 4 * LOCAL + MAX_REINDEX_ADDED_BYTES,
        O::Return => CHARGE + LOCAL,
        O::LocalGet { .. }
        | O::LocalSet { .. }
        | O::LocalTee { .. }
        | O::GlobalGet { .. }
        | O::GlobalSet { .. }
        | O::RefFunc { .. }
        | O::DataDrop { .. }
        | O::ElemDrop { .. } => MAX_REINDEX_ADDED_BYTES,
        _ => 0,
    }
}
