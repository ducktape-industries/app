#!/bin/sh
# The simulator gate (wasm-mobile SPEC §13.3 (b)), on a Mac: builds
# examples/sim_gate.rs for the arm64 simulator, bundles it as an app, runs it
# on the simulator SIM, and plays the outside part of its death check: when
# the gate prints `KILL_ME pid <n>`, that WebContent process is killed
# (`kill -9`, the one way to end a page's process from outside, S0-webkit §5).
# The gate's console is printed; the exit status is the gate's. Built
# --release: its tick times are a shipped build's.
#
#   crates/host-jsc/sim-gate.sh <simulator udid>
#
# CARGO_TARGET_DIR is honoured; four build jobs, as the shared Mac asks.
set -eu
sim=$1
root=$(cd "$(dirname "$0")/../.." && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
id=industries.ducktape.simgate

cd "$root"
IPHONEOS_DEPLOYMENT_TARGET=15.0 cargo build --locked --release -j 4 -p ducktape-host-jsc \
    --example sim_gate --features gate --target aarch64-apple-ios-sim
built=$target/aarch64-apple-ios-sim/release/examples
app=$built/SimGate.app
rm -rf "$app"
mkdir -p "$app"
cp "$built/sim_gate" "$app/"
cat > "$app/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$id</string>
<key>CFBundleExecutable</key><string>sim_gate</string>
<key>CFBundleName</key><string>SimGate</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>MinimumOSVersion</key><string>15.0</string>
<key>UIDeviceFamily</key><array><integer>1</integer></array>
<key>UILaunchScreen</key><dict/>
<key>UIApplicationSceneManifest</key><dict>
  <key>UIApplicationSupportsMultipleScenes</key><false/>
  <key>UISceneConfigurations</key><dict>
    <key>UIWindowSceneSessionRoleApplication</key><array><dict>
      <key>UISceneConfigurationName</key><string>Default Configuration</string>
      <key>UISceneDelegateClassName</key><string>DucktapeSimGateScene</string>
    </dict></array>
  </dict>
</dict>
</dict></plist>
EOF
codesign --force --sign - "$app"

log=$(mktemp)
xcrun simctl install "$sim" "$app"
xcrun simctl launch --console-pty --terminate-running-process "$sim" "$id" > "$log" 2>&1 &
launched=$!
killed=""
end=$(( $(date +%s) + 900 ))
until grep -q "GATE PASS\|GATE FAIL" "$log"; do
    if [ "$(date +%s)" -gt "$end" ]; then
        echo "GATE FAIL the gate did not finish in 15 minutes" >> "$log"
        break
    fi
    # the console ends with the app: one that died says no last line
    if ! kill -0 "$launched" 2>/dev/null; then
        sleep 1
        grep -q "GATE PASS\|GATE FAIL" "$log" || echo "GATE FAIL the app ended before the gate did" >> "$log"
        break
    fi
    pid=$(sed -n 's/.*GATE KILL_ME pid \([0-9]*\).*/\1/p' "$log" | head -1)
    # only a WebKit content process: the pid is the gate's own word for it
    if [ -n "$pid" ] && [ -z "$killed" ] && ps -p "$pid" -o command= | grep -q "WebContent"; then
        kill -9 "$pid" || true
        killed=$pid
        echo "KILLED $pid"
    fi
    sleep 1
done
xcrun simctl terminate "$sim" "$id" 2>/dev/null || true
xcrun simctl uninstall "$sim" "$id" || true
cat "$log"
passed=$(grep -c "GATE PASS" "$log" || true)
rm -f "$log"
[ "$passed" -gt 0 ]
