# Architecture

The guide a new engineer reads first. It names code by directory, file and
symbol, never by line: the tree is about to be split and line numbers rot.
Every claim here was checked against `dev` on the day it was written; when
the code and this file disagree, fix one of them.

Companion documents: [README.md](README.md) (what the app is, how to run
it, the layout table), [docs/ax.md](docs/ax.md) (the accessibility plan),
[docs/perf.md](docs/perf.md) (the performance plan).

## 1. The app in one screen

The app is a **blind host**. It reaches a **node** (a `noded` process,
`/v1` over HTTP and one WebSocket), reads that node's **roster** (the
programs the network runs, each with a code blob), pulls each program's
**view** (a wasm module) out of its code blob, compiles it, seats it and
draws what it sends. Every screen inside the app's chrome is a view a
program shipped. The app ships no view of its own.

Three things cross between a view and the app, and only three:

- **The wire** (`view_wire`, aliased `wire` everywhere): the view sends a
  `Frame` holding an element tree of `Node`s, patches to the tree the host
  already holds, requests and cancels. The tree is named MessagePack
  (`rmp-serde`); the host sanitizes it and draws it with real GPUI controls.
  Events go back the other way as `wire::Event` values carrying the guest's
  own handler ids.
- **The kernel contract** (`src/runtime/kernel.rs`): the fixed set of
  methods every view may call, named `<capability>.<operation>`, request
  and reply types borsh in `wire::methods`. A view reaches only the
  capabilities its manifest declares (`wire::methods::Capability`, typed
  since #346). The app relays node methods to the connected node and
  answers host methods (`host.*`, `clock.ticks`, `clipboard.*`, `notify.*`,
  `store.*`) itself.
- **Props / session**: the session basics every view gets on its
  `host.session` subscription (`wire::methods::Session`: connected, dark,
  chain id, signer, account, endpoint), encoded by `runtime::props`.

What the app deliberately does not know:

- **No user program by name.** Panes, tabs and labels come from the roster
  (`Roster::rail`), a tab's name from the view's manifest. Three *system*
  programs are named by id and nothing else: `identity`
  (`backend/passkey.rs`, `backend/join.rs`: accounts and keys),
  `module-registry` (`backend/views.rs`: the roster query;
  `shell/menus.rs`: the Account menu opens it), and `valset`; all three
  also pick an icon on a folded menu bar (`shell/menubar.rs`).
- **No payload.** `module.query` and `op.submit` carry `Call{target, body}`;
  the app signs and forwards `body` and returns the program's bytes as
  they are. `module.describe` renders an op readable by running the
  *program's own* describe module, sandboxed.
- **No program state.** `Ducktape` (`src/ui/app.rs`) holds the node, the
  key, the account, which screen the console shows and where panes sit.
  What a person does inside a program lives in that program's view.
- **No private key in a view.** Writes are signed in `backend/session.rs`
  by the one seated key; a view never sees a key or a password.

GPUI is the gpui-pre fork, reached through the `gpui-kit` and `gpui-base`
crates; the wire's `Interactivity`, `Role` and `Aria` are view-wire's own
types (`style.rs`, `node.rs`, `aria.rs` upstream) shaped after GPUI's, so a
view's tree lowers onto GPUI elements one to one.

## 2. Map of `src/`

| Module | Owns | Runs on |
|---|---|---|
| `main.rs` | CLI flags (`--version`, `ax`, debug `--render-tree`), `app.log` + panic hook, fd limit, `DUCKTAPE_VIEWS_DIR`, then `shell::run` | process start |
| `shell.rs`, `shell/` | the native chrome: `Desktop` (the one model entity), `DesktopWindow` (one per OS window), `Command` effects, launcher, desk, menu bar, panes, overlays, keys, theme, fonts glue | window thread |
| `ui.rs`, `ui/` | `Ducktape` state, `AppMessage`, the reducer (`Ducktape::handle` → sub-reducers), the pure pane geometry (`ui/layout.rs`) | window thread (called from `Desktop::dispatch`) |
| `runtime.rs`, `runtime/` | seats, guests (wasmtime), the kernel relay, replies, notify, store, clipboard, roster, `Seat` | ticks on the window thread, off the draw path; loads on loader threads; node calls on `views-kernel` |
| `render.rs`, `render/` | `ViewTree`: one wire tree drawn as GPUI elements, retained native state keyed by `AuthoredPath`, `wire::Event` out, accessibility mapping | window thread |
| `editor.rs`, `editor/` | `EditorStore` (guest-owned documents projected natively) and `TextEditor` (the one multi-line native field; `editor/text.rs` is mounted as `editor::wire::text` by a `#[path]`) | window thread |
| `backend/` | everything that crosses the process boundary: `noded` client and signed `Frame`, `session` (seated key, prefs, recent nodes), `device_key`, `passkey`, `join`, `views`, `app_dirs` | async, polled where the caller polls (see below) |
| `a11y.rs` | role/name/keyboard/state helpers for shell and renderer, plus the class markers the AX door reads | window thread |
| `ax.rs`, `ax/` | the AX test door: loopback HTTP over the AccessKit tree, its CLI client | `ax-door` accept thread; answers on the window thread |
| `fonts.rs`, `tray.rs` | bundled faces and fallback chains; the macOS status item | window thread |

Dependencies, top down (an arrow means "calls into"):

```
main
 └─ shell ──────────► ui (state + reducer)
     │  │              └──► backend (connect, keys, prefs)
     │  │              └──► runtime (Roster, connected, notify)
     │  └─► runtime::Seat ──► render::ViewTree ──► a11y
     │          │                              └──► editor (TextEditor)
     │          ├─► runtime::guest (wasmtime) ──► editor::wire (EditorStore)
     │          └─► runtime::kernel ──► backend::noded / session / views
     ├─► a11y, fonts, tray, ax
     └─► ax ──► (reads the GPUI a11y tree of every window)
```

`ui/` never touches GPUI: it asks for native effects through
`shell::Command` (open, raise, close, swap console, sync appearance, open
URL, quit) and returns `view_wire::Task`s. `render/` never touches the
node. `backend/` never names a user program.

### Threads

| Thread | What runs there | Where it is made |
|---|---|---|
| **window / main** (GPUI foreground) | `Desktop::dispatch` and the whole reducer; every `DesktopWindow` render; `Seat::turn`, so every wasm **tick**; `ViewTree` layout and paint; AX door answers (`ax::serve`); the app's own `Task` futures (connect, status poll, sign-in) — `Desktop::start` polls them on the GPUI foreground inside `runtime.enter()`, so their HTTP bodies are decoded here | `shell/launch.rs` |
| **`views-kernel`** (one tokio current-thread runtime) | the I/O driver for every `reqwest`/WebSocket; view-originated node calls (`kernel/node.rs` `spawn`, `live`, `heads`); the banner queue (`notify::in_order`); its blocking pool runs `module.describe`, OS banners and device-key opening | `kernel::handle` |
| **one `std::thread` per view load** | fetch, verify, compile, instantiate, snapshot/restore (`Guest::load`) | `seat::spawn_load` |
| **roster read thread** | `/v1/programs` and the seat reconciliation on connect and on each new block | `roster::spawn_roster_read` |
| **`ax-door`** | TCP accept and HTTP parsing, one request per connection in turn (a long `/wait` holds the next caller); each request is forwarded to the window thread | `ax/http.rs` |
| **freedesktop listener** (non-macOS) | banner clicks off the session bus | `runtime/notify/freedesktop.rs` |
| GPUI background executor | timers only (clock wake-ups, sensor delays, spin, the door's settle polls) | — |

There is no wall-clock budget on a tick. Fuel is the one bound (see §8).

## 3. Life of a view

```
roster ─► blob ─► ducktape.view ─► compile ─► seat ─► tick ─► frame ─► render ─► input
 (node)  (cache)   (custom section)  (wasmtime)  (Slot)  (fuel)  (wire)   (ViewTree)  (events)
```

1. **Roster.** `ui/connect.rs` on `Connected` calls `runtime::connected`,
   which bumps `Connection.rev` and starts `roster::spawn_roster_read`.
   That thread calls `backend::views::programs` (the `module-registry`
   query), stores the list in the app's `Roster` (`roster::roster`, the
   one `Ducktape.roster` holds), retires seats of programs that left,
   creates a preloaded seat `(module, 0)` for each program and starts a
   load for every seat whose active code moved (or that was never
   asked of this node). Each new block (`Message::StatusPushed` with a moved
   height → `runtime::deployments_checked`) repeats it, one read in flight
   at a time. A load that finds the drawn view already current
   (`Loaded::Unchanged`) only calls `Guest::reconnect`, which refuses the
   tasks of a previous connection with `stale_connection`.
2. **Blob → section.** `backend::views::view_of` fetches the program's code
   blob (`/v1/blob/get`, a disk cache under `cache_dir()/programs/`),
   checks the hash, strips the git-style header (`unframe`) and returns the
   `ducktape.view` custom section (`views::VIEW_SECTION`) — or the whole
   blob for a **bare** entry (a view with no program behind it).
3. **Compile.** `Guest::load` on the loader thread: `DUCKTAPE_VIEWS_DIR`
   override first (`seat::view_override`, unverified, logged), else the
   network's bytes. Either way `Guest::compile` reads the manifest
   (upstream `view_wire::manifest::read_manifest`), checks its `wire_id`
   against `view_wire::WIRE_ID`, then `compiled_view` compiles through the one
   `Engine` (`guest::engine`: `consume_fuel(true)`, opt level Speed, a
   wasmtime disk cache under `cache_dir()/view-code`); `runtime::manifest_of`
   reads the same manifest again for the tab name and the capabilities.
   Compiled modules are cached by sha256 of the bytes in `ViewCodeCache`
   (16 modules, 32 MiB of source).
4. **Seat / mount.** A **seat** is `seat::Mounted`, one per
   `(module, instance)` in `seat::registry`, holding a `Slot`:
   `Loading → Fetching → Compiling → Ready(Guest) | Empty | Failed(Failure)`,
   and a `wake` (`watch::Sender<()>`) the loader signals when a load
   installs, a stage shows or a retry is asked. A pane's entity is
   `runtime::Seat` (`seat/entity.rs`), made by `Seats::reconcile`
   (`shell/entities/seats.rs`, called from `Desktop::dispatch`) for every
   view pane the model holds and keyed by the pane's `instance`; it is
   `place`d in the window whose layout holds the pane. `Seat::new` claims
   seat `(module, 0)` or makes a new one and subscribes to its `wake`.
   `Guest::instantiate` builds a store with `StoreLimits` (`MEMORY_LIMIT`)
   and binds `Exports` (`memory`, `alloc`, `init`, `tick`, `snapshot`,
   `restore`); `Guest::load` then runs `init` on a fresh view, or `restore`
   on a replacement.
5. **Turn / tick.** A seat is never stepped inside a draw. Every wake ends in
   one `Seat::turn`: a kernel reply (`Replies::changes`), a due
   `clock.ticks` item (a timer the turn re-arms), a `ViewTree` event, moved
   props (`Seats::set_props`, compared), the theme, the seat's `wake`
   (install, stage, retry) or a busy frame (`frame.busy` arms one
   `BUSY_FRAME` timer, never back to back). `turn` is entered at app level
   only: anything reachable from a window callback or `Desktop::dispatch`
   goes through `Seat::wake` = `cx.defer(turn)`, because `turn` updates the
   window itself. One tick per draw: a turn that ticked holds the seat
   until its tree has drawn (`render::Drawn`, sent by every render; a tick
   dirties the tree), and a turn asked for meanwhile runs right after the
   draw. So every frame the guest makes is drawn before the next, as when
   it ticked on the draw path: the first draw of a fresh view shows its
   first frame (the keys a pane hands its first control go by it) and a
   reply's chain of ticks advances one frame per draw. The AX door turns
   every seat before a read
   (`Seats::settle`), so the tree it draws has what the guest has answered,
   as the draw itself took in; the door's task yields to no reply wake
   between a press and its read.
   `turn` → `Guest::redraw`: it merges pending inputs, drains
   kernel `Replies`, fires due `clock.ticks`, syncs visibility, offset and
   route subscriptions, and — only if something is pending, the frame said
   `busy`, or there was never a first tree — calls `Guest::tick`: `arm`
   refills `FUEL_PER_TICK`, `Exports::tick` runs the wasm with the encoded
   events, `shape` decodes and bound-checks the returned `Frame`
   (`MAX_FRAME_BYTES`, request and cancel counts, sanitize), `merge`
   applies tooltip replies and patches onto the held root. A changed root
   bumps `frame_rev`. A frame carrying more than `MAX_REQUESTS_PER_TICK`
   requests (or twice that in cancels) is refused whole by `shape`, which
   faults the view; the `tick_limit` refusal in `redraw` is a second guard
   behind it. A patch `merge` refuses does not fault: the held root stays,
   `frame_rev` bumps and `Event::Resync` asks the guest for a full tree.
6. **Frame → render.** When `frame_rev` moved, `turn` hydrates picture
   hashes back to bytes (`Pictures::hydrate`), wraps the root in
   `native_root` (an id-less full-size container: giving it an id would
   shift every `AuthoredPath`) and either `ViewTree::replace`s the existing
   tree or, for a new guest instance, builds `ViewTree::new(root)
   .with_presentation(old.presentation())` and hands it the guest's
   `EditorStore`; the seat notifies, and the pane (`layers::PaneView`)
   draws `seat.tree()` cached (`layers::cached_unless_a11y`) inside the
   `view/<module>` mark, laid out from `seat.min_width()`, or its `Standin`
   while it holds one (a load, a failure, a stopped view), over any tree it
   keeps. `ViewTree::node` (`render.rs`) is the dispatcher: one
   method per `Node` variant, each building GPUI / gpui-kit elements.
   Native state that must outlive one frame (focus handles, field text,
   scroll offsets, list state, decoded images) lives in maps on `ViewTree`
   keyed by `AuthoredPath`, the chain of wire ids from the root down; id-less
   nodes share their nearest identified ancestor's path. `ViewTree::replace`
   (`render/commands.rs`) walks the new root and `retain`s each map to the
   paths it still mounts.
7. **Input back.** Element listeners (`render/interactivity.rs`,
   `render/inputs.rs`, …) `cx.emit(wire::Event)` with the guest's handler
   ids; `Seat` subscribes, drops focus-needing events when the
   pane is unfocused, takes the one-shot **user activation**
   (`ViewTree::take_user_activation`) onto `guest.user_activation` and calls
   `runtime::input::deliver` into `guest.pending`. Raw window input (pointer, wheel, keys, IME, file
   drops) arrives through `runtime/input.rs`'s `Observe` element after
   GPUI's own controls have seen it, marked `captured` if a native control
   consumed it. The multi-line editor is its own loop: `Node::Editor` mounts
   a `TextEditor` (`editor/text.rs`) whose edits become `EditorStore`
   transactions (`editor/wire.rs`); claimed key chords go to the guest for
   a decision; the guest's next frame acknowledges the revision.
   Imperative guest requests (`host.widget`: focus, scroll, cursor, editor
   action) are refused at once when their target is not in the tree the
   guest shows now (`invalid_widget_command`); admitted ones queue in
   `guest.widget_commands` until native editor work has drained
   (`runnable_widget_commands`: a plain `Focus` does not wait, a rich
   editor's does), then run in the pane's window one frame after the draw
   that mounts the tree (`Seat::run_widget_commands`: an `on_next_frame`
   inside an `on_next_frame`, since a next-frame callback runs before the
   draw), through `ViewTree::execute_widget_command`, re-checked for mount
   (`widget_unmounted`); the turn that carries the answers back is deferred.

**Hot swap.** When a block moves a program's active code, the load thread
prepares the new view as a replacement: instantiate without `init`, take
`old.snapshot()` (under the seat lock, fuel armed; only if the old view has
ticked at all, else the fresh one is `init`ed), `restore` it into the fresh
instance, verify its `first_frame`. An old view that is not `settled()`
(pending events, unanswered requests, editor work) refuses the replacement
("its replacement waits") and the next block tries again. `Loaded::Swap`
carries the old instance's `alive` token and tick count; `spawn_load`
installs it only if the seat's `generation` is still the one it was asked
for, the app is still on the node it was asked of (`Connection.rev`), and
the drawn instance is the one the snapshot came from, at the same tick
count and still settled. The new guest is `staged`: its first tree is
already in `frame`, so the next redraw routes its requests without a tick.
Pictures and editor projections move over (`EditorStore::
retain_restored_projections`). A new instance means a new `generation` on
`Seat`: handler ids are fresh, and only `NativePresentation`
(field text, selection, focus, scroll offsets, picture caches) carries
across into the new `ViewTree`. A snapshot the new code refuses falls back
to `init`.

**What fails how.** A seat that has no view holds a `Standin`
(`seat/standin.rs`, compared: it notifies only when the words move) that
the pane draws: the load's stage words while loading; the network's word
for `Slot::Empty` ("This network has no `<module>` view. An admin can
activate a deployment that ships one."); or a `Failure` with a
title and Retry — `Unreachable` (node or blob), `HashMismatch`,
`NotListed`, `Trapped` (fuel exhausted or a wasm trap, the panic message
from `HostState::panic` when there is one), `Refused` (bytes this build does
not run, or a load overtaken), `Wire` (built against another wire than
this app's). A failed candidate is held off
`RETRY_FIRST` doubling to `RETRY_MAX` while the same bytes keep failing
(`seat::Retry`). At run time a trap, an `EditorStore` fault, a frame past
`MAX_FRAME_BYTES` or a `Replies` overflow latch `guest.fault`: the view never
ticks again and the standin shows the message. `Fetching`'s byte counts are
written once at zero and never advanced.

## 4. Life of a request

```
view ─ Request{id, kind, payload} ─► Guest::answer ─► kernel::answer ─► handler
                                        │ capability gate                    │
                                        └─ host.widget/session/log locally   ▼
view ◄─ Event::Response{id, done} ◄── Replies (drained at next redraw) ◄─ views-kernel task
```

1. **Ask.** The view's `tick` returns requests in its `Frame`. `Guest::answer`
   (`runtime/guest/requests.rs`) caps the payload (`MAX_PAYLOAD_BYTES`;
   `MAX_OP_BYTES` plus the envelope for `op.submit` and `module.describe`),
   splits the kind with `Capability::of_kind` (a kind that does not split is
   `unknown_request`), and refuses `undeclared_capability` when the
   manifest did not declare it (logged once per capability).
2. **Route.** `kernel::answer(guest, capability, operation, id, payload)`
   tries `clipboard::answer`, `notify::answer`, `store::answer`, then its own
   arms: host methods (`host.visible`, `host.badge`, `host.route`,
   `host.offset`, `host.id`, `link.open`, `clock.ticks`) are answered at
   once from app state; node methods are spawned. Three kinds never reach
   the kernel and are handled in `Guest::answer` after it returns `false`:
   `host.widget` (the one MessagePack method: a `WidgetCommand`),
   `host.session` (the props subscription) and `host.log`.
3. **Relay.** `kernel/node.rs` `spawn` first checks the guest still belongs
   to the current `roster::connection()` (else `stale_connection`, or
   `not_connected` with no node), admits the call against `MAX_IN_FLIGHT`
   (`in_flight_limit`) and runs the handler as a tokio task on
   `views-kernel`, owned by `guest.tasks` as a `NodeTask` that aborts on
   drop (cancel, swap, teardown). Transport refusals (`rpc_client`,
   `node_failed`) retry with `backend::retry_delay` backoff for
   `NODE_RETRY_BUDGET` (60 s), then answer `rpc_client` with the
   `NODE_UNREACHABLE` sentence; a node's own refusal ends the call at once.
4. **Sign.** A read: `backend::query_frame` signs a `Frame` at seq 0 with
   the seated key, or with the process's throwaway `reader_key` while nobody
   is signed in; the program hears who asks, the node checks no sequence.
   A write (`op.submit`): `backend::seated_frame` asks the node for the
   signer's next sequence (`next_seq`: the `$signers` namespace,
   `Layer::Preconfirmed`), then `Frame::sign` builds `Body{scheme: Ed25519,
   signer, network, seq, target, payload}` and signs `abi::encode(body)`
   under `FRAME_NAMESPACE`; the bytes go to `/v1/submit`. With no seated
   key the request is refused `session_locked`.
5. **Reply.** The handler pushes into `kernel::Replies`, a per-view queue
   bounded by `MAX_REPLY_EVENTS` / `MAX_REPLY_BYTES`; overflow latches a
   fault that ends the view. `Seat` awaits `Replies::changes`
   and turns; `Guest::redraw` drains the queue into
   `guest.pending` as `Event::Response` values, so the answer reaches the
   view on its next tick. Nothing wakes the wasm from another thread.
6. **Subscriptions.** `module.changes <program>` (`live`: one WebSocket
   `/v1/changes/<program>` per subscription), `chain.heads` (`heads`: polls
   status and pages `/v1/blocks`), `clock.ticks`, `host.session`,
   `host.route`, `host.offset`, `host.visible` answer many times with
   `done: false`. While the reply queue is past half full
   (`MAX_STREAM_BACKLOG_*`) a subscription parks on the `drained` watch
   instead of adding items, so a slow view holds its own backlog on the
   socket. A cancel drops the task and with it the socket.

`module.describe` (`kernel/describe.rs`) is the odd one: it fetches the
program's current code blob, compiles the `ducktape.describe` section once
per code id, runs it on the blocking pool in an import-free fuel-bounded
instance, caches by (code id, op hash) up to `MAX_KEPT` answers, and
answers `None` on any failure but a transport one, which becomes
`rpc_client` and is retried like any node call.

## 5. The app's own state

`ui/` is the model, `shell/` is the view of it. The cycle:

```
shell control ─ AppMessage ─► Desktop::dispatch ─► Ducktape::handle ─► sub-reducer
                                   │                                     │
                                   ├─ mount (panes) ◄─── Task<AppMessage> ┘
                                   ├─ tray.sync            (polled by Desktop::start,
                                   ├─ start(task)           each yield re-dispatched)
                                   ├─ subscriptions (timers)
                                   └─ cx.notify() ─► every DesktopWindow re-renders
```

- **State.** `Ducktape` (`ui/app.rs`): appearance, `stage` (which screen
  the console shows: `Connect`, `Unlock`, `Phrase`, `Recover`, `Account`,
  `Desk`), the endpoint being typed and the recent ones, the node reached
  (`connected_rpc`, `network`, `chain`, `node`, `height`, `status_misses`),
  `keyring` (this network's key directory) and `other_chain`, `signer_key`
  and `account`, `sign_in` (the step's busy/error/approval state), `overlay`
  and Spotlight fields, `layouts` (per `WindowKey`, the panes), `active`,
  `badges`, the toast, window keys. Per-step secrets live inside the
  `Stage` variant and go with it.
- **Messages.** `AppMessage` is the one enum everything arrives as: shell
  clicks and keys, tray rows, `ViewEvent(module, Intent)` from views,
  timers (`Tick`, `ToastTick`), and the async results of the reducer's own
  tasks (`Connected`, `StatusPushed`, `DeviceKey`, `Unlocked`, `Joined`,
  `PasskeyDone`, …).
- **Reducer.** `Ducktape::handle` (`ui/update.rs`) routes by variant to
  `on_connect` (`ui/connect.rs`), `on_sign_in` (`ui/sign_in.rs`),
  `on_overlay` (`ui/overlay.rs`), `on_notify` (`ui/notify.rs`), `on_desk`
  (`ui/desk.rs`) and the pane reducer (`ui/panes.rs`); a message that crosses
  between launcher and desk batches `shell::swap_console`. Each returns a
  `view_wire::Task<AppMessage>` — the view SDK's task type reused for the
  host's reducer. `Ducktape::subscriptions` declares the recurring timers
  (wall clock, toast, status poll every `STATUS_EVERY`) as `Subscription`
  recipes the shell diffs.
- **Geometry.** `ui/layout.rs` is pure: `Layout` per window holds `Pane`s
  (frame, `instance`, `module`, z), focus and the desk size; operations
  (`split`, `cycle`, fill/restore, place, measure) are
  driven by `PaneMessage` through `ui/panes.rs`. Sentinel modules: `EMPTY`
  (an empty pane shows the program finder) and `HELP`.
- **Shell.** `Desktop` (`shell.rs`) owns `Ducktape`, the tray, the OS
  window handles, the `Seats` entity and the running task streams. It also
  holds `entities::Entities` (`shell/entities/`): the shell's state as
  compared slices the layers read — the session, the chain, the account,
  the screen, the rail's badges, the notification centre's counts, the
  toast, the prefs, the program in front, and per window its desk, what is
  open over it and Spotlight — written from `Ducktape` at the end of every
  dispatch by `shell/bridge.rs` until each one's own methods take its
  source over. Neither the rail's rows nor a window's front is bridged:
  the rows are refreshed off the roster's changes channel, and the front
  (`Front::of_desk`) is derived from its desk by an observer.
  `DesktopWindow::render` copies the fields a draw needs into `Facts`
  (`shell/screens.rs`) and picks by `Stage`: the launcher screens
  (`shell/launcher.rs` frame with a `spin` figure on the left; `screens.rs`
  Connect; `sign_in.rs` key, phrase, recover, account) or the desk
  (`shell/desk.rs`): `menubar.rs` across the top, `panes.rs` drawing each
  pane's seat (its tree or standin) or the app's own body (`layers::EmptyPane`:
  the bare desk's figure or an empty window's finder; `layers::HelpPane`), and one overlay
  at a time (`spotlight.rs`, `settings.rs`, `approve.rs`, `menus.rs`,
  `notifications.rs`). Native effects the reducer asks for travel as
  `shell::Command` through `commands()`'s channel to the pump in
  `shell/launch.rs` and `Desktop::execute`; `shell/windows.rs` decides where
  windows open. Keyboard shortcuts are GPUI actions bound in
  `shell/keys.rs` under key contexts the windows set.
- **Views in panes.** After every dispatch `Seats::reconcile`
  (`shell/entities/seats.rs`) reconciles the seats against `layouts`: a new
  view pane gets a `Seat`, placed in its window, whose `Intent`s route back
  as `Message::ViewEvent`; a gone pane's seat is told `hide`, its last
  intents routed the same way, and dropped. A pane keeps its seat when it
  pops out to its own OS window (`WindowKind::View`) and back; the
  `DesktopWindow` observes `Seats` to redraw when a seat moves.

## 6. Sign-in and keys

Two different things, one after the other. The **key** is this device's,
made per network and kept by the OS. The **account** is the network's
identity that key signs for; another key already on the account admits a
new one.

```
Connect ─► /v1/status ─► contract check ─► bind_keyring ─► device key ─► account? ─► Desk
```

1. **Connect** (`ui/connect.rs`). `ConnectTo(origin)` builds a
   `backend::RpcClient` and asks `/v1/status`. `Connected` refuses a
   `status.contract` other than `noded::NODE_CONTRACT`, then
   `backend::bind_keyring(network, founded)` picks the key directory under
   `<ducktape home>/remotes/`: `<network>`, or `<network>+<founded>` when
   another chain already took the name (`other_chain`; the
   `network-founded` file remembers). `take_up` makes the network current,
   the chain id becomes `ducklink::ChainId::of(network, genesis)`,
   `runtime::connected` starts the roster read, and two tasks run in
   parallel: `open_device_key` and `resolve_account`.
2. **Device key** (`backend/device_key.rs`, `ui/sign_in.rs`). On the
   blocking pool: `device_key::load(keyring)` from the OS store (Keychain,
   Credential Manager, Secret Service via the `keyring` crate; a 0600 file
   with `DUCKTAPE_DEVICE_KEY_STORE=file`), else `device_key::mint` an
   ed25519 key — unless a **legacy** password-locked keystore file is here
   (`key_exists`), in which case nothing is minted and the key screen asks
   for its password. `backend::seat_key` puts the key in the process-wide
   `SIGNER` and answers the public key (`signer_key`). No password, no
   phrase for a device key. The legacy file is opened once on
   `UnlockSubmit` and moved into the OS store (`device_key::save`). `Lock`
   calls `lock_signer` and returns to `Stage::Unlock`; `BrowseWithoutKey`
   opens the desk read-only.
3. **Account** (`backend/passkey.rs` `account_of_key` → `AccountResolved`).
   A key with an account goes to `Stage::Desk`; one without goes to
   `Stage::Account`, which offers:
   - **Create**: `passkey::create_plain_account` (name only) or the passkey
     path: `passkey::create_account` opens the auth page
     (`AUTH_PAGE`, override `DUCKTAPE_AUTH_PAGE`) in the system browser with
     the request in the URL fragment; the result comes back as a form POST
     to a one-shot loopback `Listener`. The identity program assigns the
     account number; the passkey then `AddKey`s itself with the device key's
     consent. `Phone` offers each touch as a QR URL and polls the auth
     host's relay slot `/r/<id>` instead. **Sign in with a passkey**
     (`PasskeySignInSubmit` → `passkey::sign_in`): an existing passkey
     asserts, then `AddKey`s this device's key to its account.
   - **Join from another device** (`backend/join.rs`): `new_code` shows a
     Crockford code; `join_from_device` posts the request to the relay and
     waits for consent. On a device already on the account, "Add a device…"
     (`shell/approve.rs`, `ApproveFind`/`ApproveConfirm`) calls
     `find_request`, shows `fingerprint` for both sides to compare, and
     `approve` signs the `Consent`.
   - **Recovery key**: `join_with_recovery_key` types an existing 24-word
     phrase whose key consents to this device's. `Stage::Phrase`
     (`RecoveryKeyStart` from the account menu) makes a new one with
     `new_recovery_phrase`, quizzes three words, then `add_recovery_key`
     adds it to the account.
4. **Signing in use.** From then on every write a view submits is signed by
   the seated key (§4). `Message::Lock` empties the seat; the reader key
   signs queries meanwhile.

Files: `ui/connect.rs`, `ui/sign_in.rs` (reducers); `shell/screens.rs`
(Connect), `shell/sign_in.rs`, `shell/approve.rs`, `shell/launcher.rs`
(screens); `backend/session.rs`, `backend/device_key.rs`,
`backend/passkey.rs`, `backend/join.rs` (the work).

## 7. Accessibility and the AX test door

- **Helpers** (`src/a11y.rs`): `Control::control(role, name)` (a role never
  without a name), `keyboard` (Tab stop, Enter/Space press, focus ring,
  pointer press does not steal focus), `focus_shown`, `Aria` (reach the
  accessibility setters of any interactive element), `text_field`,
  `disabled`, `modal`, and the class markers `AX_PRIVATE` (`private`: the
  door masks it) and `AX_WHOLE` (`whole`: the door does not truncate it).
- **Renderer mapping** (`src/render/accessibility.rs`): `Accessible` is what
  one wire node is to assistive technology (role, name, description,
  placeholder, value, disabled, and a field's invalid, required and
  read-only);
  `accessible(node)` builds it per node kind and `announce` writes it onto
  the element through `a11y::aria`, the three field states through one
  `a11y::Patch`. `ViewTree::guest_aria` is the one aria
  mapper for the nodes that carry the guest's own `Interactivity`
  (Container, Image, Svg, UniformList, List, ResizeHandle): gpui's setters,
  one `a11y::Patch` for the aria gpui has none for, `on_click`. A List the
  view roled or wired is a box in its place (gpui's list is not
  interactive). `ViewTree::presentation` and
  `with_presentation` (the `NativePresentation` copy across guest
  instances) also live in this file for now; the struct itself is in
  `render.rs`.
- **The door** (`src/ax.rs`, `src/ax/{http,tree,actions}.rs`). Off unless
  `DUCKTAPE_AX_DOOR=<port|0>` is set (0 picks a port). `ax::open`
  binds 127.0.0.1 only and writes `{port, token}` to
  `$XDG_RUNTIME_DIR/ducktape/ax-door.json` (else the state dir) mode 0600;
  the `ax-door` thread parses HTTP and forwards each `Request` over a
  channel to `ax::serve` on the window thread. Endpoints: `GET /tree`
  (`?compact`, `?bounds`, `?window`, `?view`), `GET /actions`, `POST /act`,
  `POST /key`, `GET /keys`, `POST /drag`, `POST /wait`, and `POST /reveal`
  only with `DUCKTAPE_AX_DOOR_PRIVATE=1`. Every read settles the seats
  (`ax::Door::settle`), draws the window and
  reads the same AccessKit tree GPUI hands the OS (`ax::tree::snapshot`
  over `Window::a11y_tree`); every act goes through GPUI's own a11y action,
  key or mouse dispatch (`ax::actions`). Ids are `<window>:<element id>`;
  windows are `console`, `console2`, … (`Desktop::ax_windows`); a view's
  nodes sit under the `view/<module>` mark `Seat::ax_mark`
  wraps around each view (`tree::VIEW_MARK`). Answers carry
  `X-Ax-Revision`, which moves when a tree changed. `ducktape-app ax …`
  (`ax::cli`) is the command-line client.
- **How qa drives it.** The qa repo's rig launches the app with
  `DUCKTAPE_AX_DOOR=0`, reads the door file, and walks scenarios through
  these endpoints; element ids such as `rail/<module>` and `pane/<n>/…` are
  its contract. In-repo, `shell/panes_tests.rs`, `shell/screens_tests.rs`
  and `render/tests/accessibility.rs` read `ax::snapshot` directly.
- **Caveat.** Activating the door switches the a11y tree on through the
  same flag an assistive technology sets, so the door proves the tree's
  shape, not that anything reached the OS. The plan is in
  [docs/ax.md](docs/ax.md).

## 8. Performance facts

- **Fuel.** `FUEL_PER_TICK` (`runtime.rs`, 250M instructions) is armed by
  `guest::arm` before every entry into a view: `init`, `tick`, `snapshot`,
  `restore`, and instantiate. There is no wall-clock or epoch budget.
  `RUST_LOG=ducktape::perf=debug` logs fuel used per tick, and
  `DUCKTAPE_PERF=1` keeps the counters `GET /perf` reads (docs/perf.md).
- **Limits** (`runtime.rs`, `kernel.rs`), as of this writing: `MEMORY_LIMIT`
  64 MiB per store, `MAX_FRAME_BYTES` 8 MiB (a bigger frame ends the view),
  `MAX_REQUESTS_PER_TICK` 256, `MAX_PAYLOAD_BYTES` 1 MiB, `MAX_OP_BYTES`
  16 MiB, `MAX_BLOB_BYTES` 16 MiB, `MAX_IN_FLIGHT` 256, `MAX_SUBSCRIPTIONS`
  256, `MAX_REPLY_EVENTS` 1024 / `MAX_REPLY_BYTES` 32 MiB and their
  half-size stream backlog; `layout::MAX_PANES` 8; a per-window SVG raster
  budget (`render/svg_limits.rs`); picture decode size limits
  (`render/picture_resources.rs`).
- **Window-thread work.** Every wasm tick, frame decode (`shape`), `merge`
  and `ViewTree::replace` run on the window thread inside `Seat::turn`,
  while holding the seat mutex, between draws and never inside one; GPUI
  layout and paint follow in the next frame, lock released. The
  loader thread takes that mutex for `snapshot` and for the install (not
  for compile, `restore` or `first_frame`), so a swap stalls the window for
  those two steps. The reducer's own `Task`s are
  polled on the window thread (HTTP bodies decode there). Every
  `Desktop::dispatch` ends in `cx.notify()`, and every `DesktopWindow`
  observes the model, so each message re-renders every window; the wall and
  toast timers alone make that several times a second. `Roster::rail()`,
  which locks every seat, runs once per roster or seat change
  (`entities::Rail`), not per render of a desk whose panes all hold
  programs; still per draw of Spotlight and Settings › Notifications while
  open, and of an empty pane (a bare desk, an empty window: its render and
  its model observer) until s9.
- **Off-thread.** View loads, roster reads, node I/O, describe, banners and
  key opening are off the window thread (§2).
- **Measured.** A `view_load` info line per network load
  (`seat::LoadTiming`) and the fuel debug line, both on `ducktape::perf`.
  With `DUCKTAPE_PERF=1`, `src/perf.rs` also times and counts the shell,
  each window and each view's load, tick and render stages, served at
  `GET /perf` and logged as a `perf_summary` line every 10 minutes and at
  quit ([docs/perf.md](docs/perf.md)).

## 9. Glossary

House words, and where one word means several things.

- **blind host** — the app: it hosts views without knowing the programs.
- **node** — the `noded` process the app talks to over `/v1`
  (`backend/noded.rs`). Not a `wire::Node`, which is one element of a
  view's tree. `kernel/node.rs`'s `Node` is the client + network pair a
  view's request goes to.
- **network / chain** — `network` is the bare label signed into frames;
  the **chain id** is `<network>#<salt>` from the genesis
  (`ducklink::ChainId`), used for links, the view store and the notify log.
- **contract** — `noded::NODE_CONTRACT`, the `/v1` version; a node with
  another is refused at connect. Also "the kernel contract" (§4).
- **roster / registry** — the node's program list
  (`backend::views::programs`, kept in a `Roster`: the app's one is
  `roster::roster`, and a test builds its own). "Registry" is
  also `seat::registry()`, the seat map. **bare** — a roster entry whose
  blob is the view itself.
- **module** — a program's name as the roster lists it, interned to
  `&'static str` (`runtime::intern`) and used as the view's key. Not a
  Rust module and not a `wasmtime::Module`.
- **view** — a program's UI: the wasm in its blob's `ducktape.view`
  section. **guest** — one running instance of it (`runtime::Guest`); "guest-owned" means the wasm decides it.
- **seat** — four meanings. (1) `seat::Mounted`: the per-`(module,
  instance)` slot a view is loaded into, in `runtime/seat.rs`.
  (2) the **seated key**: the signing key in `backend/session.rs`'s
  `SIGNER` (`seat_key`, `seated_frame`; `SignIn::seating` while it opens).
  (3) "unseated": a pane that left the desk for its own window
  (`shell/windows.rs`). (4) the element id `seat` of the pane area
  (`shell/desk.rs`).
- **slot** — `seat::Slot`, what a seat holds (Loading … Ready … Failed).
  Also the **slot mask** in `render/`: the pane box a view is clipped to,
  applied to tooltips (`tooltip_containment::SlotMask`).
- **generation** — the seat's load counter; a finished load installs only
  if it is still current, and `Seat.generation` names the
  guest instance it drew. Unrelated: the `generation` the identity program
  answers for a key (`passkey::generation`, `Reply::Generation`), which an
  `AddKey` names.
- **tick / redraw / turn** — `Guest::tick` is one wasm call; `Guest::redraw`
  is one host step (deliver, maybe tick, route requests); a **turn**
  (`Seat::turn`) is the seat's step, run on a wake and never by a draw.
- **frame** — three meanings. (1) `wire::Frame`: what a tick returns.
  (2) the signed node **frame** (`noded::Frame`, `seated_frame`,
  `query_frame`). (3) a GPUI frame.
- **snapshot** — three meanings. (1) the wasm `snapshot`/`restore` export
  pair a swap uses. (2) `ax::snapshot`: a read of a window's AccessKit
  tree. (3) `tray`'s rendered-menu snapshot.
- **presentation / NativePresentation** — host-side native state copied
  into a fresh `ViewTree` across guest instances.
- **authored path / AuthoredPath** — the wire ids of a node and its
  identified ancestors, root first: the key for all retained native state
  and for editor identity. **native_id** — the GPUI `ElementId` lowered
  from a wire id.
- **handler / message (wire)** — u32 ids the guest attaches to callbacks;
  the host echoes them in `wire::Event`. Fresh per guest instance.
- **user activation** — a one-shot mark that an event came from a real
  gesture: the renderer records the handler a pointer pressed
  (`ViewTree.user_activation`), `take_user_activation` matches it against
  the emitted event, tooltips forward it to their source tree. The widget
  copies it onto `guest.user_activation`, which `redraw` clears; nothing in
  the kernel reads it yet.
- **widget command** — a `host.widget` request acting on a native control
  (focus, next/previous, scroll, cursor, editor action); its **target** is
  an id suffix matched against mounted authored paths.
- **method / capability / operation** — a request kind is
  `<capability>.<operation>`; `Capability` is the typed part before the dot
  that a manifest must declare; `wire::methods` holds every request and
  reply type. Older docs said "door" for a method; today **door** means
  only the AX test door.
- **kernel** — the host-side router in `runtime/kernel.rs`, not an OS
  kernel and not the node. **Replies** — its per-view answer queue.
  **in flight / admit / settled** — a running request's slot against
  `MAX_IN_FLIGHT`. **backlog / park** — a subscription waiting because the
  queue is half full. **refusal** — a `wire::Error{code, message}` reply
  with a snake_case code.
- **intent** — what a view asked the app itself to do: `Intent::Badge`,
  `OpenLink`, `Notified`; handled in `ui/desk.rs`.
- **props / session** — the session facts every view gets on
  `host.session` (`wire::methods::Session`, built by `runtime::props`).
- **route** — three meanings. (1) the path part of a `duck://<view>/<route>`
  link, held in `runtime::route_to` until the view's first `host.route`
  subscriber (`take_route`). (2) `runtime::input::Route`: the
  (seat, generation, revision, alive) an input event is addressed to.
  (3) a closure-local name for a cloned authored path in `render/`.
- **link** — a `duck://` URL (`Roster::parse_link`, `Link`), or, in
  sign-in, "link from another device" (`join_from_device`). Unrelated.
- **standin / stage words** — the native placeholder drawn where a view is
  not (`seat/standin.rs`, `stage_words`). **Stage** (`ui/app.rs`) is which
  screen the console shows; `layers::PaneLayer` (`shell/layers/panes.rs`)
  draws the pane area. Unrelated.
- **override** — `DUCKTAPE_VIEWS_DIR`: a developer's `<module>_view.wasm`
  files replace the network's views, unverified, logged.
- **console** — the main OS window (`WindowKind::Console`, `console_win`):
  the launcher first, then the desk. Also the AX door's window names
  `console`, `console2`, ….
- **launcher** — every screen before the desk (Connect, key, phrase,
  recover, account), in the console at launcher size; `in_launcher()`.
- **desk** — the area under the menu bar where panes float
  (`ui/layout.rs`); also `Stage::Desk` and the `desk` key context.
- **pane / window** — `layout::Pane` is one floating frame on the desk
  holding a view, the program finder or Help. User copy, GPUI actions and
  `layout.rs` comments call it a **window**. **window** therefore means
  three things: an OS window (`DesktopWindow`, `WindowKey`,
  `WindowKind`), a pane, and the view-wire `events::Window` events
  (`Focused`, `CloseRequested`, `Closed`, …) a guest receives.
- **instance** — a pane's unique u64 (`Pane.instance`), the key for
  `Seats`; `Seat.instance` is its own counter.
- **split / cycle / fill / measure** — pane geometry operations in
  `ui/layout.rs`. `split` adds a pane.
- **rail / RailRow** — the roster-ordered program list the menu bar shows
  as tabs (`Roster::rail`, read into `entities::Rail` once per change the
  roster's channel reports). The name is from an older side rail and
  survives in AX ids (`rail/<module>`, `rail-search`) that qa depends on.
- **overlay / popover / scrim** — the one thing open over the desk
  (`Overlay`: Spotlight, Approve, Settings, Network, a bar menu); a
  **popover** hangs under a bar button; a **scrim** dims the desk and makes
  the overlay modal.
- **Command** — two meanings. (1) `shell::Command`: a native effect the
  reducer asks the window thread for. (2) The command line: the empty
  pane's program-finder field and its `Mode` (Module | Chat), in
  `shell/layers/empty_pane.rs`.
- **Task / Subscription** — `view_wire::Task` and `Subscription`, the view
  SDK's types reused by the host reducer.
- **ink / Ink** — the design palette per appearance and the widget kit the
  shell screens are built from (`shell/ink.rs`); `ink.ink` is the text
  colour. **board / canvas** — the external design mockups the screens were
  ported from; not GPUI's `canvas()`.
- **figure / spin** — a **figure** is an ASCII-art 3D drawing
  (`shell/figure.rs`); **spin** is the widget that tumbles and drags it
  (`shell/spin.rs`). In `runtime/kernel.rs` "spin" means busy per-frame
  work on the window thread.
- **keyring** — two meanings. (1) `Ducktape.keyring` / `session::Keyring`:
  the *directory name* under `<ducktape home>/remotes/` holding this
  network's keys (`<network>` or `<network>+<founded>`). (2) the `keyring`
  crate: the OS credential store `backend/device_key.rs` uses.
- **device key / seated key / reader key / legacy key** — this device's
  per-network ed25519 key in the OS store; the one loaded into `SIGNER`;
  the throwaway key that signs queries while locked; a password-locked
  keystore file from before keys moved into the OS.
- **passkey / touch / ceremony / auth page / relay slot / Phone** — an
  account key held by an authenticator; one WebAuthn step; the browser
  page that runs it (`AUTH_PAGE`); the auth host's one-shot mailbox
  `/r/<id>`; the phone path that polls it.
- **notice / banner / centre / tag / burst / permission / in front** —
  what a view posts (`notify.post`); the OS banner the host may raise; the
  per-chain log (`notify::Center`); a view-chosen fold key; the per-view
  token bucket (`BURSTS`); the person's Allow/Silent/Block choice; the
  view in the active window, which gets no banner.
- **door file / rig / walk / reveal / mask / settle / compact / offer /
  delta / nearest** — AX-door words: the `{port, token}` file; the QA
  harness launching the app; one QA scenario run; the private-mode read of
  a masked node; `•••` over `AX_PRIVATE` text; polling until the tree stops
  changing after an act; nodes with a name, value, state or action; one
  (id, action, label) triple from `/actions`; what an act changed; near
  ids returned with a 404.
- **editor words** — **projection** (what a mounted editor reads from the
  store), **field** (one mounted `Node::Editor` by `AuthoredPath`),
  **document / reset / revision** (the guest-owned text, its generation,
  its commit count), **claim** (a chord the guest wants first),
  **decision** (the guest's answer to a claimed key), **pump**, **mirror**,
  **fault** (a sticky error that stops the store and faults the view).
- **pictures / hash-only frame** — the per-guest image and SVG byte cache
  (`runtime/pictures.rs`): bytes cross once, later frames name them by
  hash; `adopt` remembers, `hydrate` fills back.

## 10. Where to start

- **Add a kernel method.** The request and reply types live upstream in
  `view_wire::methods` (the modules repo); bump the pin in `Cargo.toml`.
  Then: a new operation under an existing capability gets an arm in
  `kernel::answer` (or in `clipboard`/`notify`/`store::answer` if it
  belongs there); a node-backed one gets a handler in `kernel/node.rs` and
  is spawned with `spawn`/`spawn_once`, answering through `Replies`; a
  subscription writes through `Items`. Add the method to the module doc in
  `kernel.rs` and a case in `kernel/tests.rs`, which stands up a loopback
  `TcpListener` as the node.
- **Add a native screen or overlay.** State and messages in `ui/app.rs`
  (a `Stage` variant or an `Overlay` variant), the reducer arm in the
  matching `ui/*.rs`, the drawing as a `DesktopWindow` method in `shell/`
  built from `ink` pieces, routed from `DesktopWindow::render` (stages) or
  `shell/desk.rs` (overlays). Give every control a role and name through
  `a11y` and an element id the door can address; add a walk to
  `shell/screens_tests.rs` or `panes_tests.rs`.
- **Debug a view that will not load.** Read `app.log` (path from
  `backend::app_log_path`; state dir): every load logs one
  `view_source` line per outcome with stable fields, plus a `view_load`
  timing line once a fetch began, and the standin's title names the
  `Failure`. Check in order: is the program in
  the roster (`NotListed`); does the node hold the blob (`Unreachable`);
  does the section exist (`Slot::Empty`); do the bytes hash
  (`HashMismatch`); does the manifest parse and name this app's `WIRE_ID`
  (`Refused`, `Wire`); did `init` or the first tick
  trap (`Trapped` — a panic message is in the sentence). To bypass the
  node, point `DUCKTAPE_VIEWS_DIR` at a directory of `<module>_view.wasm`.
  `RUST_LOG=ducktape::perf=debug` shows per-tick fuel.
- **Run against a node.** Start a node from `ducktape-industries/ducktape`
  (the branch README names), then `cargo run -p ducktape-app`;
  `DUCKTAPE_RPC=<url>` skips the connect screen. Keys go to the OS store;
  `DUCKTAPE_DEVICE_KEY_STORE=file` and XDG variables point a scratch run
  elsewhere (`backend/app_dirs.rs`).
- **Run the AX door.** `DUCKTAPE_AX_DOOR=0 cargo run -p ducktape-app`, then
  `ducktape-app ax tree` (or `actions`, `act <id> <action>`, `key`, `keys`,
  `drag`, `wait`); the client reads the door file for port and
  token. `DUCKTAPE_AX_DOOR_PRIVATE=1` adds `reveal`.
- **Node-less screenshots.** `dev/screens/chat-screens.sh FIXTURES OUT`
  builds the debug binary and, per fixture in `manifest.json`, runs
  `ducktape-app --render-tree` (`shell/fixtures.rs`: a `ViewTree` in a
  window, no node, no model) under Xvfb and captures a PNG. Linux only; see
  `dev/screens/README.md`.
