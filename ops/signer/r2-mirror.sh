#!/usr/bin/env bash
# The signer's r2-mirror tool: `r2-mirror.sh <inputs> <out>` uploads a published release to the R2 mirror installed apps read, the archives first and the channel file last, once publish has recorded and tagged it.
# It mirrors only over an older published sequence, and reads the R2 credentials from the keychain (ops/signer/README.md).
set -euo pipefail
inputs=${1:?the inputs folder the signer hands over}
out=${2:?the folder the signer keeps}
channel=${SIGNER_ENV_CHANNEL:?the job names its channel in env CHANNEL}
mirror=https://dl.ducktape.industries/app
keychain=()
if [[ -n "${SIGNER_KEYCHAIN:-}" ]]; then keychain=("$SIGNER_KEYCHAIN"); fi
secret() {
  security find-generic-password -s dev.ducktape.signer -a "$1" -w ${keychain[@]+"${keychain[@]}"}
}
channel_file="$inputs/publish/$channel.json"
staged="$(dirname "$out")/mirrored"
mkdir -p "$staged" "$out"
sequence=$(python3 - "$channel_file" "$inputs/publish/upload-order" "$staged" \
  linux-x86_64="$inputs/linux/ducktape-app-x86_64-unknown-linux-gnu.tar.gz" \
  macos-aarch64="$inputs/macos-sign/ducktape-app-aarch64-apple-darwin.app.tar.gz" <<'PY'
import hashlib, json, shutil, sys
from pathlib import Path
channel_file, order, staged = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
manifest = json.loads(json.loads(channel_file.read_text())["manifest"])
archives = dict(given.split("=", 1) for given in sys.argv[4:])
if set(archives) != set(manifest["artifacts"]):
    sys.exit(f"r2-mirror: the channel file names {sorted(manifest['artifacts'])}, not the archives handed over, {sorted(archives)}")
by_sha = {}
for platform, path in archives.items():
    sha = hashlib.sha256(Path(path).read_bytes()).hexdigest()
    if manifest["artifacts"][platform]["sha256"] != sha:
        sys.exit(f"r2-mirror: the channel file's {platform} archive is not the one handed over ({sha})")
    by_sha[sha] = path
*names, last = order.read_text().split()
if last != channel_file.name:
    sys.exit(f"r2-mirror: publish-release's upload order ends with {last}, not the channel file {channel_file.name}")
for name in names:
    named = [sha for sha in by_sha if sha in name]
    if len(named) != 1:
        sys.exit(f"r2-mirror: the upload order's {name} names no one archive handed over")
    shutil.copy(by_sha.pop(named[0]), staged / name)
if by_sha:
    sys.exit(f"r2-mirror: the upload order names no archive of sha256 {', '.join(sorted(by_sha))}")
shutil.copy(channel_file, staged / channel_file.name)
shutil.copy(order, staged / "order")
print(manifest["sequence"])
PY
)
mirrored=$(curl -fsS "$mirror/$channel.json" 2>/dev/null \
  | python3 -c 'import json, sys; print(json.loads(json.load(sys.stdin)["manifest"]).get("sequence", 0))' 2>/dev/null || true)
mirrored=${mirrored:-0}
if [[ "$mirrored" -ge "$sequence" ]]; then
  echo "r2-mirror: $mirror/$channel.json already publishes $mirrored, not older than $sequence: not mirrored"
  exit 0
fi
access_key_id=$(secret r2-access-key-id)
secret_access_key=$(secret r2-secret-access-key)
account_id=$(secret r2-account-id)
bucket=$(secret r2-bucket)
while read -r name; do
  case "$name" in
    *.tar.gz) cache="public, max-age=31536000, immutable" ;;
    *) cache="public, max-age=60" ;;
  esac
  AWS_ACCESS_KEY_ID="$access_key_id" AWS_SECRET_ACCESS_KEY="$secret_access_key" AWS_DEFAULT_REGION=auto \
    aws s3 cp "$staged/$name" "s3://$bucket/app/$name" \
    --endpoint-url "https://$account_id.r2.cloudflarestorage.com" --cache-control "$cache"
done < "$staged/order"
echo "r2-mirror: $mirror/$channel.json publishes $sequence"
