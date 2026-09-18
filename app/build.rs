//! Stamp this checkout's identity into the binary for `ducktape-app --version`.
//!
//! The same stamp `ducktape --version` carries (`crates/noded/build.rs` in
//! ducktape-industries/ducktape), so a walk report compares the app's build
//! with the node's like for like: `<short sha>`, plus a working-tree digest
//! when the tree is dirty. It has to be stamped HERE: the `node` crate this
//! app links carries the core pin's build, not this checkout's.
//!
//! Git absent (a source tarball, a vendored build) is not an error: the env
//! var is left unset and the app reports its build as `unknown`.
//!
//! And record WHICH core checkout the linked `noded` is compiled from, so the
//! sim tests boot the simulation set that checkout's build script staged
//! (`backend::tests::sim_modules_dir`) and not whichever set the shared
//! `.staged-modules` pointer names at the moment a test boots.

use std::process::Command;

fn main() {
    // re-run when HEAD moves. `--git-path` resolves correctly inside a git
    // worktree, where `.git` is a file pointing elsewhere.
    for path in ["HEAD", "index"] {
        if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
    // a core pin bump moves noded to another checkout.
    println!("cargo:rerun-if-changed=../Cargo.lock");

    if let Some(build) = build_id() {
        println!("cargo:rustc-env=DUCKTAPE_APP_BUILD={build}");
    }

    // Missing is not a build error: only the sim tests read it, and they fail
    // naming this warning's reason when it is not there.
    match noded_dir() {
        Ok(dir) => println!("cargo:rustc-env=DUCKTAPE_CORE_NODED_DIR={dir}"),
        Err(reason) => println!("cargo:warning=no core noded checkout recorded: {reason}"),
    }
}

/// The directory of the `noded` package this build links, as cargo resolved it
/// for this lockfile — a git pin's checkout under `$CARGO_HOME/git/checkouts`.
/// A git dependency's source is keyed by its revision, so unlike a core
/// checkout's own build, this path cannot be shared with another revision.
fn noded_dir() -> Result<String, String> {
    let var = |name: &str| std::env::var(name).map_err(|error| format!("{name}: {error}"));
    let out = Command::new(var("CARGO")?)
        .args(["metadata", "--format-version", "1", "--offline", "--locked"])
        .args(["--filter-platform", &var("TARGET")?, "--manifest-path"])
        .arg(format!("{}/Cargo.toml", var("CARGO_MANIFEST_DIR")?))
        .output()
        .map_err(|error| format!("cargo metadata: {error}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // no JSON parser without a build-dependency; a manifest path is a plain
    // string field, and only one package's ends in `crates/noded/Cargo.toml`.
    let metadata = String::from_utf8_lossy(&out.stdout);
    let noded: Vec<&str> = metadata
        .split("\"manifest_path\":\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter_map(|path| path.strip_suffix("/crates/noded/Cargo.toml"))
        .collect();
    match noded[..] {
        [checkout] => Ok(format!("{checkout}/crates/noded")),
        _ => Err(format!(
            "cargo metadata names {} noded packages",
            noded.len()
        )),
    }
}

/// `<short sha>`, or `<short sha>-<digest>` when the working tree differs from
/// it: two different uncommitted trees at one commit must not read as the same
/// build, so the diff is digested rather than marked `-dirty`.
fn build_id() -> Option<String> {
    let commit = git(&["rev-parse", "--short", "HEAD"])?;
    // tracked changes only: untracked scratch files are not part of the build.
    let diff = git(&["diff", "HEAD"]).unwrap_or_default();
    if diff.is_empty() {
        return Some(commit);
    }
    // display only, so `DefaultHasher` not being stable across toolchains
    // costs nothing, and stdlib means no build-dependency for one hash.
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    diff.hash(&mut hasher);
    Some(format!("{commit}-{:x}", hasher.finish()))
}

/// Run one git command, returning its trimmed stdout. `None` for any failure —
/// git missing, not a repository, or a non-zero exit.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}
