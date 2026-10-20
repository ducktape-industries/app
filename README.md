# app

The native desktop client, and a blind host: it reaches a node, reads the
roster of programs that node runs, fetches each program's view out of its
code blob and draws it. The app knows no program by name. Every screen
inside its chrome is a wasm view a program ships; the app's own screens are
two — reach a node, unlock a key.

```bash
# a node (ducktape-industries/ducktape, rev b5f164664) is a
# separate process this app talks to over /v1; run one, then:
cargo run -p ducktape-app
DUCKTAPE_RPC=http://127.0.0.1:8844 cargo run -p ducktape-app   # skip the connect screen
DUCKTAPE_VIEWS_DIR=/path/to/views cargo run -p ducktape-app     # a view developer's override
```

## Install

Releases are published to `https://dl.ducktape.industries/app/`: one
channel file per channel (`stable.json` is `main`, `nightly.json` is `dev`
once a day) naming each platform's archive by its sha256. An installed app
keeps itself up to date from there; Settings › Updates picks the channel.
Its "Update automatically" switch (on unless turned off) stops that: off,
the app still checks and says what is newer, but downloads and installs
nothing until you press Update now there.
For nightly, read `nightly.json` instead of `stable.json` below.

**macOS (Apple silicon).** Needs `jq`. The archive is `Ducktape.app`,
Developer ID signed and notarized, with a Developer ID provisioning profile
embedded: it authorizes the app's application identifier and keychain access
group, which a key locked by Touch ID needs (macOS keeps those in the data
protection keychain). At its first start from `/Applications` it adopts
itself as the install the updater owns.

```bash
sha=$(curl -fsS https://dl.ducktape.industries/app/stable.json \
  | jq -r .manifest | jq -r '.artifacts["macos-aarch64"].sha256')
curl -fLo /tmp/ducktape.tar.gz "https://dl.ducktape.industries/app/Ducktape-$sha-macos-aarch64.tar.gz"
echo "$sha  /tmp/ducktape.tar.gz" | shasum -a 256 -c
tar xzf /tmp/ducktape.tar.gz -C /Applications
open /Applications/Ducktape.app
```

The DMG on a commit's GitHub prerelease (`bin-<commit>`) works too, but a
browser download is quarantined: after dragging it to `/Applications`, run
`xattr -dr com.apple.quarantine /Applications/Ducktape.app` once.

**Linux (x86_64).** Needs `jq`, and at runtime `libxcb1`,
`libxkbcommon-x11-0`, ALSA's library for the microphone and speakers
(`libasound2`; `libasound2t64` from Ubuntu 24.04 on) and a Vulkan driver
(`libvulkan1` and `mesa-vulkan-drivers` on Ubuntu). `ducktape-app install`
puts the binary under `~/.local/share/ducktape/` and registers a desktop
entry; the extracted copy can be deleted after.

```bash
sha=$(curl -fsS https://dl.ducktape.industries/app/stable.json \
  | jq -r .manifest | jq -r '.artifacts["linux-x86_64"].sha256')
curl -fLo /tmp/ducktape.tar.gz "https://dl.ducktape.industries/app/Ducktape-$sha-linux-x86_64.tar.gz"
echo "$sha  /tmp/ducktape.tar.gz" | sha256sum -c
tar xzf /tmp/ducktape.tar.gz -C /tmp
/tmp/ducktape-app install
```

## What the app does

- **Connect.** A node URL. `/v1/status` answers the network the node serves
  and the contract it speaks; the app refuses a contract it does not know.
- **Roster → views.** `/v1/programs` lists every program and its code blob.
  A program that ships a view carries it inside that blob as the
  `ducktape.view` custom section; the app reads it out, compiles it and
  seats it. The roster's order is the rail's order; a view's manifest names
  its tab. A program without the section is left off the rail.
- **Relay.** A view asks through the kernel contract (`crates/host/src/runtime/kernel.rs`),
  every method a borsh type in `view_wire::methods`, named `<capability>.<op>`:
  `program.query`/`op.submit` `Call{target, body}`, `program.changes <program>`,
  `program.describe`, `chain.status`/`block`/`blocks`/`heads`, `invite.create`,
  `blob.get`, `blob.manifest`, `link.open`, `link.window`, `host.widget` (the one MessagePack method: a tree
  command), `live.*` (`publish`/`send`/`subscribe`/`tracks`: a view's live
  tracks through the node's relay, off consensus, on one QUIC or websocket
  session per connection, `src/backend/live.rs`), and the app's own
  (`host.*`, `clock.ticks`, `clipboard.*`, `notify.*`, `store.*`), and
  `socket.open`/`socket.send` (a WebSocket from this device to a `wss://`
  host the view's manifest lists, once the person allowed those hosts on a
  card; `src/runtime/kernel/socket.rs`) with `host.random`. A view
  reaches only the capabilities its manifest declares. The app forwards the
  bytes to the program the view names and signs writes with the seated key;
  it never reads a payload.
- **Sign in.** The key file under `$DUCKTAPE_USER_KEY`, else the network's
  active wallet under `<ducktape home>/remotes/<network>/`. A write is a frame
  anchored at the tip `/v1/status` reports and salted with a random number,
  so the node runs it once and only while that block is recent.

