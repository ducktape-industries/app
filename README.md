# app

The native desktop client, and a blind host: it reaches a node, reads the
roster of programs that node runs, fetches each program's view out of its
code blob and draws it. The app knows no program by name. Every screen
inside its chrome is a wasm view a program ships; the app's own screens are
two — reach a node, unlock a key.

```bash
# a node (ducktape-industries/ducktape at the rev Cargo.toml pins) is a
# separate process this app talks to over /v1; run one, then:
cargo run -p ducktape-app
DUCKTAPE_RPC=http://127.0.0.1:8844 cargo run -p ducktape-app   # skip the connect screen
DUCKTAPE_VIEWS_DIR=/path/to/views cargo run -p ducktape-app     # a view developer's override
```

## What the app does

- **Connect.** A node URL. `/v1/status` answers the network the node serves
  and the contract it speaks; the app refuses a contract it does not know.
- **Roster → views.** `/v1/programs` lists every program and its code blob.
  A program that ships a view carries it inside that blob as the
  `ducktape.view` custom section; the app reads it out, compiles it and
  seats it. The roster's order is the rail's order; a view's manifest names
  its tab. A program without the section is left off the rail.
- **Relay.** A view asks through the kernel contract (`src/runtime/kernel.rs`),
  every method a borsh type in `view_wire::methods`, named `<capability>.<op>`:
  `module.query`/`op.submit` `Call{target, body}`, `module.changes <program>`,
  `module.describe`, `chain.status`/`block`/`blocks`/`heads`, `invite.create`,
  `blob.get`, `link.open`, `host.widget` (the one MessagePack method: a tree
  command), and the app's own (`host.*`, `clock.ticks`, `clipboard.*`,
  `notify.*`, `store.*`). A view reaches only the capabilities its manifest
  declares. The app forwards the bytes to the
  program the view names and signs writes with the seated key; it never
  reads a payload.
- **Sign in.** The key file under `$DUCKTAPE_USER_KEY`, else the network's
  active wallet under `<ducktape home>/remotes/<network>/`. A write is a frame
  anchored at the tip `/v1/status` reports and salted with a random number,
  so the node runs it once and only while that block is recent.

## Layout

New to the code? Start with [ARCHITECTURE.md](ARCHITECTURE.md): the map of
`src/`, the life of a view and of a request, the threads, and the glossary
of the app's house words. The plans for accessibility and performance
tracking are [docs/ax.md](docs/ax.md) and [docs/perf.md](docs/perf.md).


| Path | What |
|---|---|
| `src/backend/noded.rs` | the node's `/v1` wire and the signed frame, mirrored from the pinned core rev |
| `src/backend/views.rs` | roster → blob → `ducktape.view` section |
| `src/backend/session.rs` | the seated key, its frames, preferences |
| `src/runtime.rs`, `runtime/` | the wasm view runtime: seats, loads, swaps, the kernel relay |
| `src/render.rs`, `render/` | the wire tree presenter, its text fields the kit's engine holds (IME, caret, clipboard, undo) |
| `src/shell.rs`, `shell/` | the native chrome: the app's state as entities (`shell/entities/`, each written by its own methods) and each window's root with its layers (`shell/layers/`: launcher, menu bar, panes, dialogs, footer) |
| `src/ui/layout.rs`, `src/shell/entities/{desk,windows}.rs` | panes floating on the desk, popped out into their own windows and back |
| `crates/network` | `ducktape-network`, which founds or joins a network at the revs this workspace pins ("Run a network" below) |

## Dependency line

`ducktape-industries/modules` (`crates/sdk`; pinned by rev to modules `dev` until the next `dev` → `main` promotion): `abi`, `view-wire`, `ducklink`, `design`.
`ducktape` (one rev on `main`, the node the app speaks): `ducktape-home`, `keystore`, `keyscheme`.

Out until their upstreams settle: the self-update lane (`app-update`,
`bin/app-launcher`) — the kernel branch names `keyscheme` at an sdk revision
that no longer carries it, and cargo refuses a same-source patch.

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

## Run a network: `ducktape-network`

`crates/network` builds `ducktape-network`, the server side for whoever runs a
network for this app: it founds or joins one from published artifacts, with no
checkouts and no wasm toolchain. It installs core's `ducktape` node at the core
commit this workspace pins (Cargo.lock; core's release `bin-<commit>`) and the
programs at the modules commit it pins (modules' release `wasm-<commit>`), so a
network it founds is one this build of the app speaks. `ducktape-network
--version` names both commits.

Install on Linux x86_64 or macOS on Apple silicon (the targets core publishes
the node for), with a Rust toolchain, `curl` and `tar`; it is not published as
a binary of its own yet:

```bash
cargo install --locked --git https://github.com/ducktape-industries/app ducktape-network
```

Found a network, this server its one validator:

```bash
ducktape-network found mynet --address tcp://203.0.113.7:9000
```

It downloads the node and the programs, checks the programs, packs each view
and describe module into its program, writes `~/.ducktape-network/founding.toml`,
founds the network and runs the node as the systemd service `ducktape-network`:
a system unit when run as root, else a user unit (`loginctl enable-linger`
keeps it running after you log out). On macOS it prints the `ducktape run`
command instead. It prints the URL the app connects to (here
`http://203.0.113.7:8844`) and an invite with the command for a second server.
The address takes core's forms: `tcp://host:port`, `tls://host:port`,
`ws://host[:port]` or `wss://host[:port]` (core's README, "Reach a node").

Join it from a second server:

```bash
ducktape-network join http://203.0.113.7:8844 --address tcp://198.51.100.4:9000 --invite <hex>
```

It enrolls, waits for the epoch that seats it, syncs and runs, as a resident:
it follows the chain and does not vote. It prints the vote the founder runs on
the first server to seat it as a validator, `ducktape-network cli vote promote <key>`.

`ducktape-network status` prints the node's height, and `ducktape-network cli
ARGS…` is the node's own CLI on this network's workspace (`invite`, `vote`,
`logs`, `wallet`, …). `--dir` puts everything somewhere other than
`~/.ducktape-network` (one network per directory, and one service per user).
For testing, or for a commit not published yet, `--ducktape <binary>` and
`--wasm <dir>` (unpacked, with its `SHA256SUMS`) stand in for the downloads.

**What is public.** The node listens on every interface: on the address's port
(the p2p socket for `tcp://`, the TLS socket for `tls://`; `ws://` and `wss://`
ride the HTTP port) and on the HTTP API, `--http-port` (default 8844), which
the app and joining servers reach. Anyone who reaches the HTTP port can read
the network's state; writes are frames a key signs, and the node's own admin
calls are signed with its identity key. Put the port behind a firewall or a
proxy when the network must not be readable by everyone.

**What is checked.** Every program, describe module and view must match its
line in the `SHA256SUMS` inside modules' archive before it is packed; a
mismatch stops the founding. That file travels in the same archive, so it
catches a damaged download, not a forged release. Core publishes no checksum
for the node at all: the trust is GitHub's TLS (`curl --proto =https`) and a
release tag that names the commit.
