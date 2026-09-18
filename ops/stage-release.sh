#!/usr/bin/env bash
# Lay out a Linux app release where core's ops/release/archive.sh --from
# takes it:
#   <out-dir>/{ducktape-launcher, ducktape-app}
# A release is the launcher and the app, nothing else: every view the app
# draws comes off the connected node's registry.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
(($# == 1)) || { echo "usage: ops/stage-release.sh <out-dir>" >&2; exit 2; }
out=$1
[[ ! -e "$out" || -z "$(ls -A "$out")" ]] ||
  { echo "$out is not empty; stage a release into a new or empty directory" >&2; exit 1; }
mkdir -p "$out"
out=$(cd "$out" && pwd -P)
cd "$repo"
"${CARGO:-cargo}" build --locked --release -p ducktape-app -p app-launcher
release_bin="${CARGO_TARGET_DIR:-$repo/target}/release"
install -m 0755 "$release_bin/ducktape-launcher" "$release_bin/ducktape-app" "$out/"
echo "staged $out"
