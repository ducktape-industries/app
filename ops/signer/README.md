# The signer's tools for the app

The signer (core `ops/signer/signer.py`, its manual in core `ops/README.md` § Signing on a Mac) runs
only its own pinned tools. These three sign, publish and mirror the app's releases, for the release
workflows' `macos-sign`, `publish` and `mirror` jobs (`.forge/workflows/release-stable.toml`, a
push to `main`, and `release-nightly.toml`, a `nightly` dispatch). Each is called as
`<tool> <inputs> <out>`, with every needed job's outputs at `inputs/<job>/<path>` and `SIGNER_*`
set; what it writes in `out` the signer records as the check's outputs. `publish` also waits on
the workflows' `contract` job, which runs `ops/contract-gate.py` once `linux` and `macos-sign`
are done: a release goes out only while duckpond's `/v1/status` answers the commit's
`NODE_CONTRACT`, so a release built for the next contract before a roll is published by a run
after it (`python3 ops/contract-gate-test.py` tests the gate). The gate's line is that job's
output, `contract.txt`: the signer takes a need only with the outputs its builder recorded, so a
job that kept nothing would refuse `publish`. The three signing jobs name `signer-eddy`
(account 63) as their only runner, so no other account can claim or report them.

| Tool | Reads | Writes in `out` |
|---|---|---|
| `developer-id.sh` | `inputs/macos/`: `ducktape-app-unsigned.app.tar.gz`, `entitlements.plist`, `helper-entitlements.plist` | `ducktape-app-aarch64-apple-darwin.dmg` (signed, notarized, stapled) and `ducktape-app-aarch64-apple-darwin.app.tar.gz` (the update archive) |
| `release-manifest.sh` | `inputs/linux/plan.json` and `ducktape-app-x86_64-unknown-linux-gnu.tar.gz`, `inputs/macos-sign/ducktape-app-aarch64-apple-darwin.app.tar.gz`, `SIGNER_ENV_CHANNEL` | `<channel>.json`, `record.json`, `publish.json` and `upload-order` |
| `r2-mirror.sh` | `inputs/publish/<channel>.json` and `upload-order`, the same two archives, `SIGNER_ENV_CHANNEL` | nothing; it uploads to R2 |

- **developer-id** finds the Developer ID Application identity in the keychain, makes or reuses its
  provisioning profile (`ops/apple-profile.swift`, with the notary key) for the bundle's own
  identifier, and runs `ops/sign-app-macos.sh` over the unpacked bundle with the built commit's
  entitlements (`DUCKTAPE_ENTITLEMENTS`, `DUCKTAPE_HELPER_ENTITLEMENTS`): `main` may not hold the
  entitlements that commit's app needs, and the profile check in `sign-app-macos.sh` still bounds
  them. Then `spctl` assesses the DMG and the app.
- **release-manifest** builds `publish-release` from its own checkout before any key is read, then
  runs it with the release key over both archives: `--sequence`, `--display` and `--published-at`
  (the commit's time, so a rerun signs the same bytes) come from `plan.json`, and `--record
  /shared/releases/app/` writes the release record. The archives stay in the check's work folder
  beside `out`, which `publish.json` names, so the signer commits them into files and pushes the
  tag `release/<channel>/<sequence>` without putting them twice. `upload-order` is what
  `publish-release` printed: each archive's name in the order R2 takes them, the channel file
  last. It touches no mirror: a publish the signer refuses leaves R2 as it was.
- **r2-mirror** runs only once `publish` passed, so only a release that is recorded and tagged
  reaches installed apps. It checks that the signed channel file names exactly the archives handed
  over (each by its sha256), then uploads to R2 by the names and in the order `upload-order` holds
  (it works out no name itself, so a renamed archive changes only `publish-release`), each name
  matched to the one handed-over archive whose sha256 it carries: archives first (immutable cache)
  and the channel file last, and only when the mirror's `<channel>.json` publishes an older
  sequence.

## Installing

The tools run from a checkout of app at a fixed `main` commit, never the built one: its path in the
signer's `config.json`, with `cargo`, `python3`, `swift`, `aws` and Xcode's tools on the
LaunchAgent's `PATH`. `r2-mirror` takes the channels' branches as `release-manifest` does:

```json
"repos": {"app": {"developer-id": ["main", "dev"],
                  "release-manifest": {"stable": ["main"], "nightly": ["main", "dev"]},
                  "r2-mirror": {"stable": ["main"], "nightly": ["main", "dev"]}}},
"tools": {"developer-id": ["<app checkout>/ops/signer/developer-id.sh"],
          "release-manifest": ["<app checkout>/ops/signer/release-manifest.sh"],
          "r2-mirror": ["<app checkout>/ops/signer/r2-mirror.sh"]}
```

Every key is read from the keychain at use, never from a file the signer keeps: the Developer ID
Application identity (its private key allowing `codesign`), and generic passwords of service
`dev.ducktape.signer`, by account:

| Account | Holds |
|---|---|
| `notary-key` | the App Store Connect API key's `.p8`, base64 on one line (`base64 -i AuthKey_<id>.p8`), since `security -w` prints a value with line breaks as hex; for notarization and the profile |
| `notary-key-id`, `notary-issuer` | that key's id and issuer |
| `release-signing-key` | the self-update seed, 64 hex (`RELEASE_SIGNING_KEY`) |
| `r2-access-key-id`, `r2-secret-access-key`, `r2-account-id`, `r2-bucket` | the R2 mirror's credentials and bucket |

`SIGNER_KEYCHAIN` names the keychain that holds them, when it isn't the default search list.
Nothing a tool prints names a key; what it prints stays in the signer's own log.

`signer-tools-test.py` runs the three tools over stubs of `security`, the signing scripts,
`cargo`, `curl` and `aws` (`python3 ops/signer/signer-tools-test.py`, on Linux too).
