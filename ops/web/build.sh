#!/bin/sh
# Builds the web page from this tree into $1 (default target/web/page): the shell with the `web` profile for wasm32,
# wasm-bindgen's page glue, wasm-opt -Oz, the page, and a brotli and a gzip copy beside each file for the server to send.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$root/target/web/page}
target=${CARGO_TARGET_DIR:-$root/target}
cd "$root"
cargo build --locked --profile web --target wasm32-unknown-unknown -p ducktape-web-shell
rm -rf "$out"
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  "$target/wasm32-unknown-unknown/web/ducktape_web_shell.wasm"
wasm-opt -Oz \
  --enable-bulk-memory --enable-mutable-globals --enable-nontrapping-float-to-int \
  --enable-sign-ext --enable-multivalue --enable-reference-types \
  "$out/ducktape_web_shell_bg.wasm" -o "$out/ducktape_web_shell_bg.wasm"
cp crates/web-shell/www/index.html "$out/"
find "$out" -type f ! -name '*.br' ! -name '*.gz' | while read -r file; do
  brotli --best --keep "$file"
  gzip --best --keep "$file"
done
