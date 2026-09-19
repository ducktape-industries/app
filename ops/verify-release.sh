#!/usr/bin/env bash
# Stage the release twice, each into a fresh target dir, and compare the
# binaries' sha256: a GO needs both stagings byte-identical. The build box
# has flipped bits before, so one build proves nothing and two equal ones
# bind the sha. The first staging is left in <out-dir>.
#   ops/verify-release.sh [--work <dir>] <out-dir>
# --work holds the two target dirs and the second staging (~20 GB each); by
# default a fresh dir under target/, removed when the builds agree.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
usage() { echo "usage: ops/verify-release.sh [--work <dir>] <out-dir>" >&2; exit 2; }
work= out=
while (($#)); do
  case $1 in
    --work) (($# >= 2)) || usage; work=$2; shift 2 ;;
    -*) usage ;;
    *) [[ -z $out ]] || usage; out=$1; shift ;;
  esac
done
[[ -n $out ]] || usage
own_work=
if [[ -z $work ]]; then
  mkdir -p "$repo/target"
  work=$(mktemp -d "$repo/target/verify-release.XXXXXX")
  own_work=1
fi
mkdir -p "$work"
CARGO_TARGET_DIR="$work/target-1" "$repo/ops/stage-release.sh" "$out"
CARGO_TARGET_DIR="$work/target-2" "$repo/ops/stage-release.sh" "$work/stage-2"
status=0
for bin in ducktape-launcher ducktape-app; do
  first=$(sha256sum <"$out/$bin") second=$(sha256sum <"$work/stage-2/$bin")
  echo "$bin ${first%% *} ${second%% *}"
  [[ $first == "$second" ]] ||
    { echo "$bin differs between two builds of one commit" >&2; status=1; }
done
if ((status == 0)) && [[ -n $own_work ]]; then rm -rf "$work"; fi
exit "$status"
