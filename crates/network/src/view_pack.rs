//! Packing, as a founder does it: a program's view and its describe module ride in the program's
//! code as custom sections. A copy of qa's `crates/view-pack/src/lib.rs` (qa keeps its own for kit),
//! less `strip`, which nothing here calls.
use std::borrow::Cow;

use wasm_encoder::{CustomSection, Encode, Section};
use wasmparser::{Encoding, Parser, Payload, Validator};

pub const VIEW_SECTION: &str = "ducktape.view";

fn validate(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(
        Parser::new(0).parse_all(bytes).next(),
        Some(Ok(Payload::Version {
            encoding: Encoding::Module,
            ..
        }))
    ) {
        return Err("expected a core WebAssembly module".into());
    }
    Validator::new().validate_all(bytes)?;
    Ok(())
}

/// What a program's describe module rides in (modules `crates/sdk/describe`).
pub const DESCRIBE_SECTION: &str = "ducktape.describe";

/// Remove every custom section named `name`, preserving all other bytes.
pub fn strip_section(program: &[u8], name: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    validate(program)?;
    let mut output = Vec::with_capacity(program.len());
    let mut keep = 0;
    let mut section_start = 8;
    for payload in Parser::new(0).parse_all(program) {
        let payload = payload?;
        if let Some((_, range)) = payload.as_section() {
            if matches!(&payload, Payload::CustomSection(s) if s.name() == name) {
                output.extend_from_slice(&program[keep..section_start]);
                keep = range.end;
            }
            section_start = range.end;
        }
    }
    output.extend_from_slice(&program[keep..]);
    Ok(output)
}

/// The payload is the complete view module, without framing or transformation.
pub fn embed(program: &[u8], view: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    validate(view)?;
    embed_section(program, VIEW_SECTION, view)
}

/// The program's describe module, as its `ducktape.describe` section. The
/// module must be a core module that imports nothing: a host runs it with
/// none.
pub fn embed_describe(
    program: &[u8],
    module: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    validate(module)?;
    for payload in Parser::new(0).parse_all(module) {
        if let Payload::ImportSection(imports) = payload?
            && imports.count() > 0
        {
            return Err("a describe module imports nothing".into());
        }
    }
    embed_section(program, DESCRIBE_SECTION, module)
}

/// `payload` as the one custom section `name` at the end of `program`.
fn embed_section(
    program: &[u8],
    name: &str,
    payload: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut output = strip_section(program, name)?;
    let section = CustomSection {
        name: Cow::Borrowed(name),
        data: Cow::Borrowed(payload),
    };
    output.push(section.id());
    section.encode(&mut output);
    Ok(output)
}
