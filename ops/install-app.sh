#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$repo"

[[ -n ${HOME:-} ]] || {
  echo "make install needs HOME to resolve user install directories" >&2
  exit 1
}

host=$(uname -s)
cargo=${CARGO:-cargo}
cargo_home=${CARGO_HOME:-$HOME/.cargo}
bin_dir=$cargo_home/bin

install_binary() {
  local source=$1 name=$2
  [[ -x $source ]] || {
    echo "missing executable: $source" >&2
    return 1
  }
  install -m 0755 "$source" "$bin_dir/$name"
}

install_entrypoints() {
  local source_dir=$1
  mkdir -p "$bin_dir"
  install_binary "$source_dir/ducktape-app" ducktape-app
  install_binary "$source_dir/ducktape-launcher" ducktape-launcher
}

case "$host" in
  Linux)
    "$cargo" build --locked --release -p ducktape-app -p app-launcher
    release_bin="${CARGO_TARGET_DIR:-target}/release"
    install_entrypoints "$release_bin"
    "$bin_dir/ducktape-launcher" install --from "$release_bin"

    data_home=${XDG_DATA_HOME:-$HOME/.local/share}
    icon_dir="$data_home/icons/hicolor/scalable/apps"
    mkdir -p "$icon_dir"
    install -m 0644 "$repo/app/assets/icon.svg" "$icon_dir/ducktape.svg"

    [[ -x "$data_home/ducktape/current/ducktape-app" ]] || {
      echo "launcher did not install the Linux app" >&2
      exit 1
    }
    [[ -x "$data_home/ducktape/current/ducktape-launcher" ]] || {
      echo "launcher did not install the Linux launcher" >&2
      exit 1
    }
    [[ -f "$data_home/applications/dev.ducktape.app.desktop" ]] || {
      echo "launcher did not install the Linux desktop entry" >&2
      exit 1
    }
    [[ -f "$icon_dir/ducktape.svg" ]] || {
      echo "Linux icon installation failed" >&2
      exit 1
    }
    ;;
  Darwin)
    "$repo/ops/bundle-app-macos.sh"
    bundle="$repo/target/app-bundle/Ducktape.app"
    source_bin="$bundle/Contents/MacOS"
    install_entrypoints "$source_bin"

    mac_install_dir=${DUCKTAPE_INSTALL_DIR:-$HOME/Applications}
    DUCKTAPE_INSTALL_DIR="$mac_install_dir" \
      "$bin_dir/ducktape-launcher" install --from "$bundle"

    [[ -x "$mac_install_dir/Ducktape.app/Contents/MacOS/ducktape-app" ]] || {
      echo "launcher did not install the macOS app bundle" >&2
      exit 1
    }
    [[ -x "$mac_install_dir/Ducktape.app/Contents/MacOS/ducktape-launcher" ]] || {
      echo "launcher did not install the macOS launcher" >&2
      exit 1
    }
    ;;
  *)
    echo "make install is unsupported on $host; use Linux or macOS" >&2
    exit 1
    ;;
esac

echo "installed ducktape-app and ducktape-launcher to $bin_dir"
