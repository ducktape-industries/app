#!/usr/bin/env bash
# Draws one whole view_wire frame to a PNG with no display: the host's headless wgpu renderer (Mesa
# lavapipe where there is no GPU), the app's theme and bundled fonts. dev/screens/README.md.
set -euo pipefail
if (( $# < 2 || $# > 4 )); then
    echo "usage: $0 FRAME_JSON OUT_PNG [WxH] [light|dark]" >&2
    exit 2
fi
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SNAP_FRAME=$(realpath "$1")
SNAP_OUT=$(realpath -m "$2")
export SNAP_FRAME SNAP_OUT SNAP_SIZE=${3:-1100x760} SNAP_THEME=${4:-light}
cd "$ROOT"
cargo test --locked -p ducktape-host --lib render::tests::snap::snap -- --ignored --exact
