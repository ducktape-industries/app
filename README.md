# ducktape-app

The native GPUI desktop shell for a ducktape workspace: `app/` (the
`ducktape-app` binary), `bin/app-launcher/` (its boot-time flip/rollback/exec
executor) and `crates/gpui-notion/` (the Notion-shaped block editor it
embeds for Pages). See `app/README.md` for how the app finds a workspace,
what it signs writes with, how module-owned wasm views mount into its tabs,
and how to build and sign a macOS release bundle.

## Repo DAG

Ducktape is split into six repositories under the `ducktape-industries`
organization (ducktape-qa is private):

```
ducktape-sdk  <-- ducktape            (the node: kernel, consensus modules, services)
ducktape-sdk  <-- ducktape-app        (this repo: the desktop shell)
ducktape-app  <-- ducktape-qa         (private: the Jev walk runner + acceptance scenarios; pinned here by ops/qa/RUNNER_REV)
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

## Install

`make install` builds the release app and launcher, installs both entrypoints
under `${CARGO_HOME:-$HOME/.cargo}/bin`, and runs the shipped launcher installer.
On Linux it also installs the managed desktop entry and `ducktape.svg` under
`${XDG_DATA_HOME:-$HOME/.local/share}`; `DUCKTAPE_INSTALL_DIR` is not used for
that layout. On macOS the existing bundle script builds and ad-hoc-signs
`Ducktape.app`, which is installed under `$HOME/Applications` by default (or a
non-empty `DUCKTAPE_INSTALL_DIR`). The target honors `CARGO`,
`CARGO_TARGET_DIR`, `CARGO_BUILD_JOBS`, and the XDG variables; it never uses
`sudo`.

An install lays down a clean set: it keeps nothing of the install it finds (no
rollback copy, no staged release) and never refuses because of it, so running it
again is always safe. Updating an installed app is the launcher's other job and
starts from the state an install leaves.

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
- `cargo test ax_contract`: every screen's accessibility tree, read headless (#114). `ax_contract_native` and `ax_contract_views` (every view in `DUCKTAPE_VIEWS_DIR`: the deployed set, wire epoch 10, with no exemption) must pass.
- `ops/qa/run.sh --check`: every suite scenario (`ops/qa/scenarios/suite/*.json`) loads through the QA runner at the pinned `ops/qa/RUNNER_REV`. The runner's own unit tests run in ducktape-qa.

## Test door (`ax`)

A QA runner reads and drives the app through its accessibility tree — the same AccessKit tree the OS gets (#114). The door is **off in every launch unless the environment asks for it**; a normal launch has no listener, no file and no way to open one.

- Open: launch with `DUCKTAPE_AX_DOOR=<port>` (`0` picks a free port). It binds `127.0.0.1` only (an address in the variable is refused), writes `{port, token}` to `$XDG_RUNTIME_DIR/ducktape/ax-door.json` (else the app's state directory) mode 0600, and logs one `ax_door_open port=…` line in `app.log`. Every request carries the token.
- Client: `ducktape-app ax tree [--window W] [--view V] [--compact] [--bounds]`, `ax actions`, `ax act <id> <press|focus|set_value|type|scroll_into_view> [value]`, `ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]` (one wait answers within 60 s), `ax key <keys> [--text T] [--window W]` (`POST /key`: keystrokes such as `tab`, `shift-tab`, `enter`, `ctrl-k`, then text, through the app's own key dispatch, down then up — never an OS event), `ax keys` (`GET /keys`: the GPUI bindings reachable from focus and the chords the seated views claim). Exit 0 answered, 1 not found / refused / timed out, 2 the door is not open.
- Ids are `<window>:<element id>` (`console:view:chat`), a view's `<window>:<module>/<wire key>`; never an index. Password fields' values and the recovery-phrase words read `•••`.
- Private reveal (test rig only): so an unattended walk can confirm the recovery phrase, a launch that sets `DUCKTAPE_AX_DOOR_PRIVATE=1` as well as `DUCKTAPE_AX_DOOR` adds `POST /reveal {id}` (`ducktape-app ax reveal <id>`) and logs `ax_door_private=on` in `app.log`. It answers the unmasked name and value of ONE showing node marked private — text a person reads on the screen, like the phrase words. A secure input is refused (403): its dots are all anyone sees. Without the variable the endpoint does not exist (404). `tree`, `actions`, `act` deltas and `wait` stay masked either way. Never set it outside a QA rig.

## QA walk (ducktape-qa, pinned)

The walk runner and the acceptance scenarios live in the private repo [ducktape-industries/ducktape-qa](https://github.com/ducktape-industries/ducktape-qa) (moved from `ops/qa` at 8f74e92a); its README has the scenario format, the rig, what leaves the machine, output and cost. The runner drives the app only through the test door above. This repo keeps the per-surface suite (`ops/qa/scenarios/suite/*.json`) and pins the runner by rev in `ops/qa/RUNNER_REV`.

`ops/qa/run.sh <scenario.json>… --out <dir> [--params file.json] [--param k=v …] [--keep-going] [--keyboard]` checks out ducktape-qa at exactly that rev (under `${XDG_CACHE_HOME:-~/.cache}/ducktape-qa/<rev>`; a dirty or other-rev checkout is refused) and runs its `walk.py`. Suite scenarios include `setup.json`, which the runner takes from its own `scenarios/acceptance/`. How to run the suite: [`ops/qa/README.md`](ops/qa/README.md).
