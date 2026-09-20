#!/usr/bin/env bash
# Lay out a Linux app release where core's ops/release/archive.sh --from
# takes it:
#   <out-dir>/{ducktape-launcher, ducktape-app}
# A release is the launcher and the app, nothing else: every view the app
# draws comes off the connected node's registry.
#
# The recipe, not the caller, decides the bytes: two stagings of one commit
# are byte-identical wherever their target dirs are (ops/verify-release.sh
# proves it). So no compiler cache (an sccache hit is some other build's
# output, compiled before or after the toolchain gained rust-src), build
# scripts that stamp a date (mozjpeg-sys) get the commit's, and two paths are
# remapped out of panic locations: the target dir (cranelift-codegen and
# html5ever compile code generated into OUT_DIR) and an installed rust-src,
# which rustc otherwise names instead of /rustc/<commit> for std code inlined
# here. A path's length moves every symbol after it.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
(($# == 1)) || { echo "usage: ops/stage-release.sh <out-dir>" >&2; exit 2; }
out=$1
[[ ! -e "$out" || -z "$(ls -A "$out")" ]] ||
  { echo "$out is not empty; stage a release into a new or empty directory" >&2; exit 1; }
mkdir -p "$out"
out=$(cd "$out" && pwd -P)
cd "$repo"
cargo=${CARGO:-cargo}
export RUSTC_WRAPPER= SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)
# Where cargo builds: CARGO_TARGET_DIR, else a config's build.target-dir.
target=$("$cargo" metadata --locked --no-deps --format-version 1 |
  python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
rust_src="$(rustc --print sysroot)/lib/rustlib/src/rust"
rustc_commit=$(rustc -vV | sed -n 's/^commit-hash: //p')
# A key of its own: `target.<cfg>.rustflags` tables join, so the repo's
# tokio_unstable and a user's linker flags stay.
"$cargo" build --locked --release -p ducktape-app -p app-launcher --config \
  "target.'cfg(all())'.rustflags = [\"--remap-path-prefix=$target=/target\", \"--remap-path-prefix=$rust_src=/rustc/$rustc_commit\"]"
install -m 0755 "$target/release/ducktape-launcher" "$target/release/ducktape-app" "$out/"
echo "staged $out"
