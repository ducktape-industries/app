//! pushd is its own compile child (`ducktape_host::runner::COMPILE_APART`),
//! as the desktop app is: a notifier's code compiles in it, read out of the
//! file it lies in (a section of its program's blob in the cache), into the
//! cache pushd then reads, and the compile's memory leaves with the child.

use ducktape_host::runner::COMPILE_APART;
use std::path::Path;
use std::process::{Command, Output};

/// The child, asked for the `len` bytes of `blob` from `offset` on that its
/// parent checked as `sha256`.
fn child(cache: &Path, blob: &Path, offset: usize, len: usize, sha256: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pushd"))
        .arg(COMPILE_APART)
        .arg("1")
        .arg(cache)
        .arg(blob)
        .args([&offset.to_string(), &len.to_string(), sha256])
        .output()
        .unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    noded_client::hex::hex_encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

#[test]
fn pushd_compiles_a_notifier_apart_into_the_cache() {
    let cache = tempfile::tempdir().unwrap();
    let code = b"\0asm\x01\0\0\0";
    let blob = cache.path().join("blob");
    std::fs::write(&blob, [&b"program 8\0"[..], code, b"after"].concat()).unwrap();
    let compile = || {
        let output = child(cache.path(), &blob, 10, code.len(), &sha256(code));
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    assert_eq!(compile(), "cold");
    assert_eq!(compile(), "disk", "the second compile reads the first");
}

/// The child compiles only the bytes its parent checked. A file rewritten
/// in place after the parent read it (a developer's view rebuilt mid-load)
/// is refused with exit 2, naming the file, the range and both hashes, and
/// nothing is compiled, even when the new bytes would compile: exit 1 stays
/// "the parent's code does not compile", and no compile lands under a hash
/// the parent never asked for.
#[test]
fn pushd_refuses_code_its_parent_did_not_check() {
    let cache = tempfile::tempdir().unwrap();
    let (checked, there) = (b"not wasm", b"\0asm\x01\0\0\0");
    let blob = cache.path().join("blob");
    std::fs::write(&blob, [&b"program 8\0"[..], there, b"after"].concat()).unwrap();
    let output = child(cache.path(), &blob, 10, checked.len(), &sha256(checked));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let error = String::from_utf8(output.stderr).unwrap();
    let file = blob.display().to_string();
    for named in [
        &file,
        "8 bytes from 10 on",
        &sha256(checked),
        &sha256(there),
    ] {
        assert!(error.contains(named), "{error:?} names {named}");
    }
    let compiled = std::fs::read_dir(cache.path())
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "cwasm"))
        .count();
    assert_eq!(compiled, 0, "nothing compiled");
}
