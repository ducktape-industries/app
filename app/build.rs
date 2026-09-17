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

use std::process::Command;

fn main() {
    // re-run when HEAD moves. `--git-path` resolves correctly inside a git
    // worktree, where `.git` is a file pointing elsewhere.
    for path in ["HEAD", "index"] {
        if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }

    if let Some(build) = build_id() {
        println!("cargo:rustc-env=DUCKTAPE_APP_BUILD={build}");
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
