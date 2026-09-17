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
