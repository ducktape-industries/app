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
- `python3 -m unittest discover ops/qa`: the QA walk runner against a fake door and a fake judge

## Test door (`ax`)

A QA runner reads and drives the app through its accessibility tree — the same AccessKit tree the OS gets (#114). The door is **off in every launch unless the environment asks for it**; a normal launch has no listener, no file and no way to open one.

- Open: launch with `DUCKTAPE_AX_DOOR=<port>` (`0` picks a free port). It binds `127.0.0.1` only (an address in the variable is refused), writes `{port, token}` to `$XDG_RUNTIME_DIR/ducktape/ax-door.json` (else the app's state directory) mode 0600, and logs one `ax_door_open port=…` line in `app.log`. Every request carries the token.
- Client: `ducktape-app ax tree [--window W] [--view V] [--compact] [--bounds]`, `ax actions`, `ax act <id> <press|focus|set_value|type|scroll_into_view> [value]`, `ax wait [--role R] [--name N] [--state S] [--in W[/V]] [--gone] [--deadline-ms MS]` (one wait answers within 60 s), `ax key <keys> [--text T] [--window W]` (`POST /key`: keystrokes such as `tab`, `shift-tab`, `enter`, `ctrl-k`, then text, through the app's own key dispatch, down then up — never an OS event), `ax keys` (`GET /keys`: the GPUI bindings reachable from focus and the chords the seated views claim). Exit 0 answered, 1 not found / refused / timed out, 2 the door is not open.
- Ids are `<window>:<element id>` (`console:view:chat`), a view's `<window>:<module>/<wire key>`; never an index. Password fields' values and the recovery-phrase words read `•••`.
- Private reveal (test rig only): so an unattended walk can confirm the recovery phrase, a launch that sets `DUCKTAPE_AX_DOOR_PRIVATE=1` as well as `DUCKTAPE_AX_DOOR` adds `POST /reveal {id}` (`ducktape-app ax reveal <id>`) and logs `ax_door_private=on` in `app.log`. It answers the unmasked name and value of ONE showing node marked private — text a person reads on the screen, like the phrase words. A secure input is refused (403): its dots are all anyone sees. Without the variable the endpoint does not exist (404). `tree`, `actions`, `act` deltas and `wait` stay masked either way. Never set it outside a QA rig.

## QA walk (`ops/qa/walk.py`)

`python3 ops/qa/walk.py <scenario.json>… --out <dir> [--params file.json] [--param k=v …] [--keep-going] [--keyboard]` walks the app through the door: several scenarios run in turn on one rig, each after the preludes it `include`s. The regression suite, its params file, output and cost: [`ops/qa/README.md`](ops/qa/README.md). Plain Python 3 stdlib. Exit 0 PASS, 1 FAIL, 2 FAIL-UNJUDGED, 3 a bad scenario or command line.

- Scenario: `{name, params: {name: doc}, rig: {display, env, secrets}, steps: [{say, expect?, kind?, …}]}`. `{param:k}`, `{rig:dir|home|config|data|state|run|ducktape_home}` and `{secret:name}` fill argv and text. Kinds:
  - `ui` (default): Jev `choice` over the door's closed action list (`<id> <action> <label>` + "none of these") → act (text from the step's `text` or a rig `secret`, never from the model) → Jev `noul` on the delta and the tree against `expect`. Passes at ≥ 0.7, else one retry after `settle_ms` (after the step's `wait` if it has one). An answer outside the list leaves the step unjudged.
  - `wait` (`wait: {role, name, state, in, gone}`, `deadline_ms`; the runner asks the door again until its own deadline), `shell` (fixed `argv`, no shell; `expect_exit`, `expect_output` regex; `background` keeps it running), `launch` (`argv`, waits for the door), `stop`: no model call.
  - `private_remember` (`match: {role, name, in, ids_prefix}`, `as`) and `private_copy` (`from`: ids or matchers, or `from_indexed: {memory, prompt: <matcher>, regex}` — the numbers the visible prompt asks for, in its order; `into`: a secure input only; `how`: `type` | `set_value`): only with `rig: {private: true}`, which sets `DUCKTAPE_AX_DOOR_PRIVATE=1`. The revealed text is held in memory and goes only back into the app: never into a Jev request, the transcript, the ledger or any file. The transcript says `private_copy from <source> into <id>: <n> chars`.
- Rig: private HOME/XDG/`DUCKTAPE_HOME` under `<out>/rig`, its own Xvfb on a free display, `DUCKTAPE_AX_DOOR=0`. Every child starts in its own session and is recorded in `rig/pids.json` (pid, exe, start time). Teardown sends SIGTERM to a recorded process group only while `/proc/<pid>/exe` is under the rig (or is the rig's own Xvfb) and the start time matches; nothing else is ever signalled.
- Secrets: `{"file": path}` or `{"generate": "password"}`, resolved at run time. Every file the runner writes shows them as `{secret:name}`.
- What leaves the machine: only the step text and the door's masked compact tree and delta, sent to `api.typesafe.ai` with `JEV_API_KEY` from the environment. Any judge or door error makes the step unjudged (fail closed).
- Output, in a new `<out>/run-NN/` per invocation: `transcript.jsonl` (one line per step), `failing-tree-<scenario>-<step>.json`, `ledger.jsonl` (one line per Jev call: tokens, USD at $0.042/M), `result.json` (per-scenario verdicts and failures).
- Scenarios: `ops/qa/scenarios/launch-window.json` (no network: `--param app=<ducktape-app>`); `setup.json` (the live install-and-join prelude; its params are listed at the top of the file) and `reopen.json`; `seq4-acceptance.json`, `registry-views.json` (every rail tab and the palette off the registry) and `update-path.json` on that prelude; `suite/*.json`, one per surface.
