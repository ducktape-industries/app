#!/usr/bin/env bash
# CEF's build tools for a macOS build, `ops/cef-tools-macos.sh <dir>`: cmake and ninja at pinned versions, fetched into <dir> and checked by sha256.
# Prints each tool's bin folder on a line of its own, for PATH; every other word goes to stderr.
set -euo pipefail
dir=${1:?the folder the tools go in}
mkdir -p "$dir/ninja"
curl -fsSL -o "$dir/cmake.tar.gz" https://cmake.org/files/v3.31/cmake-3.31.6-macos-universal.tar.gz
echo "330b9514f5112e5ed4fb08b8b05803b776fd9b539a6ae12927d14dcc0ee2ba8d  $dir/cmake.tar.gz" | shasum -a 256 -c - >&2
tar xzf "$dir/cmake.tar.gz" -C "$dir"
curl -fsSL -o "$dir/ninja-mac.zip" https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-mac.zip
echo "89a287444b5b3e98f88a945afa50ce937b8ffd1dcc59c555ad9b1baf855298c9  $dir/ninja-mac.zip" | shasum -a 256 -c - >&2
unzip -oq "$dir/ninja-mac.zip" -d "$dir/ninja" >&2
echo "$dir/cmake-3.31.6-macos-universal/CMake.app/Contents/bin"
echo "$dir/ninja"
