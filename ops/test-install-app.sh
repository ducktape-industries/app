#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
root=$(mktemp -d "${TMPDIR:-/tmp}/ducktape-install-test.XXXXXX")
trap 'rm -rf "$root"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
assert_file() { [[ -f $1 ]] || fail "missing file: $1"; }
assert_exec() { [[ -x $1 ]] || fail "missing executable: $1"; }

fixture=$root/fixture
mkdir -p "$fixture/ops" "$fixture/app/assets"
cp "$repo/Makefile" "$fixture/Makefile"
cp "$repo/ops/install-app.sh" "$fixture/ops/install-app.sh"
cp "$repo/app/assets/icon.svg" "$fixture/app/assets/icon.svg"

fake_uname=$root/uname
printf '%s\n' '#!/bin/sh' 'printf "%s\\n" "${FAKE_UNAME:-Linux}"' >"$fake_uname"
chmod 0755 "$fake_uname"

fake_linux_launcher=$root/linux-launcher
printf '%s\n' \
  '#!/bin/sh' \
  'printf "%s|%s|%s\\n" "${DUCKTAPE_INSTALL_DIR-}" "$2" "$3" >>"$LAUNCHER_LOG"' \
  'if [ "${LAUNCHER_FAIL-0}" = 1 ]; then exit 7; fi' \
  'mkdir -p "$XDG_DATA_HOME/ducktape/current" "$XDG_DATA_HOME/applications"' \
  'touch "$XDG_DATA_HOME/ducktape/current/ducktape-app" "$XDG_DATA_HOME/ducktape/current/ducktape-launcher" "$XDG_DATA_HOME/applications/dev.ducktape.app.desktop"' \
  'chmod 0755 "$XDG_DATA_HOME/ducktape/current/ducktape-app" "$XDG_DATA_HOME/ducktape/current/ducktape-launcher"' \
  >"$fake_linux_launcher"
chmod 0755 "$fake_linux_launcher"

fake_mac_launcher=$root/mac-launcher
printf '%s\n' \
  '#!/bin/sh' \
  'printf "%s|%s\\n" "$DUCKTAPE_INSTALL_DIR" "$3" >>"$LAUNCHER_LOG"' \
  'if [ "${LAUNCHER_FAIL-0}" = 1 ]; then exit 7; fi' \
  'mkdir -p "$DUCKTAPE_INSTALL_DIR/Ducktape.app/Contents/MacOS"' \
  'touch "$DUCKTAPE_INSTALL_DIR/Ducktape.app/Contents/MacOS/ducktape-app" "$DUCKTAPE_INSTALL_DIR/Ducktape.app/Contents/MacOS/ducktape-launcher"' \
  'chmod 0755 "$DUCKTAPE_INSTALL_DIR/Ducktape.app/Contents/MacOS/ducktape-app" "$DUCKTAPE_INSTALL_DIR/Ducktape.app/Contents/MacOS/ducktape-launcher"' \
  >"$fake_mac_launcher"
chmod 0755 "$fake_mac_launcher"

fake_cargo=$root/cargo
printf '%s\n' \
  '#!/bin/sh' \
  'printf "%s|%s|%s\\n" "$*" "${CARGO_TARGET_DIR-}" "${CARGO_BUILD_JOBS-}" >>"$FAKE_CARGO_LOG"' \
  'mkdir -p "$CARGO_TARGET_DIR/release"' \
  'printf "app\\n" >"$CARGO_TARGET_DIR/release/ducktape-app"' \
  'cp "$FAKE_LINUX_LAUNCHER" "$CARGO_TARGET_DIR/release/ducktape-launcher"' \
  'chmod 0755 "$CARGO_TARGET_DIR/release/ducktape-app" "$CARGO_TARGET_DIR/release/ducktape-launcher"' \
  >"$fake_cargo"
chmod 0755 "$fake_cargo"

fake_bundle=$fixture/ops/bundle-app-macos.sh
printf '%s\n' \
  '#!/bin/sh' \
  'set -eu' \
  'repo=$(cd "$(dirname "$0")/.." && pwd -P)' \
  'bin="$repo/target/app-bundle/Ducktape.app/Contents/MacOS"' \
  'printf "%s\\n" bundle >>"$FAKE_BUNDLE_LOG"' \
  'mkdir -p "$bin"' \
  'printf "app\\n" >"$bin/ducktape-app"' \
  'cp "$FAKE_MAC_LAUNCHER" "$bin/ducktape-launcher"' \
  'chmod 0755 "$bin/ducktape-app" "$bin/ducktape-launcher"' \
  >"$fake_bundle"
chmod 0755 "$fake_bundle"

