#!/usr/bin/env bash
# Stage the desktop's views at the ducktape-views revision ops/views.rev pins
# into target/views, where views_dir() finds them for a checkout build.
#
#   ops/stage-views.sh             a dev run (make views)
#   ops/stage-views.sh --release   what a release ships (ops/views-pin-check.sh)
#
# A cache clone of ducktape-views lives in target/views-src. The set is built
# by THAT revision's own ops/build-views.sh — every view package in one build,
# the set its repro check covers — and target/views/VIEWS_REV names the pin.
# A target/views whose VIEWS_REV is not the pin, or that has none, is rebuilt,
# never trusted.
#
# wasm-tools writes each component, so its version is part of the bytes:
# ops/views.wasm-tools records the one the pinned set was built with. Another
# version is refused for a release and warned about for a dev run, whose
# VIEWS_REV then names that version instead of claiming the pin.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
case "${1:-}" in
  '') release=0 ;;
  --release) release=1 ;;
  *) echo "usage: ops/stage-views.sh [--release]" >&2; exit 2 ;;
esac
pin=$(<"$repo/ops/views.rev")
tools_pin=$(<"$repo/ops/views.wasm-tools")
url=https://github.com/ducktape-industries/ducktape-views
src=$repo/target/views-src
staged=$repo/target/views

# app/src/backend/view_source.rs DESKTOP_OWNED: the views the app ships.
desktop_owned_staged() {
  local view
  for view in members agents node explorer settings palette; do
    [[ -f "$staged/${view}_view.wasm" ]] || { missing=${view}_view.wasm; return 1; }
  done
}
if [[ "$(cat "$staged/VIEWS_REV" 2>/dev/null)" == "$pin" ]] && desktop_owned_staged; then
  echo "views ${pin:0:7} already staged"
  exit 0
fi

install_tools="cargo install --locked wasm-tools@$tools_pin"
read -r _ tools _ < <(wasm-tools --version 2>/dev/null) ||
  { echo "wasm-tools is not on PATH; the views build needs it: $install_tools" >&2; exit 1; }
views_rev=$pin
if [[ "$tools" != "$tools_pin" ]]; then
  mismatch="wasm-tools $tools is not the $tools_pin views ${pin:0:7} was built with (ops/views.wasm-tools), so the views' bytes would not match the shipped set"
  if ((release)); then
    echo "$mismatch: $install_tools" >&2
    exit 1
  fi
  echo "WARNING: $mismatch; staging them for this dev run only" >&2
  views_rev="$pin wasm-tools $tools"
fi

[[ -d "$src/.git" ]] || git clone --quiet "$url" "$src"
git -C "$src" cat-file -e "$pin^{commit}" 2>/dev/null || git -C "$src" fetch --quiet origin || :
git -C "$src" cat-file -e "$pin^{commit}" 2>/dev/null ||
  { echo "views $pin (ops/views.rev) is not fetchable from $url" >&2; exit 1; }
git -C "$src" -c advice.detachedHead=false checkout --quiet --detach "$pin"
git -C "$src" diff --quiet HEAD ||
  { echo "$src has local changes, so it would not build views $pin; restore it" >&2; exit 1; }

# Every view package at the checkout root, as the views repo's own
# ops/views-repro-check.sh selects them.
packages=$(awk '/^\[/{ in_package = ($0 == "[package]") } in_package && /^name *= *"/ { split($0, part, "\""); printf "-p %s ", part[2] }' "$src"/*/Cargo.toml)
rm -rf "$src/target/views"
# build-views.sh waits on /var/tmp/ducktape-view-root.lock while another
# build holds it.
# shellcheck disable=SC2086 # one word per -p and package name
(cd "$src" && CARGO_TARGET_DIR="$repo/target/views-build" bash ops/build-views.sh $packages)

rm -rf "$staged"
mkdir -p "$staged"
cp "$src"/target/views/*_view.wasm "$staged/"
desktop_owned_staged || { echo "views $pin built no $missing" >&2; exit 1; }
# Written last: a stage cut short leaves no VIEWS_REV, so the next run rebuilds.
printf '%s\n' "$views_rev" >"$staged/VIEWS_REV"
echo "staged views ${pin:0:7} into target/views"