## Sending a transaction from the terminal

```bash
ducktape-app send <network> <program> <hex>
```

This sends one transaction, signed with the key this device holds for the
network: the same key the app's own screens sign with. It opens no window.

- `<network>` is a network from the app's Recent list. A bare name
  (`duckpond`) goes to the node the app last used for that name, and signs
  for the chain that node runs now. A name with its founding time
  (`duckpond+1792250296000`) picks that one chain.
- `<program>` is the program's name on the chain (`identity`).
- `<hex>` is the call's bytes, hex-encoded, exactly as the program decodes
  them (a borsh `identity::Op`, for example).

The key opens the way it does in the app. The OS may ask to unlock its key
store, or for Touch ID. A password-locked key from before keys moved into
the OS asks for its password on the terminal, without echo.

The node preconfirms the frame. The command then waits for the block that
carries it and prints the frame's id, the block height, and `applied` with
any output as hex, or `refused <code>: <sentence>`. It exits 0 when the
call applied, 1 when it was refused or did not land, and 2 when the
arguments are wrong. When the network is not in the Recent list, its node
does not answer, or this device holds no key for the chain it runs, the
command also lists the networks it can send to.

## Layout

New to the code? Start with [ARCHITECTURE.md](ARCHITECTURE.md): the map of
`src/`, the life of a view and of a request, the threads, and the glossary
of the app's house words. The plans for accessibility and performance
tracking are [docs/ax.md](docs/ax.md) and [docs/perf.md](docs/perf.md).


| Path | What |
|---|---|
| `crates/noded-client` | the node's `/v1` wire and the signed frame, mirrored from the kernel branch, and the flows onto an account; shared with the phone |
| `crates/host` | the blind host core the desktop shell and the phone shell share; `src/contract.rs` is the one interface a shell implements (directories, the reader key, banners, OS facts, invite minting) |
| `crates/host/src/backend/views.rs` | roster → blob → `ducktape.view` section |
| `crates/host/src/backend/session.rs` | the seated key (any `noded_client::Signer`), its frames, preferences |
| `crates/host/src/backend/live.rs`, `crates/host/src/runtime/kernel/live.rs` | the live session with the node's relay (core's `live-wire`: QUIC, or the websocket) and the `live.*` methods over it |
| `crates/host/src/runtime.rs`, `runtime/` | the wasm view runtime: seats, loads, swaps, the kernel relay |
| `crates/host/src/render.rs`, `render/` | the wire tree presenter, its text fields the kit's engine holds (IME, caret, clipboard, undo) |
| `src/os.rs`, `src/backend/` | the desktop's `OsServices` impl (its directories, the freedesktop / macOS banners, invites with the node this machine runs) and what only the desktop runs: the device key, passkeys, recovery keys, `ducktape-app send` (`src/backend/send.rs`) |
| `src/shell.rs`, `shell/` | the native chrome: the app's state as entities (`shell/entities/`, each written by its own methods) and each window's root with its layers (`shell/layers/`: launcher, menu bar, panes, dialogs, footer) |
| `src/ui/layout.rs`, `src/shell/entities/{desk,windows}.rs` | panes floating on the desk, popped out into their own windows and back |

## Dependency line

`ducktape-industries/programs` (rev `f45348abf11f5c53c8b8313ca97fcd352ac756ba`): `view-wire`, `program`, `ducklink`, `design`, `describe`, `identity`, `valset`, `programs`, `blobs`, `files`.
`ducktape-industries/ducktape` (rev `f836730c6a3adeb1657bbe61f6c04524149bfcc3`, the rev that programs rev pins): `abi`, `blob`, `keyscheme`, `ducktape-home`, `keystore`, `live-wire`.
`ducktape-industries/gpui-pre` (rev `eef11e96c9fcf7e0389d31f125013370a1b02d92`, the rev that programs rev pins): `gpui-pre` and the crates it patches.

## Building

The toolchain is pinned in `rust-toolchain.toml`. A fresh Linux machine needs
git, a C toolchain and the packages CI installs (Ubuntu names):

```bash
sudo apt-get install -y pkg-config libclang-dev libasound2-dev \
  libx11-xcb-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libfontconfig1-dev libfreetype6-dev \
  mesa-vulkan-drivers libvulkan1
```

The last two are the Vulkan driver a machine with no GPU draws with (a
container, a CI runner): with no driver the app opens no window and says so
only in `~/.local/state/ducktape/app.log`. On macOS, Xcode is all the build
needs.

`cargo fmt --all -- --check`,
`cargo clippy --locked --workspace --tests --no-deps -- -D warnings` (and again
with `--features ax-door`) and `cargo test --locked --workspace --no-fail-fast`
are what CI runs.

Each commit on `main` and `dev` also has a built app: the prerelease
`bin-<commit>` carries `ducktape-app-x86_64-unknown-linux-gnu.tar.gz` and a
Developer ID signed, notarized `ducktape-app-aarch64-apple-darwin.dmg`
(`.github/workflows/release.yml`); the same build is what the stable (`main`)
or nightly (`dev`) channel publishes (see [Install](#install)).
