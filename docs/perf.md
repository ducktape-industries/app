# Performance tracking: views and the shell

A design for continuous performance tracking of (a) the wasm view modules
the app seats and (b) the app shell itself. One mechanism for both, cheap
when off, gateable from qa through the AX door.

Code is referenced by file and symbol, never by line: the tree is about to
be split. Every claim about the code was checked against `origin/dev` at
d147bfab (#346) and re-checked against #347 (`193c9a21`, dev 38cecdee), the
window-switch fix whose measurements set the baselines in §4; a second,
adversarial pass re-read every symbol named here against d147bfab. Where a
claim comes from another session's measurement it says so.

---

## 1. Goals and non-goals

**Goals**

- One switch, `DUCKTAPE_PERF=1`. Off, every hook is one relaxed atomic load;
  no `Instant::now`, no lock, no allocation. Users and qa run the same
  binary.
- One registry in the process, keyed by `(view, stage)` and `(window,
  stage)`, holding counters, gauges and small histograms.
- One tracing target, `ducktape::perf`, for the per-event stream. It goes
  through the app's one `fmt` file layer (`src/main.rs` `install_log`), so
  there is no second log format. The existing `ducktape::fuel` target is
  renamed into it, with no alias.
- One door endpoint, `GET /perf`, on the AX door (`src/ax/http.rs`), same
  token, same loopback bind. It returns the registry as JSON and must not
  draw a window.
- One summary line in app.log, so a user's log tells the story without the
  door.
- Deep dives through wasmtime's own profiler hooks, dev-only, switched on by
  environment, no rebuild.

**Non-goals, and the dependency question**

- **No new crate in the core mechanism.** The registry is stdlib: a
  `Mutex<BTreeMap>` of fixed-size sample rings. `serde_json` is already a
  dependency for the door's replies.
- **`hdrhistogram` via `gpui-kit/profiler` is the one candidate dependency,
  and phase 0 does not take it.** The gpui-pre fork already carries a frame
  profiler (`WindowProfiler`, the foreground journal, `HangDetector`) behind
  its `profiler` cargo feature; `gpui-kit` 0.6.1 exposes it as
  `profiler = ["gpui/profiler"]`. The app does not enable it: `hdrhistogram`
  is absent from `Cargo.lock`, and in the fork `window_profiler` is a
  `#[cfg(feature = "profiler")]` field on `Window`. Turning it on is not
  zero-cost when off: the journal allocates its ring at `App` build, and the
  draw, input and task-poll paths take timestamps whenever the feature is
  compiled in. It is the only way to get present-time and gpui-level
  long-frame causes, so it is an owner question (§7), not a silent pick.
- **Not measured from inside the guest.** A view has no clock: its only wasm
  import is the panic hook the host binds in `Guest::instantiate`
  (`src/runtime/guest/lifecycle.rs`), and `clock.ticks` items carry an empty
  payload (`kernel::ticked`, `src/runtime/kernel.rs`). Every timing here is
  host-side. Guest-side counters could travel through the `host.log` request
  at one request per tick; fuel, frame bytes and the `Frame` fields already
  cover the continuous signal, so this design does not ask for it.
- **Not a per-frame line in app.log.** The file writer is a `Mutex<File>`
  written by whichever thread logs; a line per tick per view would be a
  write syscall per frame on the window thread.
- **No tracy, puffin, `profiling` backend or `--cfg ztracing`.** The fork
  has `#[profiling::function]` on draw, present and dispatch, with no backend
  in the lock; `profile-with-tracing` would also turn wgpu's scopes into INFO
  spans the app's `info` default lets through.
- **Not gating on qa's own timings.** qa records a per-step `ms` and a
  per-run `wall_time_s`; both include judge time and door round-trips. They
  are not app performance.

---

## 2. Stages

Legend for "held": which session has the file in flight (see §5). `70` is
ducktape-70 (window-switch perf), `a3` is ducktape-a3 (view snapshot fix
and dead wire-variant removal). Metric types: **C** counter, **H**
histogram (µs unless said), **G** gauge. Class **D** is deterministic for
the same inputs and can be gated; **W** is wall clock and is reported only
under Xvfb.

### 2.1 One view's life

Threads: a load runs on its own OS thread (`seat::spawn_load`); the tick
loop runs on the window thread inside `NativeModuleView::frame`
(`src/runtime/widget.rs`), which holds the seat mutex while it runs.

| Stage | Measured today | Hook point | Metric | Held |
|---|---|---|---|---|
| Roster read (per block) | nothing | `roster::spawn_roster_read` around `backend::views::programs` | H `roster` (W) | free |
| Fetch the blob | `LoadTiming.fetch` (`seat.rs`), logged by `LoadTiming::log` as `fetch_ms`; disk-cache hit and node read are not told apart | `Guest::load` (`guest/lifecycle.rs`) around `backend::views::view_of`; source from `backend::views::program_bytes` (cache path vs `client.blob`) | H `fetch` (W), C `fetch.source{disk,node}` (D), G `view_bytes` (D) | a3 for the wall time in `Guest::load`; free for source and bytes in `program_bytes` (`backend/views.rs`) |
| Compile: memory hit / disk-cache hit / cold | `LoadTiming.compile` as `compile_ms`, one number for all three | `Guest::compile` → `guest::compiled_view` (owns the static `ViewCodeCache`) → `guest::compile_view`: in-process cache hit before `Module::new`; disk vs cold from a `wasmtime::Cache::cache_hits`/`cache_misses` delta around `Module::new`. `guest::engine` moves its `Cache` into `config.cache(..)`; `Cache` is `Clone`, so keep a clone beside the `Engine` to read the counters. They are process-global and loaders compile concurrently on purpose, so the delta is approximate — `ponytail:` note | H `compile` (W), C `compile.source{memory,disk,cold}` (D) | a3 (`guest.rs`) |
| Instantiate | lumped into `init_ms` | `Guest::instantiate` (`guest/lifecycle.rs`): `Linker::instantiate` plus `Exports::bind`; fuel is armed there too, so `FUEL_PER_TICK - get_fuel()` after it is the start function's fuel | H `instantiate` (W), H `fuel.instantiate` (D) | a3 |
| Snapshot of the old view (swap only) | lumped into `init_ms`, and runs under the seat mutex the window thread needs | `Guest::load` where it calls `old.snapshot()` (`Guest::snapshot` → `Exports::snapshot`) | H `snapshot` (W), G `snapshot_bytes` (D), H `fuel.snapshot` (D) | a3 |
| Restore | lumped into `init_ms` | `Guest::restore` → `Exports::restore` | H `restore` (W), H `fuel.restore` (D) | a3 |
| Init | `init_ms` = instantiate + snapshot + restore + init together | `Guest::init` → `Exports::init` | H `init` (W), H `fuel.init` (D) | a3 |
| First frame of a swap | `first_frame_ms` | `Guest::first_frame` | H `first_frame` (W) | a3 |
| Install | nothing: `LoadTiming::log` runs before the loader takes the seat lock in `spawn_load` | `seat::spawn_load`, the closure after `Guest::load` returns: lock wait, then the match on `Loaded` through `locked.changes.send_replace(())`; a swap also drops the old `Guest` (its `Store`, up to `MEMORY_LIMIT` of memory) under the lock | H `install.lock_wait`, H `install` (W) | free |
| First tree of a fresh view | nothing. A fresh load only calls `init`; the first `tick` runs on the window thread at the first redraw, after `view_load` was logged. Seats are preloaded by `spawn_roster_read` before any tab shows them, so "from load start" is not what a user feels | end of the first `Guest::tick` (`Guest.ticks == 0` in `Guest::redraw`), measured from `Mounted.shown` (set in `NativeModuleView::frame`) or from install end if later | H `first_tree` (W, ms) | a3 / 70 |
| Tick: fuel | `tracing::debug!(target: "ducktape::fuel", used, limit)` in `Guest::tick` (`guest/requests.rs`) | same site; rename the target to `ducktape::perf` | H `fuel.tick` (D) | a3 |
| Tick: wall | nothing | `Guest::tick`: the `arm` → `Exports::tick` → `shape` chain is one expression today and has to be split to time the call and the decode apart | H `tick.call`, H `tick.decode` (W) | a3 |
| Requests per tick | nothing (only the `MAX_REQUESTS_PER_TICK` refusal) | `Guest::redraw`, the loop over `frame.requests` and `frame.cancels` | H `requests`, H `cancels` (D) | a3 |
| Host-call latency and attempts | nothing (a warn when the 60 s retry budget runs out, `node::until_answered`) | `kernel::node::spawn_call`: `Instant` before the spawned future, record when `replies.item` is called; attempts from `until_answered`; the kind is the `(Capability, operation)` pair `kernel::answer` matched on. Also `node::spawn_device` | H `host_call.<kind>` (W), C `host_call.<kind>.attempts` (D) | free |
| Reply backlog at drain | nothing | `Replies::drain_into` (`kernel/replies.rs`): `events.len()` before the append | H `backlog` (D) | free |
| Frame bytes | nothing (the `MAX_FRAME_BYTES` refusal in `guest::shape`) | `Exports::tick` answer length; `wire::encode(&events)` length in `Guest::tick` | H `frame_bytes`, H `events_bytes` (D) | a3 |
| Frame kind, busy, node count | nothing | after `guest::shape` in `Guest::tick`: `frame.root.is_some()`, `frame.patches.len()`, `frame.unchanged`, `frame.busy`; the node count from a `root.for_each_mut` walk (view-wire has no `count`), only when `frame_rev` bumps (O(n), so only when on) | C `frame.{full,patch,unchanged}`, C `busy_ticks`, G `nodes` (D) | a3 |
| Sanitize truncations | a `display_text_truncated` warn (`Guest::report_display_truncation`) | `reports.local` after `guest::shape` | C `truncations` (D) | a3 |
| Tree merge and replace | nothing | `guest::merge` (patch frames clone the held tree before applying); `NativeModuleView::frame`, the `changed` branch: root clone, `Pictures::hydrate`, `native_root`, `ViewTree::replace` (`src/render/commands.rs`) | H `merge`, H `replace` (W) | a3 / 70 |
| ViewTree render | `ViewTree.renders`, `#[cfg(test)]` only (`src/render.rs`) | `ViewTree::render` (`Render` impl); promote the counter out of `cfg(test)` | C `renders` (D), H `render` (W) | 70 |
| Cache hit / miss per view | nothing | On the cached path (`NativeModuleView::render`, the `.cached(..)` branch) gpui reuses the last prepaint when bounds, content mask and text style match, the entity is not dirty and the window is not refreshing (`gpui:src/view.rs`, the `AnyView` `prepaint` reuse branch); otherwise it calls `ViewTree::render` again. So `draws` = `NativeModuleView::render` calls, `misses` = `ViewTree` renders, `hits = draws − misses`. Before #347 every draw was a miss | C `draws` (D); `misses`, `hits` derived | 70 |
| Full redraws per interaction | nothing (#347 measured it with temporary spans: 57–67 per window switch before, 11–14 after) | the `misses` delta between two door reads around the interaction | derived (D) | 70 |
| Refresh causes | nothing | every `window.refresh()` caller in `src` is a cache-buster for all cached views in that window (`gpui:src/view.rs`, `!window.refreshing`). Today there are two, both in `src/render/text.rs`: the selection path (post-#347 `refresh_on_change`, only when the shown selection changes; before it, gpui-base's `refresh_window_on_change`) and the drag mouse-up handler (`MouseUpEvent` while `DRAG_CLIP` is set). `Window::activate_a11y` also refreshes. Count per site | C `refresh.<site>` (D) | free (`text.rs`) |
| gpui layout / paint per view | nothing. gpui's own histograms are behind the fork's `profiler` feature (§1) | `input::Observe` (`src/runtime/input.rs`) wraps the guest element: `Observe::prepaint` runs render + layout + prepaint for a cached `AnyView`, `Observe::paint` the paint. Per view only on the cached path; with a11y active `NativeModuleView::render` renders the tree uncached and taffy layout is window-wide | H `layout`, H `paint` (W) | a3 |
| Pictures | nothing | `Pictures::adopt` (`src/runtime/pictures.rs`) is insert-only, never evicts; sum `raster`/`vector` byte lengths when on | G `picture_bytes` (D) | free |
| Linear memory | nothing (the `MEMORY_LIMIT` trap) | `Exports.memory.data_size(&store)` after `Guest::tick` | G `memory` max (D) | a3 |
| Snapshot on the way out | nothing | `Guest::snapshot` (covered above) | | a3 |
| Faults | warns `module_view_trapped` (`Guest::tick`), `module_view_unloadable` (`seat::spawn_load`) | same sites | C `faults` (D) | a3 / free |

Two findings the table depends on, both true on dev today:

- **There is no time budget.** The `src/runtime.rs` module doc says a view is
  "ticked inside a fuel and time budget"; `guest::engine` sets only
  `cranelift_opt_level`, `consume_fuel(true)` and the disk cache, and no
  `epoch_interruption`, `set_epoch_deadline` or `increment_epoch` appears in
  `src`. The only bound is `FUEL_PER_TICK`, re-armed by `guest::arm` before
  every call. This is an owner decision (§7); the perf design does not
  depend on it, but the doc comment is stale and the design cannot claim a
  budget that is not there.
- **`LoadTiming.status` and `LoadTiming.check` are never written.** They are
  logged as `status_ms` and `check_ms`; the only writes in `Guest::load` are
  to `path`, `fetch`, `compile`, `init` and `first_frame`. The `LoadTiming`
  doc in `seat.rs` describes both as real. They go, and the split above
  takes their place.

### 2.2 The shell

Threads: reducer, all rendering, view ticks and the door's answers run on
the gpui foreground thread (`Desktop::dispatch` and `Desktop::start` in
`src/shell.rs`; `ax::serve` in `src/ax.rs`). I/O is driven by the
`views-kernel` tokio thread (`kernel::handle`), but reducer `Task` futures
are polled on the foreground under `runtime.enter()` in `Desktop::start`,
so response bodies are decoded there.

| Stage | Measured today | Hook point | Metric | Held |
|---|---|---|---|---|
| Startup milestones | nothing | t0 at the top of `main` (`src/main.rs`); `log` after `install_log`; `gpui` at the top of the `application.run` closure in `shell::launch::run`; `fonts` after `initialize_rendering`; `boot` after `Ducktape::boot`; `window` at `Ok(handle)` in `Desktop::open_window` (`shell/windows.rs`); `connected` in the `Connected` arm (`ui/connect.rs`); `desk` where `Ducktape::handle` (`ui/update.rs`) sees the launcher→desk crossing; `first_seated.<module>` in the new-content branch of `NativeModuleView::frame`. No first-present mark without the gpui profiler | G `startup.<mark>` ms since t0 (W) | free, except `frame` (70) |
| Frame time per window | nothing (`ZED_MEASUREMENTS=1` — `gpui_util::measure` in the fork's frame callback — prints `frame duration` to stderr for callback-driven frames only, no rebuild) | `DesktopWindow::render` (`shell.rs`) for the app's render phase; end-to-end draw+present needs the fork's `WindowProfiler` (§1, §7) | H `frame.render` per `WindowKey` (W) | free |
| Long frames > 16 ms with a cause | nothing | with only the app's spans: a ring of the last N spans over 1 ms, timestamped; a `frame.render` over budget is written to the ring with the spans that overlapped it (reducer domain, view module and stage, `turn`/`hide`/close). Draw/present/input causes need `HangDetector` over `cx.foreground_journal()` (fork, `profiler` feature) | C `long_frames`, ring `slow` (W) | free |
| Reducer update per message | nothing | `Desktop::dispatch` (`shell.rs`) whole; each routing arm of `Ducktape::update` (`ui/update.rs`) keyed by domain (`connect`, `overlay`, `notify`, `sign_in`, `desk`, `pane`). **Never key by `{:?}` of the message**: `AppMessage`'s derived `Debug` prints payloads, and `PasswordTyped`, `ApproveCodeTyped`, `PhraseWordTyped` and `RestorePhraseTyped` carry secrets. Messages arrive three ways — the spawn site in `Desktop::start`, synchronously from input listeners (`pane_message`, keys, screens), and from inside render (`DeskShown` in `DesktopWindow::console`, `shell/desk.rs`) — and `dispatch` sees all three | H `reducer.<domain>` (W) | free |
| Pane / window switch | nothing (ducktape-70's temporary `pane_message` mark and `pane_stage` span) | start at `DesktopWindow::pane_message` / `open_view` (`shell/panes.rs`), `Desktop::raise_window` (`shell.rs`) or the `WindowFocused` dispatch from `DesktopWindow::new`'s activation observer (`shell/windows.rs`); end with `window.on_next_frame` scheduled from the `DesktopWindow::render` that shows the switch. Next-frame callbacks run at the start of the frame after the switched one was presented, so this is an upper bound high by one frame interval | H `switch` (W, ms) | 70 (`panes.rs`), free (`shell.rs`, `windows.rs`) |
| Animation frames | `a_frame_is_cheap` test in `shell/figure.rs` (under 15 ms at test opt-level 1) | `Spin::render` (`shell/spin.rs`): time `Figure::frame`, and the interval between renders. `Spin::run` paces with a background timer plus `cx.notify()`, never `request_animation_frame`, so gpui's present-interval histogram and its inactive-window throttle both miss it | H `figure.frame`, H `figure.interval` (W) | free |
| Idle frames per second | nothing (#347's PR body: idle drawing over 5 s at 979 ms before, 334 ms after; the ~20 idle frames/s per window and the pulse as its driver are ducktape-70's session notes, not in the PR) | count `DesktopWindow::render` per window over a quiet interval; door draws (`ax::actions::current` forces one per read) counted separately. Known idle drivers: the status-dot pulse (`screens::pulse`, `shell/screens.rs`, an `Animation::repeat().with_max_fps(30.)` drawn by `menubar`, `menus` and `screens`; gpui's animation element calls `request_animation_frame` at that cadence) — cheap since #347 but still ~20 frames/s while motion is on; the 1 s / 300 ms / 2 s ticks below; the Spin figure's 33 ms timer | C `renders.<window>`, C `door_draws` (D) | free / a3 |
| Every dispatch re-renders every window | nothing | `Desktop::dispatch` ends in `cx.notify()`; each `DesktopWindow` observes the model and notifies itself (`DesktopWindow::new`); the subscriptions run a 1 s `WallTick`, a 300 ms `ToastTick` and, connected, a 2 s `Tick` (`Ducktape::subscriptions`, `ui/update.rs`). So an idle, connected app re-renders each window 1 + 1000/300 + 0.5 ≈ 4.8 times a second from the ticks alone. The `renders.<window>` counter above is the measure | | free |
| `rail()` per desk render | nothing | `Roster::rail` locks the roster, the registry and every seat mutex; called in `DesktopWindow::console` (`shell/desk.rs`) and through `panes::label` twice per pane in `pane_stage` | H `rail` (W), C `rail.calls` (D) | free (`roster.rs`) |
| Synchronous I/O on the window thread | nothing | `backend::session::read_prefs`/`write_prefs`; `store::answer` (`src/runtime/store.rs`, a whole-file read per `get`, write and rename per `set`, inside a guest's redraw); `notify::Center::save` (`src/runtime/notify.rs`, the whole log rewritten per post); `backend::RpcClient::new` (`backend/noded.rs`) builds a new `reqwest::Client` per call, including the 2 s status tick in `ui/connect.rs` | H `io.<site>` (W) | free |
| RSS | nothing | `libc::getrusage(RUSAGE_SELF).ru_maxrss` (libc is already a dependency): KiB on Linux, bytes on macOS; current RSS from `/proc/self/statm` on Linux only, skipped on macOS. Sampled by the same 1 s task that writes the summary | G `rss.peak`, G `rss.current` (bytes) | free |

Where the shell's biggest window-thread costs sit, for the record, all
verified: wasm ticks and frame decode (`Guest::tick`, `guest::shape`);
deep tree clones on every patch (`guest::merge`), change
(`NativeModuleView::frame`) and render (`ViewTree::render` clones `root`);
`rail()` 1 + 2×panes times per desk render (plus one more `label()` per
permission bar); `read_prefs` in render paths through
`notify::Settings::load` (the permission bar in `pane_stage`, Settings ›
Notifications); file I/O in
reducer arms and guest requests; a `reqwest::Client` per RPC. The reducer
and I/O histograms above are what will show which of these matter.

---

## 3. The mechanism

### 3.1 The registry (`src/perf.rs`, stdlib only)

```rust
//! Performance counters, on with `DUCKTAPE_PERF=1`, read at `GET /perf`.
pub(crate) fn on() -> bool                       // one relaxed atomic load

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    View { module: &'static str, instance: u64 }, // the seat key (`seat::mounted`)
    Window(crate::runtime::WindowKey),
    Shell,                                       // startup, reducer, rss
}

/// `None` when off: the caller never takes an `Instant`.
pub(crate) fn time(key: Key, stage: &'static str) -> Option<Timer>
pub(crate) fn count(key: Key, stage: &'static str, n: u64)
pub(crate) fn gauge(key: Key, stage: &'static str, value: u64) // keeps max and last
pub(crate) fn record(key: Key, stage: &'static str, value: u64) // a sample that is not a time (fuel, bytes)
pub(crate) fn mark(stage: &'static str)          // ms since t0, once

pub(crate) fn snapshot() -> serde_json::Value
pub(crate) fn reset()
```

- `Timer` records `elapsed()` on drop into the histogram named by its key.
- The store is `static REGISTRY: Mutex<BTreeMap<(Key, &'static str), Samples>>`
  with `Samples { n, sum, max, last, ring: [u64; 256] }`. `n`, `sum` and
  `max` cover everything since the last reset; p50/p95 come from the ring
  and cover only the last 256 samples. `ponytail:` one global lock, one
  fixed ring; fine at fewer than a few thousand samples a second.
- Keys are `&'static str`: module names are interned already
  (`runtime::intern`), stage names are literals.
- Keyed by **(module, instance)**, not module: the seat registry is
  `(module, instance)` (`seat::mounted`, `NativeModuleView::new`), and two
  windows can show the same view. `/perf` aggregates by module on request.
- Why not on `Guest`: stats must survive a swap and cover load stages that
  run before the `Guest` exists. The registry is a leaf lock; reading it
  never takes a seat lock.
- Windows are keyed by `WindowKey` (`src/runtime.rs`). The door names
  windows positionally (`console`, `console2`, … in `Desktop::ax_windows`),
  which shifts when one closes, so the snapshot carries both.

**Cost when off.** `on()` is an atomic load. `time` returns `None` before
touching the clock; `count`/`gauge`/`record` return before the lock. ducktape-70 reports that
its temporary helper's `span()` took `Instant::now()` unconditionally and
tested the switch on drop (never committed, so not checked here); the
permanent version must not copy that.

**Cost when on.** Two `Instant::now()`, one lock, one map lookup and an
O(1) ring write per sample. The node-count walk and picture-byte sums run
only when on and only when the tree changed.

### 3.2 One tracing target

- `ducktape::perf` carries the per-event debug stream: the per-tick fuel
  event that lives in `Guest::tick` today under `ducktape::fuel`, plus one
  debug event per registry sample. Callsite-filtered, so it costs nothing
  unless `RUST_LOG=ducktape::perf=debug` (the filter is `info` plus
  `RUST_LOG`, `install_log`).
- The rename is `ducktape::fuel` → `ducktape::perf`, no alias. Two sites:
  the `debug!` in `Guest::tick` (held, a3) and the `FUEL_PER_TICK` doc
  comment in `src/runtime.rs` that names `RUST_LOG=ducktape::fuel=debug`
  (free; goes stale the moment the target moves, so same PR).
- `view_load` (`LoadTiming::log`) moves to this target as well, gains
  `blob`, `code`, `instantiate_ms`, `restore_ms`, `snapshot_ms`,
  `snapshot_bytes` and `install_ms`, drops `status_ms` and `check_ms`, and is
  logged after install rather than before it. Nothing in qa reads the old
  field names.

### 3.3 The door: `GET /perf`

- Route: `("GET", "perf")` in `ax::http::route`, listed in its 404 endpoint
  list; `Request::Perf` in `src/ax.rs`; a CLI verb `ducktape-app ax perf` in
  `ax::http::cli`. `POST /perf/reset` clears; `GET` never mutates.
- **It must not draw.** `ax::actions::current` turns a11y on at the first
  read and calls `window.draw(cx)` on every read; a11y on switches
  `NativeModuleView::render` to the uncached tree. `/perf` is answered in
  `ax::answer` straight from `perf::snapshot()` with no window read, or on
  the `ax-door` accept thread (`ax::http::open`) before the request is
  forwarded to the app thread. Both files are free (§5).
- Reply shape, one object per key:

```json
{ "on": true, "since_ms": 123456,
  "startup": { "log": 3, "gpui": 41, "fonts": 60, "boot": 70, "window": 190, "connected": 800, "desk": 1500 },
  "views": { "chat/7": { "module": "chat", "instance": 7, "window": "console",
      "ticks": 412, "busy_ticks": 3, "renders": 415, "faults": 0,
      "fuel": { "tick": { "n": 412, "p50": 4100000, "p95": 31000000, "max": 71800000, "limit": 250000000 } },
      "us": { "tick.call": {...}, "tick.decode": {...}, "merge": {...}, "replace": {...}, "render": {...}, "layout": {...}, "paint": {...} },
      "bytes": { "frame": {...}, "events": {...}, "snapshot": 20480, "memory_max": 12582912, "pictures": 300000, "view": 1830000 },
      "frames": { "full": 3, "patch": 90, "unchanged": 319 }, "nodes_max": 1900, "truncations": 0,
      "requests": {...}, "backlog_max": 2,
      "host_call": { "module.query": { "n": 20, "p50": 9000, "p95": 40000, "max": 90000, "attempts": 20 } },
      "load": { "path": "first", "fetch_us": 12000, "blob": "disk", "compile_us": 900, "code": "memory",
                "instantiate_us": 400, "restore_us": 0, "init_us": 3000, "install_us": 200, "first_tree_ms": 18 } } },
  "windows": { "1": { "name": "console", "renders": 240, "door_draws": 0, "us": { "frame.render": {...} }, "switch_ms": {...} } },
  "shell": { "reducer": { "connect": {...}, "pane": {...} }, "io": { "read_prefs": {...}, "store": {...} }, "rail": {...},
             "figure": { "frame": {...}, "interval": {...} }, "long_frames": 0, "slow": [ {"at_ms": 1200, "key": "chat/7", "stage": "tick.call", "us": 31000} ] },
  "rss": { "peak": 400000000, "current": 350000000 } }
```

- `"cache_on"`: true when no served window has a11y active (`Window::is_a11y_active`).
  A gate refuses to judge cache metrics (`misses`, `hits`, full redraws per
  switch, idle drawing) from a reply with `cache_on: false` (§4.3).
- Off: `{"error": "perf is off: launch with DUCKTAPE_PERF=1"}` with status
  409, so a gate fails loudly instead of passing on empty data.
- Access as for every door call: loopback only, bearer token from the door
  file (`ax::http::open`).

### 3.4 app.log summaries

- One `perf_summary` info line every 10 minutes if anything changed, and
  once from `cx.on_app_quit` in `shell::launch::run` — the one hook every
  quit path reaches (`Desktop::quit` runs only for `Command::Quit`, the
  tray and menu path).
- One `view_perf` info line per instance when it retires: swap (`seat::spawn_load`,
  the `Loaded::Swap` arm replaces `slot`), retry (`seat::retry`) and roster
  removal (`roster::spawn_roster_read`). There is no `impl Drop for Guest`
  today (the only drops are `NativeModuleView`, `NodeTask`, `InFlight`); one
  goes in, skipping instances whose `installed_generation` is `None`.
- Same stable field names as the `/perf` JSON, so a log line and a door
  reply are the same numbers.

### 3.5 Deep dives (dev only)

- **`perf` on Linux.** `DUCKTAPE_WASM_PROFILER=perfmap|jitdump` sets
  `Config::profiler(ProfilingStrategy::PerfMap | JitDump)` in
  `guest::engine`. The strategy is not part of the compile-cache key
  (`HashedEngineCompileEnv` in wasmtime's `compile/code_builder.rs` hashes
  the compiler triple, flags, isa flags, tunables, features, `wmemcheck` and
  `module_version`), so the disk cache stays warm and every module registers
  with the agent, cache hits included. wasmtime's `profiling` feature is in
  its defaults.
- **`GuestProfiler` (Firefox-profiler JSON).** `DUCKTAPE_WASM_PROFILE=<module>`.
  It needs `Config::epoch_interruption(true)`, a ticker calling
  `Engine::increment_epoch`, `Store::set_epoch_deadline` in `guest::arm`
  and `Store::epoch_deadline_callback` to call `GuestProfiler::sample` and
  continue; `GuestProfiler::finish` writes the file. `epoch_interruption` is
  a **tunable**, so it is in the cache key: the one process-wide `Engine`
  compiles every view cold while this is on, and every `arm` needs the
  deadline. Keep it a dev flag. If the owner adopts a real time budget (§7),
  epoch interruption is always on and this cost disappears.
- **Names.** Shipped views are stripped and `wasm-opt`'d
  (`modules/tools/view-gate.sh` → `optimize-view.sh`); `make wasm-why
  V=<view>` in modules keeps names and skips `wasm-opt`. Use it for *where*
  time goes, not *how much*. It can only be seated through
  `DUCKTAPE_VIEWS_DIR`, and override loads are always `Fresh` and log no
  `view_load`, so the swap/snapshot/restore path cannot be profiled that way.
- **Not `Store::call_hook`.** It needs wasmtime's non-default `call-hook`
  feature.
- **gpui histograms.** An app feature `perf-deep = ["gpui-kit/profiler"]`
  would expose `Window::frame_duration_snapshot`,
  `Window::input_latency_snapshot`, `App::foreground_journal` and
  `HangDetector`. Owner call (§7).

---

## 4. Budgets and the qa gate

### 4.1 Ceilings the app enforces today

| Ceiling | Value | Effect |
|---|---|---|
| `FUEL_PER_TICK` | 250M | trap → fault, "This view stopped" |
| `MEMORY_LIMIT` | 64 MiB | trap on grow |
| `MAX_FRAME_BYTES` | 8 MiB | refused in `guest::shape` → fault |
| `MAX_REQUESTS_PER_TICK` | 256 | refused in `guest::shape` → fault |
| `MAX_NODES` (view-wire) | 8192 | truncation, not a kill |
| `MAX_PATCHES` (view-wire) | 1024 | patch refused, `Resync` asked |
| `MAX_SNAPSHOT_BYTES` (view-wire) | 8 MiB | checked by `wire::Snapshot::decode` in `Guest::load` |

### 4.2 Initial budgets

Measured so far (fold in, attribute, do not over-trust):

- Fuel, heaviest ticks, `src/runtime.rs` doc: chat ~72M, forge ~45M,
  explorer ~26M.
- #347 (`193c9a21`), seeded stage, three windows, Xvfb, measured with
  ducktape-70's temporary spans before and after the fix. These replace the
  earlier `view_tree.render` ~2.9 ms figure, which was taken with the cache
  broken:

  | | before #347 | after #347 |
  |---|---|---|
  | full view redraws per window switch | 57–67 | 11–14 |
  | drawing time per frame | 11–12 ms | 2.5–3.5 ms |
  | idle drawing over 5 s | 979 ms | 334 ms |
  | idle frames/s (status-dot pulse, motion on) | ~20 | ~20, now cheap |

  Root causes, both "cached guest trees never stayed cached": an id-less
  container measured on its named ancestor's path and flagged a change
  ~2,900 times per 2 s (`ViewTree::measure`, `src/render/layout.rs`);
  gpui-base's selection sweep sent `SelectionChanged(None)` every frame and
  `refresh_window_on_change` refreshed the window on each (now
  `refresh_on_change`, `src/render/text.rs`). The 57–67 / 11–14, 11–12 /
  2.5–3.5 ms and 979 / 334 ms figures and the ~2,900 count are the PR's;
  the rest of the idle story is ducktape-70's session. Wall numbers are one
  rig, one run: **report-only until three baseline runs on the reference
  rig.**
- members-view sits at 98.5% of its wasm size limit (owner's number; the
  modules tooling enforces no limit — `tools/view-gate.sh` prints
  `name  bytes` and nothing else). Its top
  guest-side cost is `view_wire::Node` serialize/clone/eq. The host cannot
  time guest encode, but it sees the result: track `bytes.view` (the
  `view_section` length per load) and `bytes.frame` per tick as D metrics,
  and `tick.call` as the encode time's upper bound — the guest settles,
  diffs and encodes inside the one `Exports::tick` call, so `bytes.frame`
  and `tick.call` together are the tracked pair.

Rules, in `qa`'s `perf-budgets.json`, keyed by module and by window:

1. **Hard ceiling**: 50% of each kill ceiling, every view. Fuel max ≤ 125M,
   frame bytes max ≤ 4 MiB, requests max ≤ 128, memory max ≤ 32 MiB.
2. **Regression budget**: baseline × 1.25 on the D metrics — fuel max and
   p95, frame bytes max, nodes max, requests max, truncations, snapshot
   bytes, memory max, picture bytes, view bytes — refreshed from a green
   run on `dev`. Starting points: chat 72M → 90M, forge 45M → 56M, explorer
   26M → 33M. Gate on `max` and counts since the last reset, never on
   equality: inputs vary with when replies and blocks land.
3. **Idle budget**: after a scenario settles, `POST /perf/reset`, wait 2 s
   with **no door calls**, then `GET /perf`:
   - `busy_ticks == 0`;
   - `ticks ≤ clock items + block-subscription items + 1`. Clocks have a
     16 ms floor (`MIN_TICK_MS`, `src/runtime/kernel.rs`); `module.changes`
     delivers one item per block that wrote to the program, `chain.heads` polls
     at block_time/2 clamped to 100 ms–2 s and delivers one item per block
     (`kernel::node::live`, `heads`);
   - `renders ≤ ticks + 2` per view, because every tick notifies the
     ViewTree in `NativeModuleView::frame`. Renders far above ticks is a
     notify loop: the #347 class of bug, and this is its regression gate
     (only meaningful with `cache_on: true`, §4.3);
   - shell `renders.<window>` ≤ 5 × seconds + 2 with motion off (the 1 s,
     300 ms and 2 s ticks: 1 + 1000/300 + 0.5 ≈ 4.8/s); ≤ 25 × seconds with
     motion on (the pulse's 30 fps cap); `door_draws == 0`. Preconditions,
     or a healthy app fails: no rail row saying `Loading`
     (`DesktopWindow::console` requests an animation frame per render while
     one does) and no empty pane in any served window (`pane_stage` draws the
     Spin figure there, paced at 33 ms by `Spin::run` whatever the window's
     activity).
   - idle drawing time per 5 s ≤ 500 ms (334 measured), report-only.
4. **Switch budget** (the keys-only scenario, §4.3): full view redraws per
   window switch (`misses` delta) ≤ 20 per window (11–14 measured; 57–67 was
   the bug); drawing time per frame p95 ≤ 6 ms, report-only (2.5–3.5
   measured).
5. **Wall clock**: report only under Xvfb (qa's rig is Xvfb: `qa/src/rig.rs`,
   `walk.py`), apart from an absolute hang ceiling: no single
   `frame.render`, `reducer.*` or `tick.call` sample ≥ 250 ms in the steady
   phase. Proposed, to be set from recorded runs: reducer p99 ≤ 2 ms,
   `io.*` p99 ≤ 2 ms, `figure.frame` p95 ≤ 2 ms, `switch_ms` p95 ≤ 50 ms,
   `startup.desk` ≤ 3 s with a local node.
6. **RSS peak** ≤ baseline × 1.2.

**Caveat on every qa number.** The door turns a11y on at its first read and
draws on every read (`ax::actions::current`), and a `wait` polls every
50 ms (`ax::answer`, `POLL`). With a11y on, `NativeModuleView::render`
takes the uncached path. So qa's render/layout numbers are for the uncached
path and are not user numbers; the idle window must contain no door reads;
`door_draws` is reported so a reviewer can see how much of a window's
render count is the door's. The sharper consequence is §4.3.

### 4.3 The a11y trap, and measuring with the cache on

With a11y active every draw of a guest view is a full `ViewTree::render`:
`misses == draws` by construction, and the whole #347 class of bug (a cached
tree that never stays cached) is invisible. A perf gate driven through
`/tree`, `/act` or `/wait` would have passed before #347 and would pass again
if it regressed. There is no app-callable way to turn a11y back off:
`gpui:src/window.rs` has `activate_a11y`, and deactivation belongs to the
platform adapter. A "keep the cache" door flag is therefore not a flag but a
launch mode: with a11y off, `/tree`, `/act` and `/wait` have no guest nodes
to serve.

Design:

- `/perf` never draws and never activates a11y (§3.3), and reports
  `cache_on`.
- Cache-independent D metrics — fuel, bytes, nodes, requests, ticks,
  truncations, memory, faults — gate in the same launch as any scenario.
- Cache-dependent metrics — `misses`, `hits`, full redraws per switch, idle
  drawing — gate only from a reply with `cache_on: true`. They get their own
  keys-only scenario, `perf-switch`: launch, connect, open three heavy views
  (chat on a busy channel, forge, members) by keyboard alone, switch windows
  twice by keyboard, idle 5 s, `GET /perf`. Its budgets are rule 4 and the
  idle drawing line above.
- **Today's `/key` is not keys-only.** `Request::Key` in `ax::answer` reads
  every window's tree before the press (to pick the window and compute the
  delta) and `settle`s on tree reads after it, so it activates a11y like
  `/tree`, `/act` and `/wait`; so do `/keys` and `/drag`. The scenario needs
  a press that reads nothing: a `POST /key` body flag (`"delta": false`, say)
  that skips both reads and answers `{}` — in `src/ax.rs`, which is free —
  or OS-level key injection on the rig's Xvfb. This document picks the flag.
  With `window` named, `keyboard_window` already resolves the handle from the
  door's window list (`Desktop::ax_windows`) without a tree; unnamed, it
  finds the focused node in the tree it just read, so the flag makes
  `window` required (falling back to the first window, as it does today).
- The gate refuses to judge a cache metric from a `cache_on: false` reply
  and says so, instead of passing.
- Recommended default for the owner (§7): the extra scenario plus the
  no-read key, not a door launch mode.

### 4.4 The gate

- **Where**: at the end of each scenario **and** of each mission, not as a
  new step kind. The two runners have different step-kind lists and an
  unknown kind fails at load. Rust: end of `run_scenario` (`qa/src/steps.rs`)
  and `run_mission` (`qa/src/mission.rs`). Python: after the step loop and
  at the end of `Walk.run_mission` in `walk.py`.
- **How**: `POST /perf/reset` at scenario start; at its end `GET /perf`
  through the door client (`qa/src/door.rs` `call`), then the idle window
  (§4.2 rule 3), then a second `GET /perf`. Write both as a `perf.jsonl`
  line beside `transcript.jsonl`; put the breaches in the scenario's
  result.
- **Failure**: an over-budget D metric or a steady-phase hang fails the
  scenario as `perf-over-budget <module|window> <metric> <value> > <budget>`.
  Oracles cannot do this: they see trees and context, not the door.
- **Baseline**: `--perf-baseline <run-dir>` fails any D `max` above 1.25 ×
  baseline; a green run on `dev` refreshes it.
- **Rig**: `DUCKTAPE_PERF=1` beside `DUCKTAPE_AX_DOOR=0` in `qa/src/rig.rs`
  and `walk.py`.
- **A mission that drives the gate**: connect to the local node, open every
  program in the rail, switch panes and windows across all of them, type
  into chat, open forge with a large file, settle, then the idle window.
  This covers load (fetch/compile/init), swap (a view redeployed mid-run;
  `kit seed` fills a stage with data and deploys no view), tick, switch and
  idle in one pass.
- **`cargo test` counts**, already in CI: promote `ViewTree.renders` out of
  `cfg(test)` and assert deterministic counts — an idle ViewTree renders
  zero times across N frames (#347 added exactly this,
  `an_unchanged_guest_tree_is_not_drawn_again` in `src/render/tests/layout.rs`,
  on `ViewTree.renders`; promoting the counter keeps it unchanged); a
  hand-dispatched `WallTick` with nothing changed renders no ViewTree (tests
  skip subscriptions in `Desktop::subscriptions`); a still figure asks for
  no frames. Times are flaky under the test scheduler; counts are not.

---

## 5. Rollout

The view-stage hooks sit in files other sessions have in flight.
ducktape-70 (window-switch perf) holds `src/render.rs`,
`src/runtime/widget.rs`, `src/shell/panes.rs` and their tests; its fix has
merged as #347 but the hold is not assumed lifted, and its instrumentation
was never committed (§6), so nothing here conflicts with it. ducktape-a3
(snapshot fix, dead wire-variant removal) holds `src/runtime/guest.rs`,
`src/runtime/guest/{lifecycle,requests,tests}.rs`, `src/runtime/input.rs`
and `input/*`, `src/render/{inputs,commands,interactivity}.rs`, render
tests, `src/editor/wire*`, `src/ax/actions.rs`. Nothing new is created
under `src/render/` (that needs a `mod` line in the held `render.rs`).

**Phase 0 — lands now, free files only.**
- `src/perf.rs`: switch, registry, `snapshot`, `reset`, `mark`, the 10-minute
  summary task and the RSS sampler (`shell::launch::run`), `perf_summary` on
  `on_app_quit`.
- `GET /perf`, `POST /perf/reset`, `Request::Perf`, `ax perf` CLI
  (`src/ax.rs`, `src/ax/http.rs`), answered without a window read.
- Shell hooks: startup marks (`main.rs`, `shell/launch.rs`,
  `shell/windows.rs`, `ui/connect.rs`, `ui/update.rs`); `Desktop::dispatch`
  and the `Ducktape::update` domains; `DesktopWindow::render` frame.render
  and per-window render counts (`shell.rs`); `Spin::render` figure timing;
  `Roster::rail`; I/O sites (`backend/session.rs`, `runtime/store.rs`,
  `runtime/notify.rs`, `backend/noded.rs`).
- View hooks in free files: blob source and view bytes
  (`backend/views.rs` `program_bytes`), host-call latency and attempts
  (`kernel/node.rs`), backlog (`kernel/replies.rs`), install and lock wait,
  the `view_perf` retire line and the `LoadTiming` cleanup in `seat.rs`
  (the fields' writers in `lifecycle.rs` wait for phase 1), roster timing,
  picture bytes (`runtime/pictures.rs`).
- Same PR: the `FUEL_PER_TICK` doc comment in `src/runtime.rs` stops naming
  `ducktape::fuel`; the `LoadTiming` doc stops describing `status`/`check`.

**Phase 1 — after ducktape-70 releases its files and ducktape-a3 merges.**
The `draws` counter and the promoted `renders` (cache hit/miss, full redraws
per switch — the #347 metrics) are the first things to land here.
- `Guest::tick` split (call / decode / merge), fuel target rename, frame
  kind, bytes, nodes, requests, truncations, memory (`guest/requests.rs`,
  `guest.rs`); the load-stage split and fuel per call in
  `guest/lifecycle.rs`; `compile_view` source.
- `NativeModuleView::frame`: replace timing, `first_tree`, `first_seated`
  mark; `ViewTree::render` renders counter and time (`render.rs`);
  `Observe::prepaint`/`paint` layout and paint (`runtime/input.rs`); switch
  start in `pane_message` (`shell/panes.rs`).
- `door_draws` in `ax::actions::current`.

**Phase 2 — qa.** `DUCKTAPE_PERF=1` in both rigs, `perf.jsonl`,
`perf-budgets.json`, the end-of-scenario and end-of-mission gate with the
idle window in both runners, the driving mission (§4.4).

**Phase 3 — dev deep dives.** `DUCKTAPE_WASM_PROFILER`,
`DUCKTAPE_WASM_PROFILE`.

**Phase 4 — owner calls (§7).** The gpui `profiler` feature; blocking
merges on budgets; a real time budget.

---

## 6. The temporary `DUCKTAPE_PERF` helper

As ducktape-70 describes it (never committed — nothing below was checked
against code): its `src/perf.rs` (29 lines: `on()`, a `Span` that
`eprintln!`s on drop, `mark()`) and its span sites (`desk.render`,
`pane_stage`, `view_props`, `module_view.render`, `module_view.turn`,
`module_view.replace`, `view_tree.render`, `guest.tick`, a mark on every
`pane_message`) are **folded into this design, not merged as they are**:

- `on()` becomes the permanent switch, same shape.
- `Span`/`eprintln` become registry records. Its `span()` takes
  `Instant::now()` unconditionally and checks the switch only on drop; the
  permanent `time()` returns `None` before touching the clock.
- `guest.tick` becomes the `tick.call`/`tick.decode`/`merge` split;
  `module_view.replace` becomes `replace`; `view_tree.render` becomes
  `render` plus the `renders` counter.
- `module_view.render`, `module_view.turn`, `desk.render`, `pane_stage`,
  `view_props` and the `pane_message` mark were window-switch diagnostics.
  They survive only as `ducktape::perf` debug events while the switch is
  under study; `mark()` as a stderr printer goes.
- Its first suspect — `pane_stage` calling `set_focused`/`set_props` every
  render — was not the cause. The root causes were the two cache-busting
  loops in `render/layout.rs` and `render/text.rs` (§4.2), merged as #347
  with a unit test and without any of the instrumentation: the helper was
  never committed. Its early numbers (`view_tree.render` ~2.9 ms,
  `module_view.render` 7–25 µs) were taken with the cache broken and are
  superseded by the #347 before/after. The `renders ≤ ticks + 2` idle gate
  and the `misses` delta per switch are what would have caught both loops,
  and what catches the next one — with the cache on (§4.3).
- No `[patch]` of gpui-pre to a scratchpad path lands.

---

## 7. Open questions for the owner

1. **The gpui `profiler` feature.** It is the only route to end-to-end frame
   time (draw + present), input→present latency and gpui-level long-frame
   causes, and to a first-present startup mark. It costs one lock entry
   (`hdrhistogram` 7.5.4; its deps are already locked), a journal ring at
   `App` build and timestamps on draws, inputs and task polls whenever
   compiled in. Options: (a) never; (b) an app feature `perf-deep =
   ["gpui-kit/profiler"]` for dev and qa builds only, accepting that users
   and qa then run different binaries; (c) always on, accepting the cost.
   Phase 0 assumes (a) until told otherwise.
2. **A time budget.** The `src/runtime.rs` doc claims one; none exists.
   Either add epoch interruption (always on; then the guest profiler no
   longer splits the compile cache) or delete the claim. The perf design
   works either way.
3. **Do budgets block merges?** Fuel, bytes, nodes and idle counts are
   deterministic enough to gate. Until the call, the qa gate reports and
   does not fail.
4. **Measuring with the cache on.** §4.3 proposes one extra keys-only
   scenario per qa run for the cache-dependent metrics, because a11y cannot
   be turned off once the door has read a tree — and every door verb today,
   `/key` included, reads one, so the scenario also needs a `/key` that
   skips its reads. The alternative is a door launch mode that never
   activates a11y and refuses `/tree`, `/act` and `/wait` for the whole run.
   This document picks the scenario and the no-read key.
5. **Instance vs module keys on the door.** The registry is per
   `(module, instance)`; should `/perf` default to aggregating by module
   for qa's budgets, with instances behind `?by=instance`?
6. **`POST /perf/reset` vs `GET /perf?reset=1`.** This document picks the
   POST; the door's route table is a `(method, target)` match and the qa
   client handles both methods.
7. **`store.get`/`store.set` and the notification log on the window
   thread.** The I/O histograms will show whether they matter; if they do,
   moving them to the `views-kernel` thread is a runtime change, not a perf
   one — flagging it now so it is not a surprise later.
