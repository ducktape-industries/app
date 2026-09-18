# ducktape-app

The native GPUI desktop shell for a ducktape workspace: `app/` (the
`ducktape-app` binary), `bin/app-launcher/` (its boot-time flip/rollback/exec
executor) and `crates/gpui-notion/` (the Notion-shaped block editor it
embeds for Pages). See `app/README.md` for how the app finds a workspace,
what it signs writes with, how module-owned wasm views mount into its tabs,
and how to build and sign a macOS release bundle.

## Repo DAG

Ducktape is split into five repositories under the `ducktape-industries`
organization:

```
ducktape-sdk  <-- ducktape            (the node: kernel, consensus modules, services)
ducktape-sdk  <-- ducktape-app        (this repo: the desktop shell)
ducktape-sdk  <-- ducktape-modules    (consensus module crates + their wasm components)
ducktape-sdk  <-- ducktape-views      (module-owned wasm views the app loads at runtime)
```

`ducktape-sdk` holds the contract every other repo builds against without
pulling each other in: the module SDK, the `sdk` crate, `view-wire` (the
host<->wasm-view wire), `design` (the palette and type scale a view and the
app draw with) and every module's `*-wire` crate (types-only payload/query/
reply shapes, no module logic). `ducktape-modules`, `ducktape-app` and
`ducktape-views` each depend on `ducktape-sdk` directly and never depend on
one another.

## Consumption

This repo is a cargo workspace of three path crates (`app`, `bin/app-launcher`,
`crates/gpui-notion`). Everything else it links comes in as a cargo git
dependency in the workspace root `Cargo.toml`:

- module wire crates, `view-wire` and `design` — `ducktape-sdk`, branch `dev`
- the node-side platform crates it links directly (`node`, `keystore`,
  `workspace-config`, `app-update`, `ducktape-home`, `authpage`,
  `run-envelope`, `module-artifact`, `media-service`, `ducktape-rpc`,
  `simnode`) — `ducktape-industries/ducktape`, branch `dev`

A build resolves those over the git protocol at the pinned branch; there is
no vendoring and no submodule.

## Gates

Before a merge to `dev`:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`, with `DUCKTAPE_VIEWS_DIR` at the staged views and `DUCKTAPE_MODULES_DIR` at the pinned core's sim-modules set
- `cargo test ax_contract`: every screen's accessibility tree, read headless (#114). `ax_contract_native` must pass; `ax_contract_views` (every staged view) runs with `-- --ignored` until wire epoch 9 gives editors a label.

## Test door (`ax`)

A QA runner reads and drives the app through its accessibility tree — the same AccessKit tree the OS gets (#114). The door is **off in every launch unless the environment asks for it**; a normal launch has no listener, no file and no way to open one.

- Open: launch with `DUCKTAPE_AX_DOOR=<port>` (`0` picks a free port). It binds `127.0.0.1` only (an address in the variable is refused), writes `{port, token}` to `$XDG_RUNTIME_DIR/ducktape/ax-door.json` (else the app's state directory) mode 0600, and logs one `ax_door_open port=…` line in `app.log`. Every request carries the token.
- Client: `ducktape-app ax tree [--window W] [--view V] [--compact] [--bounds]`, `ax actions`, `ax act <id> <press|focus|set_value|type|scroll_into_view> [value]`, `ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]`. Exit 0 answered, 1 not found / refused / timed out, 2 the door is not open.
- Ids are `<window>:<element id>` (`console:view:chat`), a view's `<window>:<module>/<wire key>`; never an index. Password fields' values and the recovery-phrase words read `•••`.
