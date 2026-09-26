#!/usr/bin/env bash
# Native macOS bundle: stage the executable, then — on the local signing
# path — sign inside-out, image, optionally notarize. No view ships in the
# bundle: every view the app draws comes off the connected node's registry.
#
# ONE signing path per environment, chosen by DUCKTAPE_SIGN_VIA:
#   unset    local: DUCKTAPE_CODESIGN_IDENTITY (ad-hoc by default) signs the
#            bundle and the DMG here; the three DUCKTAPE_NOTARY_* notarize.
#   airlock  the airlock gateway signs: the bundle is staged UNSIGNED — no
#            codesign, no DMG, no notarization here — for `ducktape release
#            sign-bundle` (make release-app) to send to the enclave. Any of
#            the local identity/notary variables set alongside is refused
#            (`sign_path_conflict`); there is no fallback from one to the other.
#
# Ducktape.app ships ONE executable, `ducktape-app`, signed with the bundle's
# identifier: usernoted names a notification client by its code-signing
# identifier, and a helper left to codesign's default belongs to no app the
# Notification Center knows (UNErrorDomain 1, observed on macOS 27).
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
# The signing path is settled before anything else: a misconfigured
# environment is refused by name before a build or a host check.
sign_via=${DUCKTAPE_SIGN_VIA:-local}
identity=${DUCKTAPE_CODESIGN_IDENTITY:--}
notary_count=0
for value in "${DUCKTAPE_NOTARY_KEY:-}" "${DUCKTAPE_NOTARY_KEY_ID:-}" "${DUCKTAPE_NOTARY_ISSUER:-}"; do
  if [[ -n "$value" ]]; then notary_count=$((notary_count + 1)); fi
done
case "$sign_via" in
  local)
    case "$notary_count" in
      0) ;;
      3) [[ "$identity" != - ]] || { echo "notarization requires DUCKTAPE_CODESIGN_IDENTITY" >&2; exit 1; } ;;
      *) echo "set all three DUCKTAPE_NOTARY_* credentials or none" >&2; exit 1 ;;
    esac
    ;;
  airlock)
    if [[ -n "${DUCKTAPE_CODESIGN_IDENTITY:-}" || "$notary_count" != 0 ]]; then
      echo "sign_path_conflict: DUCKTAPE_SIGN_VIA=airlock with DUCKTAPE_CODESIGN_IDENTITY or DUCKTAPE_NOTARY_* set; one signing path per environment" >&2
      exit 1
    fi
    ;;
  *) echo "DUCKTAPE_SIGN_VIA=$sign_via is not a signing path (unset = local, airlock = the airlock gateway)" >&2; exit 1 ;;
esac
[[ "$(uname -s)" == Darwin ]] || { echo "macOS bundling requires macOS" >&2; exit 1; }
cd "$repo"
"${CARGO:-cargo}" build --locked --release -p ducktape-app
version=$(awk '/^\[workspace.package\]/{ package=1; next } /^\[/{ package=0 } package && /^version *=/{ gsub(/"/, "", $3); print $3; exit }' Cargo.toml)
[[ -n "$version" ]] || { echo "workspace package version missing" >&2; exit 1; }
release_bin="${CARGO_TARGET_DIR:-$repo/target}/release"
mkdir -p "$repo/target/app-bundle"
stage=$(mktemp -d "$repo/target/app-bundle/stage.XXXXXX")
app="$stage/Ducktape.app"
contents="$app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources" "$stage/Ducktape.iconset"
install -m 0755 "$release_bin/ducktape-app" "$contents/MacOS/ducktape-app"
install -m 0644 "$repo/packaging/Info.plist" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleShortVersionString string $version" "$contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleVersion string $version" "$contents/Info.plist"
swift "$repo/ops/macos-icon.swift" "$repo/assets/icon.svg" "$stage/Ducktape.iconset"
iconutil -c icns "$stage/Ducktape.iconset" -o "$contents/Resources/Ducktape.icns"
sign_locally() {
  local sign=(--force --sign "$identity")
  if [[ "$identity" != - ]]; then sign+=(--timestamp --options runtime); fi
  local entitlements="$repo/packaging/entitlements.plist"
  local bundle_id
  bundle_id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$contents/Info.plist")
  codesign "${sign[@]}" --identifier "$bundle_id" --entitlements "$entitlements" "$contents/MacOS/ducktape-app"
  codesign "${sign[@]}" --entitlements "$entitlements" "$app"
  codesign --verify --deep --strict "$app"
  mkdir "$stage/image"
  ditto "$app" "$stage/image/Ducktape.app"
  ln -s /Applications "$stage/image/Applications"
  dmg="$repo/target/app-bundle/Ducktape-$version-$(uname -m).dmg"
  hdiutil create -volname Ducktape -srcfolder "$stage/image" -ov -format UDZO "$dmg"
  codesign "${sign[@]}" "$dmg"
  if [[ "$notary_count" == 3 ]]; then
    xcrun notarytool submit "$dmg" --key "$DUCKTAPE_NOTARY_KEY" \
      --key-id "$DUCKTAPE_NOTARY_KEY_ID" --issuer "$DUCKTAPE_NOTARY_ISSUER" --wait
    xcrun stapler staple "$dmg"
    xcrun stapler staple "$app"
  fi
}
dmg=""
case "$sign_via" in
  local) sign_locally ;;
  # Unsigned, un-imaged: the enclave signs the bundle, and the DMG is not
  # part of the release archive.
  airlock) ;;
esac
# Only replace the completed named bundle, never the workspace or build root.
rm -rf "$repo/target/app-bundle/Ducktape.app"
mv "$app" "$repo/target/app-bundle/Ducktape.app"
rm -rf "$stage"
case "$sign_via" in
  local) echo "Built target/app-bundle/Ducktape.app and $dmg" ;;
  airlock) echo "Built target/app-bundle/Ducktape.app UNSIGNED for ducktape release sign-bundle" ;;
esac
