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
  `program.query`/`op.submit` `Call{target, body}`, `program.changes <program>`,
  `program.describe`, `chain.status`/`block`/`blocks`/`heads`, `invite.mint`,
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

| Path | What |
|---|---|
| `src/backend/noded.rs` | the node's `/v1` wire and the signed frame, mirrored from the kernel branch |
| `src/backend/views.rs` | roster → blob → `ducktape.view` section |
| `src/backend/session.rs` | the seated key, its frames, preferences |
| `src/runtime.rs`, `runtime/` | the wasm view runtime: seats, loads, swaps, the kernel relay |
| `src/render.rs`, `editor/` | the wire tree presenter and the one native text field (IME, caret, clipboard) |
| `src/shell.rs`, `ui/` | the window, the two native screens, the state and reducer |
| `src/shell/{layout,panes,windows}.rs` | one to three views side by side, pop-out into their own windows and back |

## Dependency line

`ducktape-industries/modules` (`crates/sdk`; pinned by rev to modules `dev` until the next `dev` → `main` promotion): `abi`, `view-wire`, `ducklink`, `design`.
`ducktape` (`feat/capable-sandbox`): `ducktape-home`, `keystore`.

Out until their upstreams settle: the self-update lane (`app-update`,
`bin/app-launcher`) — the kernel branch names `keyscheme` at an sdk revision
that no longer carries it, and cargo refuses a same-source patch.

## Building

`cargo fmt --all -- --check`, `cargo clippy --workspace --tests -- -D warnings`
and `cargo test --workspace` are what CI runs.
