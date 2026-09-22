#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR=/home/eddy/dev/ducktape/target-w1 RUSTC_WRAPPER=
OUT=/home/eddy/dev/ducktape/wt/chat-screens
mkdir -p "$OUT/logs"
# Local path overrides may rewrite source lines; preserve the caller's lockfile.
LOCK_COPY=$(mktemp "$OUT/logs/Cargo.lock.XXXXXX")
cp Cargo.lock "$LOCK_COPY"
restore_lock() { cp "$LOCK_COPY" Cargo.lock; rm "$LOCK_COPY"; }
trap restore_lock EXIT
if [[ ${1:-} != --capture-only ]]; then
    for attempt in 1 2 3 4 5; do
        set +e
        cargo build -p ducktape-app >"$OUT/logs/host-build.log" 2>&1
        result=$?
        set -e
        echo "host build exit=$result attempt=$attempt"
        if (( result == 0 )); then break; fi
        tail -40 "$OUT/logs/host-build.log"
        if ! rg -q 'SIGSEGV|signal: 11' "$OUT/logs/host-build.log"; then exit "$result"; fi
    done
    (( result == 0 )) || exit "$result"
fi
python3 scripts/chat-screens.py
