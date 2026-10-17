#!/usr/bin/env bash
# Lane H PR2 of the wasm-mobile build (SPEC §4.1, the entities row): the
# session entities every shell draws from leave `src/shell/entities/` for
# `crates/host/src/session/`, as `git mv` only. Stays desktop: `Windows`,
# `Desk`, `Front`, `DotSlot`, `Overlays`, `Spotlight` (one OS window's own)
# and `account_tests.rs` (it drives the desktop's windows). Run on any base;
# path and `use` fixes are the next commit.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
mv_() { git mv "$1" "$2"; echo "$1 -> $2"; }
mkdir -p crates/host/src/session
for f in account chain notifications prefs rail screen seats slice toast; do
    mv_ "src/shell/entities/$f.rs" "crates/host/src/session/$f.rs"
done
# `Session` is the node in hand; its file is `node.rs` in the `session`
# module (a `session/session.rs` is clippy's module inception)
mv_ src/shell/entities/session.rs crates/host/src/session/node.rs
mv_ src/shell/entities/session_tests.rs crates/host/src/session/node_tests.rs
