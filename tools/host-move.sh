#!/usr/bin/env bash
# The `crates/host` cut, as `git mv` only (SPEC wasm-mobile §4.1): what the
# desktop shell and the phone shell share leaves `src/` for `crates/host/src/`.
# Replayable: run it on any base, commit the result as the move commit, then
# fix paths and `use`s in a separate commit. It edits no file's contents.
#
# What stays in `src/`: the desktop shell (`shell/` with its entities,
# `ui/`, `tray.rs`, `update/`, `main.rs`), the desktop-only backend
# (`backend/{app_dirs,auth_page,device_key,invite,key_dir,loopback,passkey}.rs`),
# the desktop's OS services (`os.rs`, `os/`) and `crates/{web,updater}`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

mv_() {
    mkdir -p "$(dirname "$2")"
    git mv "$1" "$2"
}

# the renderer, whole (`render/web.rs` is `impl ViewTree`: it goes with it,
# behind host's `web` feature)
mv_ src/render.rs crates/host/src/render.rs
mv_ src/render crates/host/src/render

# the runtime, whole, then its two OS banner impls back out: they are the
# desktop's `Banners` (`src/os.rs`)
mv_ src/runtime.rs crates/host/src/runtime.rs
mv_ src/runtime crates/host/src/runtime
mv_ crates/host/src/runtime/notify/freedesktop.rs src/os/freedesktop.rs
mv_ crates/host/src/runtime/notify/macos.rs src/os/macos.rs

# the backend's shared half (`app_dirs.rs` is the desktop's platform rule,
# `invite.rs` mints with the node this machine runs: both stay)
for f in mod endpoints identity join prefs session views; do
    mv_ "src/backend/$f.rs" "crates/host/src/backend/$f.rs"
done
# `views.rs`'s shattered-blob fixtures go with it
mv_ src/backend/fixtures crates/host/src/backend/fixtures

# the AX door (feature `ax-door`)
mv_ src/ax.rs crates/host/src/ax.rs
mv_ src/ax crates/host/src/ax

# fonts, a11y, perf, and the product theme (its traffic-light inset stays
# in `src/shell/`, re-added by the fix commit)
mv_ src/fonts.rs crates/host/src/fonts.rs
mv_ src/tests/font_fallback.rs crates/host/src/tests/font_fallback.rs
mv_ src/a11y.rs crates/host/src/a11y.rs
mv_ src/perf.rs crates/host/src/perf.rs
mv_ src/shell/theme.rs crates/host/src/theme.rs

git status --short | grep -c '^R' | sed 's/$/ files moved/'
