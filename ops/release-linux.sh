#!/usr/bin/env bash
# A release's Linux build, `ops/release-linux.sh <out>`: plan.json for the commit at HEAD (its sequence, version, display and publish time), then the dist build with the web view, packed as the update archive.
# The release workflows run it in a pods check; the signer's release-manifest reads plan.json.
set -euo pipefail
out=${1:?the folder the check keeps}
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo"
sha=$(git rev-parse HEAD)
sequence=$(git show -s --format=%ct HEAD)
if [[ "$sequence" -gt $(($(date +%s) + 600)) ]]; then
  echo "$sha is dated $sequence, in the future: not published" >&2
  exit 1
fi
version=$(awk '/^\[workspace.package\]/{ package=1; next } /^\[/{ package=0 } package && /^version *=/{ gsub(/"/, "", $3); print $3; exit }' Cargo.toml)
[[ -n "$version" ]] || { echo "workspace package version missing" >&2; exit 1; }
cargo build --locked --profile dist -p ducktape-app --features web
dist="${CARGO_TARGET_DIR:-$repo/target}/dist"
files=(ducktape-app libcef.so libvk_swiftshader.so libvulkan.so.1 vk_swiftshader_icd.json
  icudtl.dat v8_context_snapshot.bin resources.pak chrome_100_percent.pak
  chrome_200_percent.pak locales CREDITS.html)
strip --strip-unneeded "$dist/libcef.so" "$dist/libvk_swiftshader.so"
printf '%s\n' "${files[@]}" > "$dist/release.files"
mkdir -p "$out"
tar czf "$out/ducktape-app-x86_64-unknown-linux-gnu.tar.gz" -C "$dist" "${files[@]}" release.files
python3 - "$out/plan.json" "$sha" "$sequence" "$version" <<'PY'
import json, sys, time
path, sha, sequence, version = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
plan = {
    "commit": sha,
    "sequence": sequence,
    "version": version,
    "display": f"{version}+{sha[:7]}",
    "published_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(sequence)),
}
with open(path, "w") as file:
    json.dump(plan, file, indent=2)
    file.write("\n")
PY
