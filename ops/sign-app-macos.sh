#!/usr/bin/env bash
# Sign a staged Ducktape.app inside-out, image it as a DMG, sign the DMG and —
# with all three DUCKTAPE_NOTARY_* set — notarize it and staple the DMG and the
# bundle. bundle-app-macos.sh's local path signs through here, and so does
# release.yml's macos-sign job, over the build job's bundle: nothing is
# built beside the certificate.
#
#   ops/sign-app-macos.sh <Ducktape.app> <out.dmg>
#
# DUCKTAPE_CODESIGN_IDENTITY: a name or a SHA-1 hash (ad-hoc `-` by default).
# DUCKTAPE_CODESIGN_KEYCHAIN: the keychain holding it, when not the default.
# DUCKTAPE_PROVISIONING_PROFILE: a Developer ID provisioning profile holding
#   the identity's certificate (macos-sign's comes from ops/apple-profile.swift).
#   It is embedded as Contents/embedded.provisionprofile, and the app is signed
#   with packaging/entitlements.plist plus the application identifier, team and
#   keychain access group it authorizes: a Touch ID-locked key lives in the
#   data protection keychain, which macOS opens to an app only with those
#   (TN3137). Before notarizing, the signature is checked against the profile.
#   Without a profile (ad-hoc, local) none of the three is signed: a restricted
#   entitlement no profile authorizes gets the app killed at launch.
# Every signature, with a profile or not, is launched once (`--version`)
# before the DMG, so AMFI judges it here.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
app=${1:?the Ducktape.app to sign}
dmg=${2:?the DMG to write}
identity=${DUCKTAPE_CODESIGN_IDENTITY:--}
profile=${DUCKTAPE_PROVISIONING_PROFILE:-}
notary_count=0
for value in "${DUCKTAPE_NOTARY_KEY:-}" "${DUCKTAPE_NOTARY_KEY_ID:-}" "${DUCKTAPE_NOTARY_ISSUER:-}"; do
  if [[ -n "$value" ]]; then notary_count=$((notary_count + 1)); fi
done
case "$notary_count" in
  0) ;;
  3) [[ "$identity" != - ]] || { echo "notarization requires DUCKTAPE_CODESIGN_IDENTITY" >&2; exit 1; } ;;
  *) echo "set all three DUCKTAPE_NOTARY_* credentials or none" >&2; exit 1 ;;
esac
[[ -z "$profile" || "$identity" != - ]] || { echo "a provisioning profile requires DUCKTAPE_CODESIGN_IDENTITY" >&2; exit 1; }
[[ -d "$app/Contents/MacOS" ]] || { echo "$app is not an app bundle" >&2; exit 1; }
contents="$app/Contents"
work=$(mktemp -d "${TMPDIR:-/tmp}/ducktape-sign.XXXXXX")
trap 'rm -rf "$work"' EXIT
sign=(--force --sign "$identity")
if [[ "$identity" != - ]]; then sign+=(--timestamp --options runtime); fi
if [[ -n "${DUCKTAPE_CODESIGN_KEYCHAIN:-}" ]]; then sign+=(--keychain "$DUCKTAPE_CODESIGN_KEYCHAIN"); fi
entitlements=${DUCKTAPE_ENTITLEMENTS:-$repo/packaging/entitlements.plist}
helper_entitlements=${DUCKTAPE_HELPER_ENTITLEMENTS:-$repo/packaging/helper-entitlements.plist}
bundle_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$contents/Info.plist")
if [[ -n "$profile" ]]; then
  cp "$profile" "$contents/embedded.provisionprofile"
  security cms -D -i "$contents/embedded.provisionprofile" > "$work/profile.plist"
  team=$(/usr/libexec/PlistBuddy -c "Print :TeamIdentifier:0" "$work/profile.plist")
  app_id="$team.$bundle_id"
  cp "$entitlements" "$work/entitlements.plist"
  entitlements="$work/entitlements.plist"
  /usr/libexec/PlistBuddy \
    -c "Add :com.apple.application-identifier string $app_id" \
    -c "Add :com.apple.developer.team-identifier string $team" \
    -c "Add :keychain-access-groups array" \
    -c "Add :keychain-access-groups:0 string $app_id" \
    "$entitlements"
fi
frameworks="$contents/Frameworks"
if [[ -d "$frameworks" ]]; then
  framework="$frameworks/Chromium Embedded Framework.framework"
  for library in "$framework"/Libraries/*.dylib; do
    codesign "${sign[@]}" "$library"
  done
  codesign "${sign[@]}" "$framework"
  for helper in "$frameworks"/*Helper*.app; do
    helper_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$helper/Contents/Info.plist")
    codesign "${sign[@]}" --identifier "$helper_id" \
      --entitlements "$helper_entitlements" "$helper"
  done
fi
codesign "${sign[@]}" --identifier "$bundle_id" --entitlements "$entitlements" "$contents/MacOS/ducktape-app"
codesign "${sign[@]}" --entitlements "$entitlements" "$app"
codesign --verify --deep --strict "$app"
if [[ -n "$profile" ]]; then
  # AMFI kills an app at launch whose signature claims what its profile does
  # not authorize; the hardened runtime's own (com.apple.security.*) need no
  # profile. Caught here, before a notarization.
  codesign -d --entitlements - --xml "$app" > "$work/signed.plist"
  refused=$(jq -rn \
    --argjson signed "$(plutil -convert json -o - "$work/signed.plist")" \
    --argjson allowed "$(plutil -extract Entitlements json -o - "$work/profile.plist")" \
    --arg app_id "$app_id" '
    # does the profile value (.) allow the signed one? A string ending in *
    # allows any string it prefixes; an array, what one of its items allows.
    def allows($value):
      . as $profile
      | if ($value | type) == "array" then all($value[]; . as $item | $profile | allows($item))
        elif type == "array" then any(.[]; allows($value))
        elif type == "string" and endswith("*") then
          ($value | type) == "string" and ($value | startswith($profile | rtrimstr("*")))
        else . == $value end;
    if $signed["com.apple.application-identifier"] == $app_id then empty
    else "com.apple.application-identifier is not \($app_id)" end,
    ($signed | to_entries[] | select(.key | startswith("com.apple.security.") | not)
      | select(.key as $key | .value as $value | $allowed | has($key) and (.[$key] | allows($value)) | not)
      | "\(.key) is not authorized by the profile")')
  [[ -z "$refused" ]] || { printf '%s\n' "$app is signed with entitlements its provisioning profile does not authorize:" "$refused" >&2; exit 1; }
fi
# Launched as the updater qualifies a release before it flips to it: AMFI
# checks the signature, the entitlements and the embedded profile (that it
# holds the signing certificate too) here, not on a user's Mac.
"$contents/MacOS/ducktape-app" --version > /dev/null \
  || { echo "$app does not launch as signed (AMFI kills a signature its profile does not authorize)" >&2; exit 1; }
image="$work/image"
mkdir "$image"
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
