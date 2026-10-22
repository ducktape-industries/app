# The phone app

`crates/phone` (the shell) in the iOS launcher (`ios/`, XcodeGen) and the Android one (`android/`). How to build each:
the head of `Makefile`.

## The checks a change gets

- **Kotlin:** `make -C phone android-check` (`compileDebugKotlin` alone, no Rust and no NDK) is the gate `android` of build.sh's lane, on a change that touches `android/`; the lane fills the JDK, SDK and gradle home it compiles with on the builds machine.
- **Swift:** `make -C phone ios-check` type-checks `App.swift` and `DeviceKeyStore.swift` against `Phone.h`, and the driver, with `swiftc -typecheck` (seconds; no Rust, XcodeGen or simulator). `.forge/workflows/phone.toml` runs it on the mac relay for every change into `dev` and every push to `dev`. It is reported, never required: a Mac that sleeps holds no merge. It doesn't link, so an `@_cdecl` or `Phone.h` name Rust lacks shows only at `ios-sim` or the TestFlight archive.

## The mac service's driver

`ios/Driver/Driver.swift` is an XCUITest bundle (`DucktapeDriver`, the `Ducktape` scheme's test target, so never in the
archive) through which the mac service (core `ops/mac/mac.py`) taps the system UI the app's AX door can't see: the photo
picker, permission alerts. `make -C phone ios-driver`, after `ios-sim`, builds it for testing and prints its
`.xctestrun`. The Mac runs that with `xcodebuild test-without-building`, and the driver takes `{tap, app, deadline_ms}`
commands from the Mac at `TEST_RUNNER_DUCKTAPE_DRIVER`, tapping the first element of `app` whose label contains `tap`.

## How a build reaches TestFlight

`.github/workflows/testflight.yml`, on each push to `dev` that changes what the phone is built from (`crates/`,
`phone/`, `assets/`, `Cargo.*`, `rust-toolchain.toml`, the workflow), and on demand:

1. **plan** (GitHub's Ubuntu): the build number is the commit's committer time (`git show -s --format=%ct`), as the
   desktop release's sequence. One commit is always one number, a later `dev` commit a higher one, all far above the
   Expo builds' 2 and 3. If the tag `testflight-<build>` exists, TestFlight has it: the run stops here.
2. **archive** (the self-hosted runner `mac-byeongsu`): `make -C phone ios-archive BUILD=<build>` builds the phone's
   library for `aarch64-apple-ios` without the AX door and archives the app unsigned; `ios/privacy-scan.sh` fails the
   run if the binary reaches a required-reason API that `ios/PrivacyInfo.xcprivacy` does not declare. The only job
   that runs the commit's code; it sees no secret.
3. **upload** (the same runner, environment `release`, which admits `dev` and `main` only): no checkout; `xcodebuild
   -exportArchive` signs the archive with a cloud-managed distribution certificate through the App Store Connect API
   key (`-allowProvisioningUpdates`) and uploads it, then tags the commit `testflight-<build>`. App Store Connect
   processes the build (minutes) and offers it to the internal testers.

An upload waits on **contract** (GitHub's Ubuntu, after archive): `ops/contract-gate.py` on the commit, which stops
the run unless duckpond's `/v1/status` answers the contract its `NODE_CONTRACT` speaks. An upload is the release to
the testers, so no phone is offered an app on another contract. A build for the next contract is signed and exported
before the roll with `upload` off, which skips the check, and is uploaded by a run after the roll.

Version `0.1.0` (`ios/Info.plist`), bundle id `industries.ducktape.mobile`, team `8464KRUNQG`: the Expo app's App Store
Connect record, so this build replaces it for testers.

### The secrets

Organization secrets, the team's App Store Connect API key (the one the desktop DMG's notarization uses):

| secret | what |
|---|---|
| `APPLE_NOTARY_KEY` | the key's `.p8` file, as text |
| `APPLE_NOTARY_KEY_ID` | its key id |
| `APPLE_NOTARY_ISSUER` | the team's issuer id |

Cloud-managed signing and the upload need the key's role to be **Admin** (or App Manager with access to cloud-managed
distribution certificates); notarization alone works with less.

### Running it by hand

- From GitHub: Actions > testflight > Run workflow on `dev`. `upload` off signs and exports without uploading: the
  check that the key can sign.
- `gh workflow run testflight.yml --ref dev -f upload=false`.
- On a Mac with Xcode, XcodeGen and the Rust targets: `make -C phone ios-archive BUILD=1` gives
  `phone/ios/build/Ducktape.xcarchive`; Xcode's Organizer (or `xcodebuild -exportArchive`) signs and uploads it.

## The runner

`mac-byeongsu` is a tart VM (`gh-runner`, macOS 26) on the Mac `mac-byeongsu`, kept up by a LaunchAgent there. To give
a rebuilt VM what the phone needs, run on that Mac, as the VM's owner:

    ops/runner-ios-setup.sh

It copies the host's `/Applications/Xcode.app` into the VM (no Apple ID), selects it, accepts the license, runs
Xcode's first launch, downloads the Metal Toolchain, adds Rust's iOS targets and installs XcodeGen at a pinned
version and digest. It can be rerun; Xcode is copied again only when the host's build differs. App Store Connect
takes builds only from a current Xcode: when the host's Xcode moves to a new major version, rerun it.
