# app

The native desktop client, and a blind host: it reaches a node, reads the
roster of programs that node runs, fetches each program's view out of its
code blob and draws it. The app knows no program by name. Every screen
inside its chrome is a wasm view a program ships; the app's own screens are
two — reach a node, unlock a key.

```bash
# a node (ducktape-industries/ducktape, branch feat/capable-sandbox) is a
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
  signed at the sequence the node says is that signer's next.

## Layout

New to the code? Start with [ARCHITECTURE.md](ARCHITECTURE.md): the map of
`src/`, the life of a view and of a request, the threads, and the glossary
of the app's house words. The plans for accessibility and performance
tracking are [docs/ax.md](docs/ax.md) and [docs/perf.md](docs/perf.md).


| Path | What |
|---|---|
| `src/backend/noded.rs` | the node's `/v1` wire and the signed frame, mirrored from the kernel branch |
| `src/backend/views.rs` | roster → blob → `ducktape.view` section |
| `src/backend/session.rs` | the seated key, its frames, preferences |
| `src/runtime.rs`, `runtime/` | the wasm view runtime: seats, loads, swaps, the kernel relay |
| `src/render.rs`, `render/` | the wire tree presenter, its text fields the kit's engine holds (IME, caret, clipboard, undo) |
| `src/shell.rs`, `shell/` | the native chrome: the app's state as entities (`shell/entities/`, each written by its own methods) and each window's root with its layers (`shell/layers/`: launcher, menu bar, panes, dialogs, footer) |
| `src/ui/layout.rs`, `src/shell/entities/{desk,windows}.rs` | panes floating on the desk, popped out into their own windows and back |

## Dependency line

`ducktape-industries/modules` (`crates/sdk`; pinned by rev to modules `dev` until the next `dev` → `main` promotion): `abi`, `view-wire`, `ducklink`, `design`.
`ducktape` (`feat/capable-sandbox`): `ducktape-home`, `keystore`.

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
