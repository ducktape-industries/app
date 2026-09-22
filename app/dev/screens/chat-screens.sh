#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
if (( $# < 2 || $# > 3 )) || [[ ${3:-} != '' && ${3:-} != --capture-only ]]; then
    echo "usage: $0 FIXTURES_DIR OUTPUT_DIR [--capture-only]" >&2
    exit 2
fi
FIXTURES=$(realpath "$1")
mkdir -p "$2/logs"
OUT=$(realpath "$2")
cd "$ROOT"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target} RUSTC_WRAPPER=
CARGO_TARGET_DIR=$(realpath -m "$CARGO_TARGET_DIR")
if [[ ${3:-} != --capture-only ]]; then
    for attempt in 1 2 3 4 5; do
        set +e
        cargo build --locked -p ducktape-app >"$OUT/logs/host-build.log" 2>&1
        result=$?
        set -e
        echo "host build exit=$result attempt=$attempt"
        if (( result == 0 )); then break; fi
        tail -40 "$OUT/logs/host-build.log"
        if ! rg -q 'SIGSEGV|signal: 11' "$OUT/logs/host-build.log"; then exit "$result"; fi
    done
    (( result == 0 )) || exit "$result"
fi
python3 "$ROOT/app/dev/screens/chat-screens.py" "$FIXTURES" "$OUT" \
    --binary "$CARGO_TARGET_DIR/debug/ducktape-app"
