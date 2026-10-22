#!/usr/bin/env bash
# A release's macOS build, `ops/release-macos.sh <out>`: the dist bundle, ad-hoc signed, packed as the unsigned archive, beside this commit's entitlements for the signer's developer-id to sign with.
# The release workflows run it on the mac relay; nothing here holds a key.
set -euo pipefail
out=${1:?the folder the check keeps}
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo"
tools=$("$repo/ops/cef-tools-macos.sh" "$repo/target/cef-tools")
export PATH="${tools//$'\n'/:}:$PATH"
DUCKTAPE_CARGO_PROFILE=dist "$repo/ops/bundle-app-macos.sh"
mkdir -p "$out"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$out/ducktape-app-unsigned.app.tar.gz" \
  -C "$repo/target/app-bundle" Ducktape.app
cp "$repo/packaging/entitlements.plist" "$repo/packaging/helper-entitlements.plist" "$out/"
