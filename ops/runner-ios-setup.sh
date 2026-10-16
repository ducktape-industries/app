#!/bin/sh
# Makes the tart VM behind the self-hosted runner `mac-byeongsu` able to
# archive the phone app (`make -C phone ios-archive`, .github/workflows/
# testflight.yml): the host's Xcode copied in, selected and first-launched,
# the Metal Toolchain, Rust's iOS targets and a pinned XcodeGen. Run it on the
# Mac that runs the VM, as the user that owns the VM; it can be rerun, and
# copies Xcode again only when the host's build differs from the VM's.
#
#   ops/runner-ios-setup.sh             the VM `gh-runner`
#   VM=<name> ops/runner-ios-setup.sh
#
# On the host it only reads /Applications/Xcode.app; everything it installs
# goes into the VM. No Apple ID: Xcode comes from the host, the Metal
# Toolchain is a component `xcodebuild` downloads by itself. The desktop DMG
# job (release.yml) builds with the Xcode selected here too.
set -eu

VM=${VM:-gh-runner}
# rust-toolchain.toml's channel
RUST_TOOLCHAIN=1.96.1
XCODEGEN_VERSION=2.46.0
XCODEGEN_SHA256=4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806
tart=/opt/homebrew/bin/tart

vm() { "$tart" exec "$VM" zsh -lc "$1"; }

# Xcode: the host's own bundle, streamed in (no download, no account)
build_of='defaults read /Applications/Xcode.app/Contents/version ProductBuildVersion'
host_build=$(sh -c "$build_of")
vm_build=$(vm "$build_of 2>/dev/null || true")
if [ "$host_build" != "$vm_build" ]; then
  echo "Xcode $host_build into $VM (was: ${vm_build:-none})"
  vm 'sudo rm -rf /Applications/Xcode.app'
  tar -C /Applications -cf - Xcode.app | "$tart" exec -i "$VM" sudo tar -C /Applications -xpf -
fi
vm 'sudo xcode-select -s /Applications/Xcode.app/Contents/Developer &&
    sudo xcodebuild -license accept &&
    sudo xcodebuild -runFirstLaunch'

# build-time Metal shaders (gpui's); a MobileAsset outside Xcode.app
vm 'xcodebuild -showComponent MetalToolchain | grep -q "Status: installed" ||
    xcodebuild -downloadComponent MetalToolchain'

# the phone's Rust targets on the app's toolchain (rust-toolchain.toml)
vm "rustup target add --toolchain $RUST_TOOLCHAIN aarch64-apple-ios aarch64-apple-ios-sim"

# XcodeGen, the release binary at a pinned version and digest
vm "command -v xcodegen >/dev/null && xcodegen --version | grep -qx 'Version: $XCODEGEN_VERSION' || {
      d=\$(mktemp -d) &&
      curl -fsSL -o \$d/x.zip https://github.com/yonaskolb/XcodeGen/releases/download/$XCODEGEN_VERSION/xcodegen.zip &&
      echo '$XCODEGEN_SHA256  '\$d/x.zip | shasum -a 256 -c - &&
      unzip -q \$d/x.zip -d \$d &&
      sudo \$d/xcodegen/install.sh /usr/local &&
      rm -rf \$d
    }"

vm "xcodebuild -version && xcrun --sdk iphoneos --show-sdk-version &&
    xcodebuild -showComponent MetalToolchain | grep Status &&
    xcodegen --version && rustup target list --installed --toolchain $RUST_TOOLCHAIN && df -h /"
