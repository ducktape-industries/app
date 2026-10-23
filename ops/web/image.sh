#!/bin/sh
# Builds the web image from commit $1 (default HEAD) of this repository, the commit's tree alone as the build context, and
# tags it $2 (default ducktape-web:<commit>). Extra podman words go in PODMAN_BUILD (e.g. --build-arg TOOLCHAIN=<ref>).
set -eu
cd "$(git rev-parse --show-toplevel)"
commit=$(git rev-parse --verify "${1:-HEAD}^{commit}")
tag=${2:-ducktape-web:$(printf '%.12s' "$commit")}
git archive --format=tar "$commit" |
  podman build ${PODMAN_BUILD:-} --file ops/web/Containerfile --label "org.opencontainers.image.revision=$commit" --tag "$tag" -
echo "$tag"
