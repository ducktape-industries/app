#!/usr/bin/env bash
# The signer's release-manifest tool: `release-manifest.sh <inputs> <out>` signs the channel file over the linux and macos-sign archives, and writes the release record, the publish.json the signer tags and files with, and publish-release's upload order for r2-mirror.
# It builds publish-release from the signer's pinned checkout before any key is read, and reads its key from the keychain (ops/signer/README.md).
set -euo pipefail
inputs=${1:?the inputs folder the signer hands over}
out=${2:?the folder the signer keeps}
channel=${SIGNER_ENV_CHANNEL:?the job names its channel in env CHANNEL}
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
keychain=()
if [[ -n "${SIGNER_KEYCHAIN:-}" ]]; then keychain=("$SIGNER_KEYCHAIN"); fi
planned() {
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$inputs/linux/plan.json" "$1"
}
sequence=$(planned sequence)
display=$(planned display)
published_at=$(planned published_at)
(cd "$repo" && cargo build --locked --release -p ducktape-updater --bin publish-release)
publish_release="${CARGO_TARGET_DIR:-$repo/target}/release/publish-release"
staged="$(dirname "$out")/staged"
signing_key=$(security find-generic-password -s dev.ducktape.signer -a release-signing-key -w ${keychain[@]+"${keychain[@]}"})
RELEASE_SIGNING_KEY="$signing_key" "$publish_release" \
  --channel "$channel" \
  --sequence "$sequence" \
  --display "$display" \
  --published-at "$published_at" \
  --notes-url duck://duckpond-725bcad1/forge/app \
  --archive linux-x86_64="$inputs/linux/ducktape-app-x86_64-unknown-linux-gnu.tar.gz" \
  --archive macos-aarch64="$inputs/macos-sign/ducktape-app-aarch64-apple-darwin.app.tar.gz" \
  --out "$staged" \
  --record /shared/releases/app/ > "$(dirname "$out")/upload-order"
unset signing_key
mkdir -p "$out"
cp "$staged/$channel.json" "$staged/record.json" "$(dirname "$out")/upload-order" "$out/"
mv "$staged/publish.json" "$out/publish.json"
