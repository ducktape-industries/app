#!/usr/bin/env bash
# Lay out a Linux app release where core's ops/release/archive.sh --from
# takes it:
#   <out-dir>/{ducktape-launcher, ducktape-app, views/*_view.wasm, views/VIEWS_REV}
# The views are the ones ops/views.rev pins (ops/views-pin-check.sh).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
(($# == 1)) || { echo "usage: ops/stage-release.sh <out-dir>" >&2; exit 2; }
out=$1
[[ ! -e "$out" || -z "$(ls -A "$out")" ]] ||
  { echo "$out is not empty; stage a release into a new or empty directory" >&2; exit 1; }
# shellcheck source=ops/views-pin-check.sh
source "$repo/ops/views-pin-check.sh"
pinned_views "$repo"
mkdir -p "$out/views"
out=$(cd "$out" && pwd -P)
cd "$repo"
"${CARGO:-cargo}" build --locked --release -p ducktape-app -p app-launcher
release_bin="${CARGO_TARGET_DIR:-$repo/target}/release"
install -m 0755 "$release_bin/ducktape-launcher" "$release_bin/ducktape-app" "$out/"
install -m 0644 "$views"/*_view.wasm "$out/views/"
printf '%s\n' "$views_rev" >"$out/views/VIEWS_REV"
echo "staged $out: views $views_rev"
