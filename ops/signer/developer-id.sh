#!/usr/bin/env bash
# The signer's developer-id tool: `developer-id.sh <inputs> <out>` Developer ID signs the macos job's bundle with that commit's entitlements and an embedded provisioning profile, images, notarizes and staples it.
# It runs from the signer's pinned checkout, never the built commit, and reads its keys from the keychain (ops/signer/README.md).
set -euo pipefail
inputs=${1:?the inputs folder the signer hands over}
out=${2:?the folder the signer keeps}
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
built="$inputs/macos"
keychain=()
if [[ -n "${SIGNER_KEYCHAIN:-}" ]]; then keychain=("$SIGNER_KEYCHAIN"); fi
secret() {
  security find-generic-password -s dev.ducktape.signer -a "$1" -w ${keychain[@]+"${keychain[@]}"}
}
work=$(mktemp -d "${TMPDIR:-/tmp}/developer-id.XXXXXX")
trap 'rm -rf "$work"' EXIT
identity=$(security find-identity -v -p codesigning ${keychain[@]+"${keychain[@]}"} \
  | awk '/Developer ID Application/ { print $2; exit }')
[[ -n "$identity" ]] || { echo "developer-id: no Developer ID Application identity in the keychain" >&2; exit 1; }
certificate=$(security find-certificate -a -Z -p ${keychain[@]+"${keychain[@]}"} \
  | awk -v sha1="$identity" '/ hash: / { take = ($1 == "SHA-1" && toupper($3) == toupper(sha1)); next } take')
[[ -n "$certificate" ]] || { echo "developer-id: no certificate with SHA-1 $identity in the keychain" >&2; exit 1; }
serial=$(printf '%s\n' "$certificate" | openssl x509 -noout -serial)
(umask 077 && secret notary-key | base64 --decode > "$work/notary.p8")
key_id=$(secret notary-key-id)
issuer=$(secret notary-issuer)
export DUCKTAPE_NOTARY_KEY="$work/notary.p8" DUCKTAPE_NOTARY_KEY_ID="$key_id" DUCKTAPE_NOTARY_ISSUER="$issuer"
mkdir "$work/signed"
tar xzf "$built/ducktape-app-unsigned.app.tar.gz" -C "$work/signed"
app="$work/signed/Ducktape.app"
bundle_id=$(plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")
swift "$repo/ops/apple-profile.swift" "$bundle_id" "${serial#serial=}" "$work/embedded.provisionprofile"
dmg="$out/ducktape-app-aarch64-apple-darwin.dmg"
mkdir -p "$out"
DUCKTAPE_CODESIGN_IDENTITY="$identity" \
  DUCKTAPE_CODESIGN_KEYCHAIN="${SIGNER_KEYCHAIN:-}" \
  DUCKTAPE_PROVISIONING_PROFILE="$work/embedded.provisionprofile" \
  DUCKTAPE_ENTITLEMENTS="$built/entitlements.plist" \
  DUCKTAPE_HELPER_ENTITLEMENTS="$built/helper-entitlements.plist" \
  "$repo/ops/sign-app-macos.sh" "$app" "$dmg"
spctl --assess --type open --context context:primary-signature -v "$dmg"
spctl --assess --type execute -v "$app"
COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$out/ducktape-app-aarch64-apple-darwin.app.tar.gz" \
  -C "$work/signed" Ducktape.app