run_linux() {
  local case_root=$root/linux
  mkdir -p "$case_root/home root" "$case_root/cargo home" "$case_root/xdg data" "$case_root/xdg config" "$case_root/target dir"
  PATH="$root:$PATH" FAKE_UNAME=Linux \
    CARGO="$fake_cargo" CARGO_HOME="$case_root/cargo home" \
    CARGO_TARGET_DIR="$case_root/target dir" CARGO_BUILD_JOBS=4 \
    FAKE_CARGO_LOG="$case_root/cargo.log" FAKE_LINUX_LAUNCHER="$fake_linux_launcher" \
    HOME="$case_root/home root" XDG_DATA_HOME="$case_root/xdg data" \
    XDG_CONFIG_HOME="$case_root/xdg config" LAUNCHER_LOG="$case_root/launcher.log" \
    make -C "$fixture" install
  assert_exec "$case_root/cargo home/bin/ducktape-app"
  assert_exec "$case_root/cargo home/bin/ducktape-launcher"
  assert_file "$case_root/xdg data/icons/hicolor/scalable/apps/ducktape.svg"
  assert_file "$case_root/xdg data/applications/dev.ducktape.app.desktop"
  grep -F -- "$case_root/target dir/release" "$case_root/launcher.log" >/dev/null || fail "Linux source path was not preserved"
  grep -F -- 'build --locked --release -p ducktape-app -p app-launcher' "$case_root/cargo.log" >/dev/null || fail "Linux build flags missing"
  grep -F -- '|4' "$case_root/cargo.log" >/dev/null || fail "CARGO_BUILD_JOBS was not preserved"
}

run_macos() {
  local case_root=$root/macos
  mkdir -p "$case_root/home root" "$case_root/cargo home"
  PATH="$root:$PATH" FAKE_UNAME=Darwin \
    CARGO_HOME="$case_root/cargo home" HOME="$case_root/home root" \
    FAKE_BUNDLE_LOG="$case_root/bundle.log" FAKE_MAC_LAUNCHER="$fake_mac_launcher" \
    LAUNCHER_LOG="$case_root/launcher.log" make -C "$fixture" install
  assert_exec "$case_root/cargo home/bin/ducktape-app"
  assert_exec "$case_root/cargo home/bin/ducktape-launcher"
  assert_exec "$case_root/home root/Applications/Ducktape.app/Contents/MacOS/ducktape-app"
  assert_file "$case_root/bundle.log"
  grep -F -- "$case_root/home root/Applications" "$case_root/launcher.log" >/dev/null || fail "macOS default path was not passed"

  rm -rf "$fixture/target" "$case_root/cargo home/bin" "$case_root/home root/Applications" "$case_root/launcher.log"
  override="$case_root/custom apps"
  DUCKTAPE_INSTALL_DIR="$override" PATH="$root:$PATH" FAKE_UNAME=Darwin \
    CARGO_HOME="$case_root/cargo home" HOME="$case_root/home root" \
    FAKE_BUNDLE_LOG="$case_root/bundle.log" FAKE_MAC_LAUNCHER="$fake_mac_launcher" \
    LAUNCHER_LOG="$case_root/launcher.log" make -C "$fixture" install
  assert_exec "$override/Ducktape.app/Contents/MacOS/ducktape-app"
  [[ ! -e "$case_root/home root/Applications/Ducktape.app" ]] || fail "macOS override was ignored"
}

run_launcher_failure() {
  local case_root=$root/failure
  mkdir -p "$case_root/home" "$case_root/cargo" "$case_root/target"
  if PATH="$root:$PATH" FAKE_UNAME=Linux CARGO="$fake_cargo" \
    CARGO_HOME="$case_root/cargo" CARGO_TARGET_DIR="$case_root/target" \
    FAKE_CARGO_LOG="$case_root/cargo.log" FAKE_LINUX_LAUNCHER="$fake_linux_launcher" \
    HOME="$case_root/home" XDG_DATA_HOME="$case_root/data" XDG_CONFIG_HOME="$case_root/config" \
    LAUNCHER_LOG="$case_root/launcher.log" LAUNCHER_FAIL=1 make -C "$fixture" install; then
    fail "launcher failure was swallowed"
  fi
}

run_unsupported() {
  local case_root=$root/unsupported
  mkdir -p "$case_root/home"
  if PATH="$root:$PATH" FAKE_UNAME=Plan9 HOME="$case_root/home" \
    make -C "$fixture" install 2>"$case_root/stderr"; then
    fail "unsupported platform unexpectedly succeeded"
  fi
  grep -F -- 'unsupported on Plan9' "$case_root/stderr" >/dev/null || fail "unsupported refusal was unclear"
}

run_linux
run_macos
run_launcher_failure
run_unsupported
echo "install-app mock tests: 4 passed"
