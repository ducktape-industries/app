#!/usr/bin/env bash
# Sign a staged Ducktape.app inside-out, image it as a DMG, sign the DMG and —
# with all three DUCKTAPE_NOTARY_* set — notarize it and staple the DMG and the
# bundle. bundle-app-macos.sh's local path signs through here, and so does
# release.yml's macos-sign job: main's copy of this script over the build
# job's bundle, so the commit being built never runs beside the certificate.
#
#   ops/sign-app-macos.sh <Ducktape.app> <out.dmg>
#
# DUCKTAPE_CODESIGN_IDENTITY: a name or a SHA-1 hash (ad-hoc `-` by default).
# DUCKTAPE_CODESIGN_KEYCHAIN: the keychain holding it, when not the default.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
app=${1:?the Ducktape.app to sign}
dmg=${2:?the DMG to write}
identity=${DUCKTAPE_CODESIGN_IDENTITY:--}
notary_count=0
for value in "${DUCKTAPE_NOTARY_KEY:-}" "${DUCKTAPE_NOTARY_KEY_ID:-}" "${DUCKTAPE_NOTARY_ISSUER:-}"; do
  if [[ -n "$value" ]]; then notary_count=$((notary_count + 1)); fi
done
case "$notary_count" in
  0) ;;
  3) [[ "$identity" != - ]] || { echo "notarization requires DUCKTAPE_CODESIGN_IDENTITY" >&2; exit 1; } ;;
  *) echo "set all three DUCKTAPE_NOTARY_* credentials or none" >&2; exit 1 ;;
esac
[[ -d "$app/Contents/MacOS" ]] || { echo "$app is not an app bundle" >&2; exit 1; }
contents="$app/Contents"
sign=(--force --sign "$identity")
if [[ "$identity" != - ]]; then sign+=(--timestamp --options runtime); fi
if [[ -n "${DUCKTAPE_CODESIGN_KEYCHAIN:-}" ]]; then sign+=(--keychain "$DUCKTAPE_CODESIGN_KEYCHAIN"); fi
entitlements="$repo/packaging/entitlements.plist"
bundle_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$contents/Info.plist")
codesign "${sign[@]}" --identifier "$bundle_id" --entitlements "$entitlements" "$contents/MacOS/ducktape-app"
codesign "${sign[@]}" --entitlements "$entitlements" "$app"
codesign --verify --deep --strict "$app"
image=$(mktemp -d "${TMPDIR:-/tmp}/ducktape-dmg.XXXXXX")
trap 'rm -rf "$image"' EXIT
ditto "$app" "$image/Ducktape.app"
ln -s /Applications "$image/Applications"
hdiutil create -volname Ducktape -srcfolder "$image" -ov -format UDZO "$dmg"
codesign "${sign[@]}" "$dmg"
if [[ "$notary_count" == 3 ]]; then
  xcrun notarytool submit "$dmg" --key "$DUCKTAPE_NOTARY_KEY" \
    --key-id "$DUCKTAPE_NOTARY_KEY_ID" --issuer "$DUCKTAPE_NOTARY_ISSUER" --wait
  xcrun stapler staple "$dmg"
  xcrun stapler staple "$app"
fi
