//! Stamp this checkout's identity into the binary for `ducktape-app --version`,
//! and its commit's committer time for the self-update (`src/update.rs`): the
//! sequence a release is published at, so an installed build pins its own.
//!
//! The same stamp `ducktape --version` carries (`crates/noded/build.rs` in
//! ducktape-industries/ducktape), so a walk report compares the app's build
//! with the node's like for like: `<short sha>`, plus a working-tree digest
//! when the tree is dirty.
//!
//! Git absent (a source tarball, a vendored build) is not an error: the env
//! var is left unset and the app reports its build as `unknown`.

use std::process::Command;

fn main() {
    for path in ["HEAD", "index"] {
        if let Some(resolved) = git_stdout(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
    if let Some(top) = git_stdout(&["rev-parse", "--show-toplevel"]) {
        println!("cargo:rerun-if-changed={top}/crates/view-meter/src");
    }

    if let Some(build) = commit_with_worktree_digest() {
        println!("cargo:rustc-env=DUCKTAPE_APP_BUILD={build}");
    }
    if let Some(time) = git_stdout(&["log", "-1", "--format=%ct", "HEAD"]) {
        println!("cargo:rustc-env=DUCKTAPE_APP_COMMIT_TIME={time}");
    }

    if std::env::var_os("CARGO_FEATURE_WEB").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
    {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }

    if std::env::var_os("CARGO_FEATURE_AV").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")
    {
        let plist =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/dev-Info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!(
            "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}

fn commit_with_worktree_digest() -> Option<String> {
    let commit = git_stdout(&["rev-parse", "--short", "HEAD"])?;
    let tracked_worktree_diff = git_stdout(&["diff", "HEAD"]).unwrap_or_default();
    if tracked_worktree_diff.is_empty() {
        return Some(commit);
    }
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tracked_worktree_diff.hash(&mut hasher);
    Some(format!("{commit}-{:x}", hasher.finish()))
}

fn git_stdout(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}
